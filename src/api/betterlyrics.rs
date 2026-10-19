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

/// How high a song's lyrics from a source rank, lower first: in the order
/// the extension tries them, word by word before line by line, before text
/// alone. The app's own LRCLIB and YouTube Music's text take their places.
pub fn rank(lyrics: &Lyrics) -> u8 {
    let provider = lyrics.provider.as_deref().unwrap_or_default();
    let (word, line) = (lyrics.by_word(), !lyrics.lines.is_empty());
    match (provider, word, line) {
        ("Better Lyrics", true, _) => 0,
        ("Unison", true, _) => 1,
        ("BiniLyrics", true, _) => 2,
        ("QQ Music", true, _) => 4,
        ("Musixmatch", true, _) => 5,
        ("Better Lyrics", false, true) => 6,
        ("Unison", false, true) => 7,
        ("BiniLyrics", false, true) => 9,
        ("LRCLIB", false, true) => 10,
        ("KuGou", false, true) => 11,
        ("Musixmatch", false, true) => 12,
        (_, true, _) => 8,
        (_, false, true) => 12,
        ("YouTube Music", ..) => 13,
        ("Unison", ..) => 14,
        ("LRCLIB", ..) => 15,
        _ => 16,
    }
}

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

/// Every source's lyrics of the song. When none has any, once more by the
/// song's title, artist and length alone: an album named otherwise than
/// the providers name it can keep them from finding it.
pub async fn find(http: &reqwest::Client, query: &Query) -> Result<Vec<Lyrics>> {
    let found = find_as(http, query).await?;
    if !found.is_empty() || query.album.is_none() {
        return Ok(found);
    }
    let bare = Query { album: None, ..query.clone() };
    log::info!("Better Lyrics had none for {}; asking again without the album", query.title);
    find_as(http, &bare).await
}

async fn find_as(http: &reqwest::Client, query: &Query) -> Result<Vec<Lyrics>> {
    let (streamed, unison) = tokio::join!(stream(http, query), unison(http, query));
    let mut found = streamed?;
    match unison {
        Ok(more) => found.extend(more),
        Err(error) => log::info!("Unison: {error:#}"),
    }
    Ok(found)
}

/// The best of these lyrics first, the rest kept beside it to switch to;
/// the same source and timing twice is kept once (the first given).
pub fn gather(found: Vec<Lyrics>) -> Option<Lyrics> {
    let mut all: Vec<Lyrics> = Vec::with_capacity(found.len());
    for mut lyrics in found {
        let others = std::mem::take(&mut lyrics.others);
        all.push(lyrics);
        all.extend(others);
    }
    // Stable: of two that rank alike, the one given first leads.
    all.sort_by_key(rank);
    let mut kept: Vec<Lyrics> = Vec::new();
    for lyrics in all {
        let same = |k: &Lyrics| k.provider == lyrics.provider && k.timing() == lyrics.timing();
        if !kept.iter().any(same) {
            kept.push(lyrics);
        }
    }
    let mut kept = kept.into_iter();
    let mut best = kept.next()?;
    best.others = kept.take(10).collect();
    Some(best)
}

/// What was found before and what is found now, together: a source found
/// now replaces its own earlier lyrics, and sources not found this time are
/// kept. Never less than before.
pub fn merge(before: Option<&Lyrics>, now: Option<Lyrics>) -> Option<Lyrics> {
    let translated = before.map(|b| b.translated.clone()).unwrap_or_default();
    let mut all: Vec<Lyrics> = now.into_iter().collect();
    all.extend(before.cloned());
    let mut merged = gather(all)?;
    // Translations made for the lines stay with them.
    if merged.translated.is_empty() {
        merged.translated = translated;
    } else {
        for (line, said) in translated.lines {
            merged.translated.lines.entry(line).or_insert(said);
        }
    }
    Some(merged)
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
async fn stream(http: &reqwest::Client, query: &Query) -> Result<Vec<Lyrics>> {
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
/// stream says it is over.
fn event(block: &str, query: &Query, found: &mut Vec<Lyrics>) -> Result<bool> {
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
        // Told, and read on, as the extension does: the server ends the
        // stream itself when there is nothing more.
        "error" => log::info!("Better Lyrics: {}", data["message"].as_str().unwrap_or("an error")),
        "provider" => found.extend(provider(data["provider"].as_str().unwrap_or_default(), &data["results"], query)),
        _ => {}
    }
    Ok(false)
}

/// A provider's results as lyrics.
fn provider(name: &str, results: &Value, query: &Query) -> Vec<Lyrics> {
    let text = |key: &str| results[key].as_str().filter(|t| !t.trim().is_empty());
    // Some results are JSON inside JSON: `{"lyrics": "…"}`.
    let inner = |key: &str, field: &str| {
        let raw = text(key)?;
        match serde_json::from_str::<Value>(raw) {
            Ok(v) => v[field].as_str().map(str::to_owned),
            Err(_) => Some(raw.to_owned()),
        }
    };
    let credits = |lines| lyric_formats::drop_credits(lines, &query.title, &query.artist);
    let mut found = Vec::new();
    match name {
        // Better Lyrics' own: Apple Music's TTML.
        "golyrics" => {
            if let Some(ttml) = inner("lyrics", "ttml").as_deref().and_then(lyric_formats::ttml) {
                found.extend(timed("Better Lyrics", ttml.lines, ttml.songwriters));
            }
        }
        "binimum" => {
            if let Some(ttml) = text("lyrics").and_then(lyric_formats::ttml) {
                found.extend(timed("BiniLyrics", ttml.lines, ttml.songwriters));
            }
        }
        "qq" => {
            if let Some(qrc) = inner("lyrics", "lyrics") {
                found.extend(timed("QQ Music", credits(lyric_formats::qrc(&qrc)), Vec::new()));
            }
        }
        "musixmatch" => {
            if let Some(lrc) = text("wordByWord") {
                found.extend(timed("Musixmatch", lyric_formats::lrc(lrc), Vec::new()));
            }
            if let Some(lrc) = text("synced") {
                found.extend(timed("Musixmatch", lyric_formats::lrc(lrc), Vec::new()));
            }
        }
        "kugou" => {
            if let Some(lrc) = inner("lyrics", "lyrics") {
                found.extend(timed("KuGou", credits(lyric_formats::lrc(&lrc)), Vec::new()));
            }
        }
        "lrclib" => {
            if let Some(lrc) = text("synced") {
                found.extend(timed("LRCLIB", lyric_formats::lrc(lrc), Vec::new()));
            } else if let Some(plain) = text("plain") {
                found.push(untimed("LRCLIB", plain));
            }
        }
        _ => {}
    }
    found
}

/// Lyrics from timed lines (backing vocals written into a line taken out of
/// it); none when there are none worth showing.
pub fn timed(provider: &str, lines: Vec<LyricLine>, songwriters: Vec<String>) -> Option<Lyrics> {
    let lines: Vec<LyricLine> = lines
        .into_iter()
        .map(|mut line| {
            lyric_formats::split_backing(&mut line);
            line
        })
        .filter(|l| !l.text.trim().is_empty() || !l.backing.is_empty())
        .collect();
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
        ..Lyrics::default()
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
async fn unison(http: &reqwest::Client, query: &Query) -> Result<Vec<Lyrics>> {
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
    Ok(match data["format"].as_str().unwrap_or_default() {
        "ttml" => lyric_formats::ttml(text).and_then(|t| timed("Unison", t.lines, t.songwriters)),
        "lrc" => timed("Unison", lyric_formats::lrc(text), Vec::new()),
        _ => Some(untimed("Unison", text)),
    }
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
        // An error from the server is told, and the stream read on.
        assert!(!event("event: error\ndata: {\"message\":\"one provider failed\"}", &query(), &mut found).unwrap());
        let mxm = serde_json::json!({ "wordByWord": "[00:01.00] <00:01.00> one <00:01.50> <00:01.50> (two) <00:01.90>\n[00:02.00] <00:02.00> two <00:02.50>" });
        let block = format!("event: provider\ndata: {}", serde_json::json!({ "provider": "musixmatch", "results": mxm }));
        assert!(!event(&block, &query(), &mut found).unwrap());
        assert!(event("event: done\ndata: {}", &query(), &mut found).unwrap());
        let best = gather(found).expect("some");
        assert_eq!(best.provider.as_deref(), Some("Musixmatch"));
        assert!(best.by_word());
        // Backing vocals in brackets, under their line.
        assert_eq!(best.lines[0].text, "one");
        assert_eq!(best.lines[0].backing.len(), 1);
        assert_eq!(best.others.len(), 1);
        // KuGou's title line is a credit, not a lyric.
        assert_eq!(best.others[0].lines[0].text, "one");
    }

    fn lyrics(provider: &str, by_word: bool) -> Lyrics {
        let words = |t: f64| if by_word { vec![super::super::model::Word { start: t, end: t + 0.5, text: "la".into() }] } else { Vec::new() };
        Lyrics {
            text: "la\nla".into(),
            provider: Some(provider.into()),
            lines: vec![
                LyricLine { time: 1.0, text: "la".into(), words: words(1.0), ..LyricLine::default() },
                LyricLine { time: 2.0, text: "la".into(), words: words(2.0), ..LyricLine::default() },
            ],
            ..Lyrics::default()
        }
    }

    #[test]
    fn what_was_found_before_is_kept() {
        let mut before = gather(vec![lyrics("LRCLIB", false), lyrics("Musixmatch", true)]).unwrap();
        before.translated.lines.insert("la".into(), "the".into());
        // Found nothing this time: what was there stays.
        let kept = merge(Some(&before), None).unwrap();
        assert_eq!(kept.provider.as_deref(), Some("Musixmatch"));
        assert_eq!(kept.others.len(), 1);
        // Found something better: it leads, and the rest stay beside it.
        let kept = merge(Some(&before), Some(lyrics("Better Lyrics", true))).unwrap();
        assert_eq!(kept.provider.as_deref(), Some("Better Lyrics"));
        assert_eq!(kept.others.iter().filter_map(|l| l.provider.as_deref()).collect::<Vec<_>>(), ["Musixmatch", "LRCLIB"]);
        assert_eq!(kept.translated.lines.get("la").map(String::as_str), Some("the"));
        assert_eq!(merge(None, None), None);
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

    /// The live stream read by the app's own code (dev): `NATIVE_YTM_BL_DIR`
    /// is a profile's `lyrics-check` folder, with its key.
    #[test]
    #[ignore]
    fn read_a_live_stream() {
        let Ok(dir) = std::env::var("NATIVE_YTM_BL_DIR") else { return };
        init(PathBuf::from(dir));
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let http = reqwest::Client::builder().gzip(true).build().unwrap();
        let songs = [("4NRXx6U8ABQ", "Blinding Lights", "The Weeknd", None, 202), ("9cLEdCzvlIQ", "トリカゴ", "XX:me", Some("ダーリン・イン・ザ・フランキス エンディング集 vol.1"), 222)];
        for (id, title, artist, album, duration) in songs {
            let query = Query { video_id: id.into(), title: title.into(), artist: artist.into(), album: album.map(str::to_owned), duration: Some(duration) };
            let started = Instant::now();
            match runtime.block_on(find(&http, &query)) {
                Ok(found) => println!("{title}: {:?} in {:?}", found.iter().map(|l| (rank(l), l.provider.clone().unwrap_or_default())).collect::<Vec<_>>(), started.elapsed()),
                Err(error) => println!("{title}: error {error:#}"),
            }
        }
    }
}
