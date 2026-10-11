//! Last.fm scrobbling, under the person's own Last.fm API account: its key
//! and shared secret come from last.fm/api/account/create, and the account
//! is authorised in the browser (Last.fm's desktop flow), so no password
//! passes through the app.
//!
//! A song is scrobbled once it has been heard for half its length or four
//! minutes, whichever comes first, as Last.fm asks; songs under 30 seconds
//! are not. Scrobbles wait in a file while Last.fm cannot be reached.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::model::Track;

const API: &str = "https://ws.audioscrobbler.com/2.0/";

/// The API account and the session, kept encrypted with DPAPI.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Stored {
    pub key: String,
    pub secret: String,
    pub session: Option<String>,
    pub user: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scrobble {
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
    pub duration: Option<u32>,
    /// When it started playing, in Unix seconds.
    pub timestamp: i64,
}

/// The song playing, as the app sees it this frame.
pub struct Current<'a> {
    pub uid: u64,
    pub track: &'a Track,
    pub position: f64,
    pub duration: Option<f64>,
    pub playing: bool,
}

pub enum Notice {
    OpenUrl(String),
    Toast { text: String, error: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Disconnected,
    /// Waiting for the person to allow access in the browser.
    Authorising,
    Connected,
}

enum Done {
    Token(Result<String, String>),
    Session(Result<(String, String), ApiError>),
    Scrobbled { count: usize, result: Result<(), ApiError> },
    NowPlaying(Result<(), ApiError>),
}

#[derive(Debug)]
enum ApiError {
    /// Last.fm answered with an error code.
    Api(i64, String),
    /// Last.fm could not be reached.
    Network(String),
}

struct Play {
    uid: u64,
    scrobble: Scrobble,
    /// Seconds actually heard, seeks left out.
    heard: f64,
    last_position: f64,
    last_tick: Instant,
    was_playing: bool,
    announced: bool,
    done: bool,
}

struct Authorising {
    token: String,
    until: Instant,
    next_poll: Instant,
    polling: bool,
}

pub struct LastFm {
    pub stored: Stored,
    path: PathBuf,
    queue_path: PathBuf,
    pending: Vec<Scrobble>,
    sending: bool,
    next_send: Instant,
    play: Option<Play>,
    authorising: Option<Authorising>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    runtime: tokio::runtime::Handle,
    http: reqwest::Client,
}

impl LastFm {
    pub fn new(config: &std::path::Path, runtime: tokio::runtime::Handle, http: reqwest::Client) -> Self {
        let path = config.join("lastfm.bin");
        let queue_path = config.join("scrobbles.json");
        let stored = std::fs::read(&path)
            .ok()
            .and_then(|bytes| crate::platform::unprotect(&bytes))
            .and_then(|plain| serde_json::from_slice(&plain).ok())
            .unwrap_or_default();
        let pending: Vec<Scrobble> = crate::settings::load(&queue_path);
        let (tx, rx) = channel();
        Self {
            stored,
            path,
            queue_path,
            pending,
            sending: false,
            next_send: Instant::now(),
            play: None,
            authorising: None,
            tx,
            rx,
            runtime,
            http,
        }
    }

    pub fn status(&self) -> Status {
        if self.stored.session.is_some() {
            Status::Connected
        } else if self.authorising.is_some() {
            Status::Authorising
        } else {
            Status::Disconnected
        }
    }

    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    /// Starts the browser authorisation with this API account.
    pub fn connect(&mut self, key: &str, secret: &str) {
        self.stored = Stored {
            key: key.trim().to_owned(),
            secret: secret.trim().to_owned(),
            session: None,
            user: None,
        };
        let (http, tx, stored) = (self.http.clone(), self.tx.clone(), self.stored.clone());
        self.runtime.spawn(async move {
            let result = call(&http, &stored, "auth.getToken", Vec::new(), false)
                .await
                .map_err(|e| describe(&e))
                .and_then(|v| v["token"].as_str().map(str::to_owned).ok_or_else(|| "no token".to_owned()));
            let _ = tx.send(Done::Token(result));
        });
    }

    pub fn disconnect(&mut self) {
        self.stored.session = None;
        self.stored.user = None;
        self.authorising = None;
        self.play = None;
        self.save();
    }

    fn save(&self) {
        let Ok(json) = serde_json::to_vec(&self.stored) else {
            return;
        };
        if self.stored.key.is_empty() && self.stored.session.is_none() {
            let _ = std::fs::remove_file(&self.path);
        } else if let Some(sealed) = crate::platform::protect(&json) {
            crate::settings::write_atomic(&self.path, &sealed);
        }
    }

    fn save_queue(&self) {
        if self.pending.is_empty() {
            let _ = std::fs::remove_file(&self.queue_path);
        } else {
            crate::settings::save(&self.queue_path, &self.pending);
        }
    }

    /// Follows what plays; call every frame (and every few seconds while
    /// the window is closed). Returns what the interface should show or do.
    pub fn tick(&mut self, current: Option<Current<'_>>, enabled: bool) -> Vec<Notice> {
        let mut notices = Vec::new();
        self.collect(&mut notices);
        self.poll_authorisation();
        if !enabled || self.stored.session.is_none() {
            self.play = None;
            return notices;
        }
        match current {
            None => self.play = None,
            Some(current) => self.follow(&current),
        }
        if !self.pending.is_empty() && !self.sending && Instant::now() >= self.next_send {
            self.send_batch();
        }
        notices
    }

    fn follow(&mut self, current: &Current<'_>) {
        let now = Instant::now();
        let restarted = self.play.as_ref().is_some_and(|play| {
            // The same entry from the top again (repeat one).
            play.uid == current.uid && play.done && current.position < 3.0 && play.last_position > 10.0
        });
        if restarted || self.play.as_ref().is_none_or(|play| play.uid != current.uid) {
            let Some(artist) = current.track.artists.first().map(|a| a.name.clone()) else {
                self.play = None;
                return;
            };
            self.play = Some(Play {
                uid: current.uid,
                scrobble: Scrobble {
                    artist,
                    track: current.track.title.clone(),
                    album: current.track.album.as_ref().map(|a| a.name.clone()),
                    duration: current.duration.map(|d| d.round() as u32).or(current.track.duration),
                    timestamp: unix_now() - current.position.max(0.0) as i64,
                },
                heard: 0.0,
                last_position: current.position,
                last_tick: now,
                was_playing: current.playing,
                announced: false,
                done: false,
            });
        }
        let Some(play) = &mut self.play else { return };
        let moved = current.position - play.last_position;
        let elapsed = now.duration_since(play.last_tick).as_secs_f64();
        // Steady playback moves the position with the clock; a seek does not.
        if play.was_playing && current.playing && moved > 0.0 && (moved - elapsed).abs() < 2.5 {
            play.heard += moved;
        }
        play.last_position = current.position;
        play.last_tick = now;
        play.was_playing = current.playing;
        if current.playing && !play.announced {
            play.announced = true;
            let scrobble = play.scrobble.clone();
            self.now_playing(scrobble);
        }
        let Some(play) = &mut self.play else { return };
        let length = play.scrobble.duration.map_or(f64::INFINITY, f64::from);
        if !play.done && length >= 30.0 && play.heard >= (length / 2.0).min(240.0) {
            play.done = true;
            log::info!("scrobbling {} - {}", play.scrobble.artist, play.scrobble.track);
            self.pending.push(play.scrobble.clone());
            self.save_queue();
            self.next_send = Instant::now();
        }
    }

    fn now_playing(&self, scrobble: Scrobble) {
        let (http, tx, stored) = (self.http.clone(), self.tx.clone(), self.stored.clone());
        self.runtime.spawn(async move {
            let mut params = vec![("artist".to_owned(), scrobble.artist), ("track".to_owned(), scrobble.track)];
            if let Some(album) = scrobble.album {
                params.push(("album".into(), album));
            }
            if let Some(duration) = scrobble.duration {
                params.push(("duration".into(), duration.to_string()));
            }
            let result = call(&http, &stored, "track.updateNowPlaying", params, true).await.map(|_| ());
            let _ = tx.send(Done::NowPlaying(result));
        });
    }

    fn send_batch(&mut self) {
        self.sending = true;
        let batch: Vec<Scrobble> = self.pending.iter().take(50).cloned().collect();
        let count = batch.len();
        let (http, tx, stored) = (self.http.clone(), self.tx.clone(), self.stored.clone());
        self.runtime.spawn(async move {
            let mut params = Vec::new();
            for (i, s) in batch.into_iter().enumerate() {
                params.push((format!("artist[{i}]"), s.artist));
                params.push((format!("track[{i}]"), s.track));
                params.push((format!("timestamp[{i}]"), s.timestamp.to_string()));
                if let Some(album) = s.album {
                    params.push((format!("album[{i}]"), album));
                }
                if let Some(duration) = s.duration {
                    params.push((format!("duration[{i}]"), duration.to_string()));
                }
            }
            let result = call(&http, &stored, "track.scrobble", params, true).await.map(|_| ());
            let _ = tx.send(Done::Scrobbled { count, result });
        });
    }

    fn poll_authorisation(&mut self) {
        let Some(auth) = &mut self.authorising else { return };
        if Instant::now() > auth.until {
            self.authorising = None;
            return;
        }
        if auth.polling || Instant::now() < auth.next_poll {
            return;
        }
        auth.polling = true;
        let token = auth.token.clone();
        let (http, tx, stored) = (self.http.clone(), self.tx.clone(), self.stored.clone());
        self.runtime.spawn(async move {
            let result = call(&http, &stored, "auth.getSession", vec![("token".into(), token)], false)
                .await
                .and_then(|v| {
                    let key = v.pointer("/session/key").and_then(Value::as_str);
                    let name = v.pointer("/session/name").and_then(Value::as_str);
                    match (key, name) {
                        (Some(key), Some(name)) => Ok((key.to_owned(), name.to_owned())),
                        _ => Err(ApiError::Network("no session in the answer".into())),
                    }
                });
            let _ = tx.send(Done::Session(result));
        });
    }

    fn collect(&mut self, notices: &mut Vec<Notice>) {
        while let Ok(done) = self.rx.try_recv() {
            match done {
                Done::Token(Ok(token)) => {
                    notices.push(Notice::OpenUrl(format!(
                        "https://www.last.fm/api/auth/?api_key={}&token={token}",
                        self.stored.key
                    )));
                    self.authorising = Some(Authorising {
                        token,
                        until: Instant::now() + Duration::from_secs(300),
                        next_poll: Instant::now() + Duration::from_secs(4),
                        polling: false,
                    });
                }
                Done::Token(Err(error)) => notices.push(Notice::Toast {
                    text: format!("Last.fm did not accept that API account: {error}"),
                    error: true,
                }),
                Done::Session(Ok((key, name))) => {
                    self.authorising = None;
                    self.stored.session = Some(key);
                    self.stored.user = Some(name.clone());
                    self.save();
                    notices.push(Notice::Toast {
                        text: format!("Scrobbling to Last.fm as {name}"),
                        error: false,
                    });
                }
                Done::Session(Err(error)) => {
                    if let Some(auth) = &mut self.authorising {
                        auth.polling = false;
                        auth.next_poll = Instant::now() + Duration::from_secs(4);
                        // 14: not authorised yet. Anything but that, or the
                        // network, ends the attempt.
                        if let ApiError::Api(code, message) = &error
                            && *code != 14
                        {
                            self.authorising = None;
                            notices.push(Notice::Toast {
                                text: format!("Last.fm: {message}"),
                                error: true,
                            });
                        }
                    }
                }
                Done::Scrobbled { count, result } => {
                    self.sending = false;
                    match result {
                        Ok(()) => {
                            self.pending.drain(..count.min(self.pending.len()));
                            self.save_queue();
                        }
                        Err(error) => {
                            log::warn!("scrobble: {error:?}");
                            self.next_send = Instant::now() + Duration::from_secs(60);
                            if self.signed_out(&error, notices) {
                                continue;
                            }
                            // Last.fm keeps nothing older than two weeks.
                            let cutoff = unix_now() - 14 * 24 * 3600;
                            self.pending.retain(|s| s.timestamp > cutoff);
                        }
                    }
                }
                Done::NowPlaying(result) => {
                    if let Err(error) = result {
                        log::info!("now playing: {error:?}");
                        self.signed_out(&error, notices);
                    }
                }
            }
        }
    }

    /// Code 9: the session was revoked (access removed on last.fm).
    fn signed_out(&mut self, error: &ApiError, notices: &mut Vec<Notice>) -> bool {
        if matches!(error, ApiError::Api(9, _)) {
            self.disconnect();
            notices.push(Notice::Toast {
                text: "Last.fm ended the app's access. Connect again in Settings to keep scrobbling.".into(),
                error: true,
            });
            return true;
        }
        false
    }
}

fn describe(error: &ApiError) -> String {
    match error {
        ApiError::Api(code, message) => format!("{message} ({code})"),
        ApiError::Network(message) => message.clone(),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Last.fm's request signature: every parameter but `format`, sorted by
/// name, joined as name and value, then the secret, hashed with MD5.
fn signature(params: &[(String, String)], secret: &str) -> String {
    let mut sorted: Vec<&(String, String)> = params.iter().filter(|(k, _)| k != "format").collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut text = String::new();
    for (k, v) in sorted {
        text.push_str(k);
        text.push_str(v);
    }
    text.push_str(secret);
    Md5::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

async fn call(
    http: &reqwest::Client,
    stored: &Stored,
    method: &str,
    mut params: Vec<(String, String)>,
    with_session: bool,
) -> Result<Value, ApiError> {
    params.push(("method".into(), method.into()));
    params.push(("api_key".into(), stored.key.clone()));
    if with_session {
        let Some(session) = &stored.session else {
            return Err(ApiError::Api(9, "not connected".into()));
        };
        params.push(("sk".into(), session.clone()));
    }
    let sig = signature(&params, &stored.secret);
    params.push(("api_sig".into(), sig));
    params.push(("format".into(), "json".into()));
    let response = http
        .post(API)
        .form(&params)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| ApiError::Network(e.without_url().to_string()))?;
    let value: Value = response
        .json()
        .await
        .map_err(|e| ApiError::Network(e.without_url().to_string()))?;
    if let Some(code) = value["error"].as_i64() {
        return Err(ApiError::Api(code, value["message"].as_str().unwrap_or("error").to_owned()));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_sorts_and_skips_format() {
        let params = vec![
            ("method".to_owned(), "auth.getToken".to_owned()),
            ("api_key".to_owned(), "key".to_owned()),
            ("format".to_owned(), "json".to_owned()),
        ];
        let expected: String = Md5::digest(b"api_keykeymethodauth.getTokensecret")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(signature(&params, "secret"), expected);
    }
}
