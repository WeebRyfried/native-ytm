//! Lyrics from every source Better Lyrics' browser extension uses. Its
//! server (run by Better Lyrics' contributors) asks the providers at once
//! and streams what each has: Better Lyrics' own (Apple Music's timings),
//! BiniLyrics, Musixmatch, QQ Music, KuGou and LRCLIB. Unison, its
//! community's lyrics, answers apart.
//!
//! The server wants a Cloudflare check passed first (src/lyrics_check.rs),
//! traded for a key that lasts a day, kept on disk.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use super::lyric_formats;
use super::model::{LyricLine, Lyrics};

const API: &str = "https://lyrics.api.dacubeking.com/";
const UNISON: &str = "https://unison.betterlyrics.org/lyrics";

/// A key with less than this left is renewed before it is used.
const RENEW_WITHIN_SECS: i64 = 60 * 60;
/// After a check that did not pass, how long before another is tried: its
/// window would come up again and again.
const CHECK_AGAIN_AFTER: Duration = Duration::from_secs(20 * 60);

/// What the song is, as YouTube Music has it: the server matches on it.
#[derive(Clone, Debug)]
pub struct Query {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Seconds.
    pub duration: Option<u32>,
}

/// One source's lyrics, and how high they rank: lower is better, in the
/// order the extension tries them (word by word before line by line,
/// before text alone).
#[derive(Clone, Debug)]
pub struct Found {
    pub rank: u8,
    pub lyrics: Lyrics,
}

/// Where Better Lyrics' plain LRCLIB and YouTube's text rank, for the
/// app's own LRCLIB lookup and YouTube's lyrics to take their places.
pub const LRCLIB_SYNCED: u8 = 10;
pub const YOUTUBE_TEXT: u8 = 13;
pub const LRCLIB_TEXT: u8 = 15;

struct State {
    dir: PathBuf,
    key: tokio::sync::Mutex<Key>,
}

#[derive(Default)]
struct Key {
    jwt: Option<String>,
    failed_at: Option<Instant>,
}

static STATE: OnceLock<State> = OnceLock::new();

/// Where the key and the check's WebView2 profile are kept.
pub fn init(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let jwt = std::fs::read_to_string(dir.join("key")).ok().map(|k| k.trim().to_owned()).filter(|k| seconds_left(k) > 60);
    let _ = STATE.set(State {
        dir,
        key: tokio::sync::Mutex::new(Key { jwt, failed_at: None }),
    });
}

/// Every source's lyrics of the song.
pub async fn find(http: &reqwest::Client, query: &Query) -> Result<Vec<Found>> {
    let (streamed, unison) = tokio::join!(stream(http, query), unison(http, query));
    let mut found = streamed?;
    match unison {
        Ok(more) => found.extend(more),
        Err(error) => log::info!("Unison: {error:#}"),
    }
    Ok(found)
}

/// The best of what was found, the rest kept beside it to switch to; the
/// same source and timing twice is kept once.
pub fn gather(mut found: Vec<Found>) -> Option<Lyrics> {
    found.sort_by_key(|f| f.rank);
    let mut kept: Vec<Lyrics> = Vec::new();
    for Found { lyrics, .. } in found {
        let same = |k: &Lyrics| k.provider == lyrics.provider && k.timing() == lyrics.timing();
        if !kept.iter().any(same) {
            kept.push(lyrics);
        }
    }
    let mut kept = kept.into_iter();
    let mut best = kept.next()?;
    best.others = kept.take(8).collect();
    Some(best)
}

async fn key(http: &reqwest::Client, renew: bool) -> Result<String> {
    let state = STATE.get().context("Better Lyrics is not set up")?;
    let mut key = state.key.lock().await;
    if !renew
        && let Some(jwt) = &key.jwt
        && seconds_left(jwt) > RENEW_WITHIN_SECS
    {
        return Ok(jwt.clone());
    }
    if key.failed_at.is_some_and(|at| at.elapsed() < CHECK_AGAIN_AFTER) {
        bail!("the lyrics check did not pass a moment ago");
    }
    match pass(http, state).await {
        Ok(jwt) => {
            crate::settings::write_atomic(&state.dir.join("key"), jwt.as_bytes());
            log::info!("Better Lyrics: a new key, for {} hours", seconds_left(&jwt) / 3600);
            key.jwt = Some(jwt.clone());
            key.failed_at = None;
            Ok(jwt)
        }
        Err(error) => {
            key.failed_at = Some(Instant::now());
            // A key not yet run out still does.
            match key.jwt.clone().filter(|k| !renew && seconds_left(k) > 60) {
                Some(jwt) => Ok(jwt),
                None => Err(error),
            }
        }
    }
}

/// The check passed, and its token traded for a key.
async fn pass(http: &reqwest::Client, state: &State) -> Result<String> {
    let token = crate::lyrics_check::obtain(&state.dir.join("webview")).await?;
    let response = http
        .post(format!("{API}verify-turnstile"))
        .json(&serde_json::json!({ "token": token }))
        .timeout(Duration::from_secs(20))
        .send()
        .await?
        .error_for_status()?;
    let answer: Value = response.json().await?;
    answer["jwt"].as_str().filter(|j| !j.is_empty()).map(str::to_owned).ok_or_else(|| anyhow!("no key in the answer"))
}

/// How long a key has left, from its own expiry; 0 when it cannot say.
fn seconds_left(jwt: &str) -> i64 {
    let Some(payload) = jwt.split('.').nth(1).and_then(base64url) else {
        return 0;
    };
    let exp = serde_json::from_slice::<Value>(&payload).ok().and_then(|v| v["exp"].as_i64()).unwrap_or(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    exp - now
}

fn base64url(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut bits, mut count) = (0u32, 0);
    for c in text.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    Some(out)
}

/// The server's stream: one event per provider, as each answers.
async fn stream(http: &reqwest::Client, query: &Query) -> Result<Vec<Found>> {
    let mut renewed = false;
    let mut response = loop {
        let jwt = key(http, renewed).await?;
        let mut form = vec![
            ("videoId", query.video_id.clone()),
            ("song", query.title.clone()),
            ("artist", query.artist.clone()),
            ("alwaysFetchMetadata", "false".to_owned()),
            ("token", jwt),
        ];
        if let Some(duration) = query.duration {
            form.push(("duration", duration.to_string()));
        }
        if let Some(album) = &query.album {
            form.push(("album", album.clone()));
        }
        let response = http.post(format!("{API}v2/lyrics")).form(&form).timeout(Duration::from_secs(30)).send().await?;
        // Turned away: the key is no good any more. Once more, with a new one.
        if response.status() == reqwest::StatusCode::FORBIDDEN && !renewed {
            renewed = true;
            continue;
        }
        break response.error_for_status()?;
    };
    let mut found = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let chunk = response.chunk().await?;
        let done = chunk.is_none();
        if let Some(chunk) = chunk {
            pending.extend_from_slice(&chunk);
        }
        // Each event ends with a blank line; the last may not.
        while let Some(at) = blank_line(&pending).or_else(|| done.then_some(pending.len()).filter(|&n| n > 0)) {
            let block: Vec<u8> = pending.drain(..at).collect();
            let skip = pending.iter().take_while(|b| **b == b'\n' || **b == b'\r').count();
            pending.drain(..skip);
            match event(&String::from_utf8_lossy(&block), query, &mut found) {
                Ok(true) => return Ok(found),
                Ok(false) => {}
                Err(error) => log::info!("Better Lyrics: {error:#}"),
            }
        }
        if done {
            return Ok(found);
        }
    }
}

fn blank_line(bytes: &[u8]) -> Option<usize> {
    let lf = bytes.windows(2).position(|w| w == b"\n\n");
    let crlf = bytes.windows(4).position(|w| w == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// One event of the stream, its lyrics added to `found`. True once the
/// stream is over.
fn event(block: &str, query: &Query, found: &mut Vec<Found>) -> Result<bool> {
    let (mut name, mut data) = ("", String::new());
    for line in block.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            name = value.trim();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.trim());
        }
    }
    if data.is_empty() {
        return Ok(false);
    }
    if data == "[DONE]" || name == "done" {
        return Ok(true);
    }
    let data: Value = serde_json::from_str(&data)?;
    match name {
        "error" => {
            // "Invalid Video Id" and the like: nothing to be had.
            log::info!("Better Lyrics: {}", data["message"].as_str().unwrap_or("an error"));
            Ok(true)
        }
        "provider" => {
            found.extend(provider(data["provider"].as_str().unwrap_or_default(), &data["results"], query));
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// A provider's results as lyrics, ranked.
fn provider(name: &str, results: &Value, query: &Query) -> Vec<Found> {
    let text = |key: &str| results[key].as_str().filter(|t| !t.trim().is_empty());
    // Some results are JSON inside JSON: `{"lyrics": "…"}`.
    let inner = |key: &str, field: &str| {
        let raw = text(key)?;
        match serde_json::from_str::<Value>(raw) {
            Ok(v) => v[field].as_str().map(str::to_owned),
            Err(_) => Some(raw.to_owned()),
        }
    };
    let mut found = Vec::new();
    // Ranked by how they turn out to be timed.
    let mut add = |rank_word: u8, rank_line: u8, provider: &str, lines: Vec<LyricLine>, songwriters: Vec<String>| {
        if let Some(lyrics) = timed(provider, lines, songwriters) {
            found.push(Found { rank: if lyrics.by_word() { rank_word } else { rank_line }, lyrics });
        }
    };
    let mut add_text = Vec::new();
    match name {
        // Better Lyrics' own: Apple Music's TTML.
        "golyrics" => {
            if let Some(ttml) = inner("lyrics", "ttml").as_deref().and_then(lyric_formats::ttml) {
                add(0, 6, "Better Lyrics", ttml.lines, ttml.songwriters);
            }
        }
        "binimum" => {
            if let Some(ttml) = text("lyrics").and_then(lyric_formats::ttml) {
                add(2, 9, "BiniLyrics", ttml.lines, ttml.songwriters);
            }
        }
        "qq" => {
            if let Some(qrc) = inner("lyrics", "lyrics") {
                add(4, 11, "QQ Music", lyric_formats::drop_credits(lyric_formats::qrc(&qrc), &query.title, &query.artist), Vec::new());
            }
        }
        "musixmatch" => {
            if let Some(lrc) = text("wordByWord") {
                add(5, 12, "Musixmatch", lyric_formats::lrc(lrc), Vec::new());
            }
            if let Some(lrc) = text("synced") {
                add(12, 12, "Musixmatch", lyric_formats::lrc(lrc), Vec::new());
            }
        }
        "kugou" => {
            if let Some(lrc) = inner("lyrics", "lyrics") {
                add(11, 11, "KuGou", lyric_formats::drop_credits(lyric_formats::lrc(&lrc), &query.title, &query.artist), Vec::new());
            }
        }
        "lrclib" => {
            if let Some(lrc) = text("synced") {
                add(LRCLIB_SYNCED, LRCLIB_SYNCED, "LRCLIB", lyric_formats::lrc(lrc), Vec::new());
            } else if let Some(plain) = text("plain") {
                add_text.push(Found { rank: LRCLIB_TEXT, lyrics: untimed("LRCLIB", plain) });
            }
        }
        _ => {}
    }
    found.extend(add_text);
    found
}

/// Lyrics from timed lines; none when there are none worth showing.
fn timed(provider: &str, lines: Vec<LyricLine>, songwriters: Vec<String>) -> Option<Lyrics> {
    let lines: Vec<LyricLine> = lines.into_iter().filter(|l| !l.text.trim().is_empty() || !l.backing.is_empty()).collect();
    if lines.len() < 2 {
        return None;
    }
    let text = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    Some(Lyrics {
        text,
        source: Some(said_from(provider)),
        lines,
        provider: Some(provider.to_owned()),
        songwriters,
        others: Vec::new(),
    })
}

fn untimed(provider: &str, text: &str) -> Lyrics {
    Lyrics {
        text: text.trim().to_owned(),
        source: Some(said_from(provider)),
        provider: Some(provider.to_owned()),
        ..Lyrics::default()
    }
}

fn said_from(provider: &str) -> String {
    match provider {
        "Better Lyrics" => "Lyrics from Better Lyrics".to_owned(),
        "Unison" => "Lyrics from Unison, Better Lyrics' community".to_owned(),
        other => format!("Lyrics from {other}, through Better Lyrics"),
    }
}

/// Unison, Better Lyrics' community lyrics: TTML, LRC (perhaps timed by the
/// word) or text.
async fn unison(http: &reqwest::Client, query: &Query) -> Result<Vec<Found>> {
    let mut params = vec![("v", query.video_id.clone()), ("song", query.title.clone()), ("artist", query.artist.clone())];
    params.push(("duration", query.duration.unwrap_or(0).to_string()));
    if let Some(album) = &query.album {
        params.push(("album", album.clone()));
    }
    let response = http.get(UNISON).query(&params).timeout(Duration::from_secs(10)).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    let answer: Value = response.error_for_status()?.json().await?;
    let data = &answer["data"];
    let Some(text) = data["lyrics"].as_str().filter(|t| !t.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let found = match data["format"].as_str().unwrap_or_default() {
        "ttml" => lyric_formats::ttml(text).and_then(|t| timed("Unison", t.lines, t.songwriters)),
        "lrc" => timed("Unison", lyric_formats::lrc(text), Vec::new()),
        _ => return Ok(vec![Found { rank: 14, lyrics: untimed("Unison", text) }]),
    };
    Ok(found
        .map(|lyrics| {
            let word_rank = if data["format"] == "ttml" { 1 } else { 3 };
            Found { rank: if lyrics.by_word() { word_rank } else { 7 }, lyrics }
        })
        .into_iter()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> Query {
        Query { video_id: "v".into(), title: "アイドル".into(), artist: "YOASOBI".into(), album: None, duration: Some(213) }
    }

    #[test]
    fn a_stream_of_providers_ranked() {
        let mut found = Vec::new();
        let kugou = serde_json::json!({ "lyrics": serde_json::json!({ "lyrics": "[00:00.00]アイドル - YOASOBI\n[00:01.00]one\n[00:02.00]two" }).to_string() });
        let block = format!("event: provider\ndata: {}", serde_json::json!({ "provider": "kugou", "results": kugou }));
        assert!(!event(&block, &query(), &mut found).unwrap());
        let mxm = serde_json::json!({ "wordByWord": "[00:01.00] <00:01.00> one <00:01.50>\n[00:02.00] <00:02.00> two <00:02.50>" });
        let block = format!("event: provider\ndata: {}", serde_json::json!({ "provider": "musixmatch", "results": mxm }));
        assert!(!event(&block, &query(), &mut found).unwrap());
        assert!(event("event: done\ndata: {}", &query(), &mut found).unwrap());
        let best = gather(found).expect("some");
        assert_eq!(best.provider.as_deref(), Some("Musixmatch"));
        assert!(best.by_word());
        assert_eq!(best.others.len(), 1);
        // KuGou's title line is a credit, not a lyric.
        assert_eq!(best.others[0].lines[0].text, "one");
    }

    #[test]
    fn a_key_says_how_long_it_lasts() {
        let exp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 7200;
        let payload = format!("{{\"exp\":{exp}}}");
        let encoded: String = {
            const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let bytes = payload.as_bytes();
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
                for i in 0..=chunk.len() {
                    out.push(ABC[((n >> (18 - 6 * i)) & 63) as usize] as char);
                }
            }
            out
        };
        let left = seconds_left(&format!("x.{encoded}.y"));
        assert!((7100..=7200).contains(&left), "{left}");
        assert_eq!(seconds_left("nonsense"), 0);
    }
}
