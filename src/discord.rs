//! Discord Rich Presence, over the pipe the Discord app listens on: the
//! status reads "Listening to <artist>", and the profile card shows the
//! artwork, the title, the artist and a progress bar.
//!
//! A thread of its own owns the pipe: it connects when Discord runs, sends
//! what the app last asked for (no more often than Discord allows), and
//! reconnects when Discord restarts. Nothing here ever waits on the
//! interface, or the interface on it.

use std::fs::File;
use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The "YouTube Music" application of th-ch/youtube-music, an open-source
/// client, whose name Discord shows. Settings take another.
pub const DEFAULT_CLIENT_ID: &str = "1177081335727267940";

/// What is playing, as Discord is told it.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub art_url: Option<String>,
    pub url: String,
    pub artist_url: Option<String>,
    /// When the song started, in Unix milliseconds, given where it is now.
    pub start_ms: i64,
    pub end_ms: Option<i64>,
}

impl Activity {
    /// The same song at about the same place: no need to tell Discord again.
    fn same(&self, other: &Activity) -> bool {
        self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.art_url == other.art_url
            && self.url == other.url
            && (self.start_ms - other.start_ms).abs() < 1500
            && self.end_ms.is_some() == other.end_ms.is_some()
    }
}

enum Message {
    Set(Option<Activity>),
    Stop,
}

pub struct Presence {
    tx: Sender<Message>,
    last: Option<Activity>,
    pub client_id: String,
}

impl Presence {
    pub fn spawn(client_id: &str) -> Self {
        let (tx, rx) = channel();
        let id = client_id.to_owned();
        let _ = std::thread::Builder::new()
            .name("discord".into())
            .spawn(move || run(&id, &rx));
        Self {
            tx,
            last: None,
            client_id: client_id.to_owned(),
        }
    }

    /// What should show; `None` clears it. Cheap to call every frame.
    pub fn set(&mut self, activity: Option<Activity>) {
        let changed = match (&self.last, &activity) {
            (Some(a), Some(b)) => !a.same(b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.last.clone_from(&activity);
            let _ = self.tx.send(Message::Set(activity));
        }
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        let _ = self.tx.send(Message::Stop);
    }
}

/// Discord allows five activity updates in twenty seconds.
const MIN_GAP: Duration = Duration::from_secs(4);
const RETRY: Duration = Duration::from_secs(15);

fn run(client_id: &str, rx: &Receiver<Message>) {
    let mut pipe: Option<File> = None;
    let mut wanted: Option<Activity> = None;
    // What Discord shows, when known: nothing, to begin with.
    let mut sent: Option<Option<Activity>> = Some(None);
    let mut last_send = Instant::now() - MIN_GAP;
    let mut next_connect = Instant::now();
    let mut nonce = 0u64;
    loop {
        let pending = sent.as_ref() != Some(&wanted);
        let mut wait = if pending {
            MIN_GAP.saturating_sub(last_send.elapsed())
        } else {
            Duration::from_secs(30)
        };
        if pending && pipe.is_none() {
            wait = wait.max(next_connect.saturating_duration_since(Instant::now()));
        }
        let wait = wait.max(Duration::from_millis(50));
        match rx.recv_timeout(wait) {
            Ok(Message::Set(activity)) => {
                wanted = activity;
                // Take the latest of a burst.
                while let Ok(message) = rx.try_recv() {
                    match message {
                        Message::Set(activity) => wanted = activity,
                        Message::Stop => return clear(pipe.as_mut(), &mut nonce),
                    }
                }
            }
            Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return clear(pipe.as_mut(), &mut nonce),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if sent.as_ref() == Some(&wanted) || last_send.elapsed() < MIN_GAP {
            continue;
        }
        if pipe.is_none() {
            if Instant::now() < next_connect {
                continue;
            }
            match connect(client_id) {
                Ok(file) => {
                    log::info!("connected to Discord");
                    pipe = Some(file);
                }
                Err(error) => {
                    log::debug!("Discord: {error}");
                    next_connect = Instant::now() + RETRY;
                    continue;
                }
            }
        }
        let Some(file) = pipe.as_mut() else { continue };
        last_send = Instant::now();
        nonce += 1;
        match set_activity(file, wanted.as_ref(), nonce) {
            Ok(()) => sent = Some(wanted.clone()),
            Err(error) => {
                log::info!("Discord connection lost: {error}");
                pipe = None;
                // Discord drops an activity whose connection closes.
                sent = Some(None);
                next_connect = Instant::now() + Duration::from_secs(2);
            }
        }
    }
}

fn clear(pipe: Option<&mut File>, nonce: &mut u64) {
    if let Some(file) = pipe {
        *nonce += 1;
        let _ = set_activity(file, None, *nonce);
    }
}

fn connect(client_id: &str) -> std::io::Result<File> {
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "Discord is not running");
    for n in 0..10 {
        let path = format!(r"\\.\pipe\discord-ipc-{n}");
        match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(mut file) => {
                write_frame(&mut file, 0, &json!({ "v": 1, "client_id": client_id }))?;
                let (op, reply) = read_frame(&mut file)?;
                if op != 1 || reply["evt"] != "READY" {
                    last = std::io::Error::other(format!("handshake refused: {}", reply["message"]));
                    continue;
                }
                return Ok(file);
            }
            Err(error) => last = error,
        }
    }
    Err(last)
}

fn set_activity(file: &mut File, activity: Option<&Activity>, nonce: u64) -> std::io::Result<()> {
    let payload = json!({
        "cmd": "SET_ACTIVITY",
        "args": {
            "pid": std::process::id(),
            "activity": activity.map(activity_json),
        },
        "nonce": nonce.to_string(),
    });
    write_frame(file, 1, &payload)?;
    // Discord answers every command; a ping may come first.
    loop {
        let (op, reply) = read_frame(file)?;
        match op {
            3 => write_frame(file, 4, &reply)?,
            2 => return Err(std::io::Error::other("Discord closed the connection")),
            _ => {
                if reply["evt"] == "ERROR" {
                    log::warn!("Discord refused the activity: {}", reply["data"]["message"]);
                }
                return Ok(());
            }
        }
    }
}

fn activity_json(activity: &Activity) -> Value {
    let mut timestamps = json!({ "start": activity.start_ms });
    if let Some(end) = activity.end_ms {
        timestamps["end"] = json!(end);
    }
    let mut assets = json!({});
    if let Some(art) = &activity.art_url {
        assets["large_image"] = json!(art);
        assets["large_url"] = json!(activity.url);
    }
    if let Some(album) = &activity.album {
        assets["large_text"] = json!(text(album));
    }
    let mut value = json!({
        // Listening: the status names the artist, and a start and an end
        // make the card's progress bar.
        "type": 2,
        "status_display_type": 1,
        "details": text(&activity.title),
        "details_url": activity.url,
        "state": text(&activity.artist),
        "timestamps": timestamps,
        "assets": assets,
        "buttons": [{ "label": "Play on YouTube Music", "url": activity.url }],
    });
    if let Some(url) = &activity.artist_url {
        value["state_url"] = json!(url);
    }
    value
}

/// Discord takes 2 to 128 characters.
fn text(s: &str) -> String {
    let mut out: String = s.trim().chars().take(128).collect();
    while out.chars().count() < 2 {
        out.push('\u{2800}');
    }
    out
}

fn write_frame(file: &mut File, op: u32, payload: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(payload)?;
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    file.write_all(&frame)?;
    file.flush()
}

fn read_frame(file: &mut File) -> std::io::Result<(u32, Value)> {
    let mut header = [0u8; 8];
    file.read_exact(&mut header)?;
    let op = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if len > 1 << 20 {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut body = vec![0u8; len];
    file.read_exact(&mut body)?;
    Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_padded() {
        assert_eq!(text("A").chars().count(), 2);
        assert_eq!(text(&"x".repeat(300)).chars().count(), 128);
    }

    #[test]
    fn a_little_drift_is_the_same() {
        let a = Activity {
            title: "Song".into(),
            artist: "Artist".into(),
            album: None,
            art_url: None,
            url: "https://music.youtube.com/watch?v=x".into(),
            artist_url: None,
            start_ms: 1_000_000,
            end_ms: Some(1_200_000),
        };
        let mut b = a.clone();
        b.start_ms += 400;
        assert!(a.same(&b));
        b.start_ms += 5000;
        assert!(!a.same(&b));
    }
}
