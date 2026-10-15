//! Last.fm scrobbling, under the person's own Last.fm API account: its key
//! and shared secret come from last.fm/api/account/create, and the account
//! is authorised in the browser (Last.fm's desktop flow), so no password
//! passes through the app.
//!
//! A song is scrobbled once it has been heard for half its length or four
//! minutes, whichever comes first, as Last.fm asks; songs under 30 seconds
//! are not. Scrobbles wait in a file while Last.fm cannot be reached.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
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

/// How many times the person has played a song, and its artist, as
/// Last.fm counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Plays {
    pub track: Option<u64>,
    pub artist: Option<u64>,
}

/// An artist's numbers on Last.fm: the person's plays of them, and
/// everyone's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArtistStats {
    pub plays: Option<u64>,
    pub listeners: Option<u64>,
    pub scrobbles: Option<u64>,
}

/// A song's key for its counts: artist and title, as Last.fm matches them.
type PlaysKey = (String, String);

fn plays_key(artist: &str, title: &str) -> PlaysKey {
    (artist.trim().to_lowercase(), title.trim().to_lowercase())
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
    Plays { key: PlaysKey, plays: Plays },
    Artist { key: String, stats: Option<ArtistStats> },
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
    /// Runs when an answer arrives, so the window shows it.
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Play counts by song, and those being asked for.
    plays: HashMap<PlaysKey, Plays>,
    asking: HashSet<PlaysKey>,
    /// Artists' numbers, by name in lower case (None: Last.fm knows them
    /// not), and those being asked for.
    artists: HashMap<String, Option<ArtistStats>>,
    asking_artists: HashSet<String>,
}

impl LastFm {
    pub fn new(
        config: &std::path::Path,
        runtime: tokio::runtime::Handle,
        http: reqwest::Client,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
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
            wake,
            plays: HashMap::new(),
            asking: HashSet::new(),
            artists: HashMap::new(),
            asking_artists: HashSet::new(),
        }
    }

    /// Sends an answer back to the interface thread, and wakes it.
    fn sender(&self) -> impl Fn(Done) + Send + 'static {
        let (tx, wake) = (self.tx.clone(), self.wake.clone());
        move |done| {
            let _ = tx.send(done);
            wake();
        }
    }

    /// How often the person has played this song and its artist, once
    /// Last.fm has said; asks the first time. None when not connected.
    pub fn plays(&mut self, artist: &str, title: &str) -> Option<Plays> {
        let user = self.stored.user.clone().filter(|_| self.stored.session.is_some())?;
        let key = plays_key(artist, title);
        if let Some(plays) = self.plays.get(&key) {
            return Some(*plays);
        }
        if self.asking.insert(key.clone()) {
            if self.plays.len() > 500 {
                self.plays.clear();
            }
            let (http, stored, send) = (self.http.clone(), self.stored.clone(), self.sender());
            let (artist, title) = (artist.to_owned(), title.to_owned());
            self.runtime.spawn(async move {
                let track_params = [("artist", artist.as_str()), ("track", title.as_str()), ("username", user.as_str())];
                let artist_params = [("artist", artist.as_str()), ("username", user.as_str())];
                let (track, by) = tokio::join!(
                    read(&http, &stored, "track.getInfo", &track_params),
                    read(&http, &stored, "artist.getInfo", &artist_params)
                );
                let plays = Plays {
                    track: track.ok().and_then(|v| count(&v["track"]["userplaycount"])),
                    artist: by.ok().and_then(|v| count(&v["artist"]["stats"]["userplaycount"])),
                };
                send(Done::Plays { key, plays });
            });
        }
        None
    }

    /// An artist's numbers, the person's plays among them, once Last.fm has
    /// said; asks the first time. None when not connected, or not known.
    pub fn artist_stats(&mut self, artist: &str) -> Option<ArtistStats> {
        let user = self.stored.user.clone().filter(|_| self.stored.session.is_some())?;
        let key = artist.trim().to_lowercase();
        if let Some(stats) = self.artists.get(&key) {
            return *stats;
        }
        if self.asking_artists.insert(key.clone()) {
            if self.artists.len() > 300 {
                self.artists.clear();
            }
            let (http, stored, send) = (self.http.clone(), self.stored.clone(), self.sender());
            let artist = artist.to_owned();
            self.runtime.spawn(async move {
                let params = [("artist", artist.as_str()), ("username", user.as_str())];
                let stats = read(&http, &stored, "artist.getInfo", &params).await.ok().map(|v| {
                    let stats = &v["artist"]["stats"];
                    ArtistStats {
                        plays: count(&stats["userplaycount"]),
                        listeners: count(&stats["listeners"]),
                        scrobbles: count(&stats["playcount"]),
                    }
                });
                send(Done::Artist { key, stats });
            });
        }
        None
    }

    /// What reads Last.fm with the person's API account (no session
    /// needed): artists' pages, artwork. None without an account.
    pub fn reader(&self) -> Option<Reader> {
        (!self.stored.key.trim().is_empty()).then(|| Reader {
            key: self.stored.key.clone(),
            user: self.stored.user.clone().filter(|_| self.stored.session.is_some()),
        })
    }

    /// Forgets counts Last.fm never gave (asked while offline), so they
    /// are asked again.
    pub fn forget_unanswered(&mut self) {
        self.plays.retain(|_, plays| plays.track.is_some() || plays.artist.is_some());
        self.artists.retain(|_, stats| stats.is_some());
    }

    /// The song's page on Last.fm.
    pub fn track_url(artist: &str, title: &str) -> String {
        format!("https://www.last.fm/music/{}/_/{}", path_part(artist), path_part(title))
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
        let (http, stored, send) = (self.http.clone(), self.stored.clone(), self.sender());
        self.runtime.spawn(async move {
            let result = call(&http, &stored, "auth.getToken", Vec::new(), false)
                .await
                .map_err(|e| describe(&e))
                .and_then(|v| v["token"].as_str().map(str::to_owned).ok_or_else(|| "no token".to_owned()));
            send(Done::Token(result));
        });
    }

    pub fn disconnect(&mut self) {
        self.stored.session = None;
        self.stored.user = None;
        self.plays.clear();
        self.artists.clear();
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
                    // As the web player names it, so a play here counts as
                    // the same song as one scrobbled from there.
                    track: current.track.romanized_title().to_owned(),
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
            // One more play of each, shown at once.
            if let Some(plays) = self.plays.get_mut(&plays_key(&play.scrobble.artist, &play.scrobble.track)) {
                plays.track = plays.track.map(|n| n + 1);
                plays.artist = plays.artist.map(|n| n + 1);
            }
            if let Some(Some(stats)) = self.artists.get_mut(&play.scrobble.artist.trim().to_lowercase()) {
                stats.plays = Some(stats.plays.unwrap_or(0) + 1);
            }
            self.pending.push(play.scrobble.clone());
            self.save_queue();
            self.next_send = Instant::now();
        }
    }

    fn now_playing(&self, scrobble: Scrobble) {
        let (http, stored, send) = (self.http.clone(), self.stored.clone(), self.sender());
        self.runtime.spawn(async move {
            let mut params = vec![("artist".to_owned(), scrobble.artist), ("track".to_owned(), scrobble.track)];
            if let Some(album) = scrobble.album {
                params.push(("album".into(), album));
            }
            if let Some(duration) = scrobble.duration {
                params.push(("duration".into(), duration.to_string()));
            }
            let result = call(&http, &stored, "track.updateNowPlaying", params, true).await.map(|_| ());
            send(Done::NowPlaying(result));
        });
    }

    fn send_batch(&mut self) {
        self.sending = true;
        let batch: Vec<Scrobble> = self.pending.iter().take(50).cloned().collect();
        let count = batch.len();
        let (http, stored, send) = (self.http.clone(), self.stored.clone(), self.sender());
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
            send(Done::Scrobbled { count, result });
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
        let (http, stored, send) = (self.http.clone(), self.stored.clone(), self.sender());
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
            send(Done::Session(result));
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
                Done::Plays { key, plays } => {
                    self.asking.remove(&key);
                    self.plays.insert(key, plays);
                }
                Done::Artist { key, stats } => {
                    self.asking_artists.remove(&key);
                    self.artists.insert(key, stats);
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

/// What Last.fm knows of an artist, for their page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArtistInfo {
    /// As Last.fm spells it.
    pub name: String,
    pub url: String,
    pub listeners: Option<u64>,
    pub scrobbles: Option<u64>,
    /// The person's own plays, when connected.
    pub plays: Option<u64>,
    /// A paragraph or two, as plain text.
    pub bio: String,
    pub tags: Vec<String>,
    pub similar: Vec<String>,
    /// Their most played songs: title and listeners.
    pub top_tracks: Vec<(String, Option<u64>)>,
    /// Their most played albums: title and cover.
    pub albums: Vec<(String, Option<String>)>,
}

/// Read-only access with the person's API key, for use on any thread.
#[derive(Clone)]
pub struct Reader {
    key: String,
    /// Their Last.fm name, for their own play counts.
    user: Option<String>,
}

impl Reader {
    async fn get(&self, http: &reqwest::Client, method: &str, params: &[(&str, &str)]) -> Result<Value, ApiError> {
        let stored = Stored { key: self.key.clone(), ..Stored::default() };
        read(http, &stored, method, params).await
    }

    /// An artist's numbers, story, tags, songs, albums and likenesses.
    pub async fn artist(&self, http: &reqwest::Client, name: &str) -> Result<ArtistInfo, String> {
        let mut info_params = vec![("artist", name)];
        if let Some(user) = &self.user {
            info_params.push(("username", user.as_str()));
        }
        let limit = [("artist", name), ("limit", "10")];
        let albums_limit = [("artist", name), ("limit", "12")];
        let (info, tracks, albums) = tokio::join!(
            self.get(http, "artist.getInfo", &info_params),
            self.get(http, "artist.getTopTracks", &limit),
            self.get(http, "artist.getTopAlbums", &albums_limit),
        );
        let info = info.map_err(|e| describe(&e))?;
        let artist = &info["artist"];
        let list = |v: &Value| -> Vec<Value> {
            match v {
                Value::Array(items) => items.clone(),
                Value::Object(_) => vec![v.clone()],
                _ => Vec::new(),
            }
        };
        let text = |v: &Value| v.as_str().unwrap_or_default().trim().to_owned();
        Ok(ArtistInfo {
            name: text(&artist["name"]),
            url: text(&artist["url"]),
            listeners: count(&artist["stats"]["listeners"]),
            scrobbles: count(&artist["stats"]["playcount"]),
            plays: count(&artist["stats"]["userplaycount"]),
            bio: plain_bio(artist["bio"]["summary"].as_str().unwrap_or_default()),
            tags: list(&artist["tags"]["tag"]).iter().map(|t| text(&t["name"])).filter(|t| !t.is_empty()).take(6).collect(),
            similar: list(&artist["similar"]["artist"]).iter().map(|a| text(&a["name"])).filter(|a| !a.is_empty()).collect(),
            top_tracks: tracks
                .map(|v| list(&v["toptracks"]["track"]))
                .unwrap_or_default()
                .iter()
                .map(|t| (text(&t["name"]), count(&t["listeners"])))
                .filter(|(t, _)| !t.is_empty())
                .collect(),
            albums: albums
                .map(|v| list(&v["topalbums"]["album"]))
                .unwrap_or_default()
                .iter()
                .map(|a| (text(&a["name"]), image(&a["image"])))
                .filter(|(a, _)| !a.is_empty() && a != "(null)")
                .collect(),
        })
    }

    /// Artists like this one, the likest first.
    pub async fn similar(&self, http: &reqwest::Client, name: &str) -> Vec<String> {
        let Ok(value) = self.get(http, "artist.getSimilar", &[("artist", name), ("limit", "6")]).await else {
            return Vec::new();
        };
        value["similarartists"]["artist"]
            .as_array()
            .map(|artists| artists.iter().filter_map(|a| a["name"].as_str()).map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// An album's cover, as Last.fm has it.
    pub async fn album_art(&self, http: &reqwest::Client, artist: &str, album: &str) -> Option<String> {
        let value = self.get(http, "album.getInfo", &[("artist", artist), ("album", album)]).await.ok()?;
        image(&value["album"]["image"])
    }

    /// A song's cover (its album's), as Last.fm has it.
    pub async fn track_art(&self, http: &reqwest::Client, artist: &str, title: &str) -> Option<String> {
        let value = self.get(http, "track.getInfo", &[("artist", artist), ("track", title)]).await.ok()?;
        image(&value["track"]["album"]["image"])
    }
}

/// The largest of Last.fm's sizes of a picture, unless it is the grey star
/// Last.fm shows for a picture it has not got.
fn image(sizes: &Value) -> Option<String> {
    sizes
        .as_array()?
        .iter()
        .filter_map(|i| i["#text"].as_str())
        .filter(|url| !url.is_empty() && !url.contains("2a96cbd8b46e442fc41c2b86b821562f"))
        .last()
        .map(str::to_owned)
}

/// Last.fm's summary without its markup and its "Read more" link.
fn plain_bio(html: &str) -> String {
    let cut = html.find("<a href=\"https://www.last.fm").unwrap_or(html.len());
    let mut out = String::new();
    let mut tag = false;
    for c in html[..cut].chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_owned()
}

/// A read-only method: asked with the API key alone, unsigned.
async fn read(http: &reqwest::Client, stored: &Stored, method: &str, params: &[(&str, &str)]) -> Result<Value, ApiError> {
    let mut query: Vec<(&str, &str)> = vec![("method", method), ("api_key", &stored.key), ("autocorrect", "1"), ("format", "json")];
    query.extend_from_slice(params);
    let response = http
        .get(API)
        .query(&query)
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

/// A count, which Last.fm sends as a string or a number.
fn count(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

/// A name as Last.fm puts it in its addresses: spaces as +, the rest
/// percent-encoded.
fn path_part(name: &str) -> String {
    let mut out = String::new();
    for byte in name.trim().bytes() {
        match byte {
            b' ' => out.push('+'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
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
    fn counts_and_addresses() {
        assert_eq!(count(&serde_json::json!("1248")), Some(1248));
        assert_eq!(count(&serde_json::json!(87)), Some(87));
        assert_eq!(count(&serde_json::json!(null)), None);
        assert_eq!(
            LastFm::track_url("Mojo", "Andai Ku Bercinta lagi"),
            "https://www.last.fm/music/Mojo/_/Andai+Ku+Bercinta+lagi"
        );
        assert_eq!(path_part("AC/DC & Me"), "AC%2FDC+%26+Me");
    }

    #[test]
    fn pictures_and_stories() {
        let sizes = serde_json::json!([
            { "#text": "https://lastfm.freetls.fastly.net/i/u/34s/a.png", "size": "small" },
            { "#text": "https://lastfm.freetls.fastly.net/i/u/300x300/a.png", "size": "extralarge" },
            { "#text": "", "size": "mega" }
        ]);
        assert_eq!(image(&sizes).as_deref(), Some("https://lastfm.freetls.fastly.net/i/u/300x300/a.png"));
        let star = serde_json::json!([{ "#text": "https://lastfm.freetls.fastly.net/i/u/300x300/2a96cbd8b46e442fc41c2b86b821562f.png" }]);
        assert_eq!(image(&star), None);
        assert_eq!(
            plain_bio("XX:me is a unit &amp; more. <a href=\"https://www.last.fm/music/XX:me\">Read more on Last.fm</a>"),
            "XX:me is a unit & more."
        );
    }

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
