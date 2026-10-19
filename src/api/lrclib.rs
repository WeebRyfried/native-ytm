//! Synced lyrics from LRCLIB (lrclib.net), an open lyrics database with a
//! free API. YouTube Music keeps its own line timings to its mobile apps, so
//! this is where timed lines come from; YouTube's plain lyrics remain the
//! fallback.

use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

use super::model::{LyricLine, Lyrics};

const API: &str = "https://lrclib.net/api";

/// LRCLIB turns away requests that come too close together: every lookup
/// asks it one request at a time, this far apart.
const SPACING: Duration = Duration::from_millis(350);

/// When LRCLIB was last asked; held while a request is out.
static GATE: std::sync::LazyLock<tokio::sync::Mutex<Option<std::time::Instant>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(None));
/// LRCLIB asks clients to name themselves.
const USER_AGENT: &str = concat!("native-ytm/", env!("CARGO_PKG_VERSION"));

/// What a song is called, to look it up.
#[derive(Clone, Debug)]
pub struct Query {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Seconds.
    pub duration: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    artist_name: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default)]
    plain_lyrics: Option<String>,
    #[serde(default)]
    synced_lyrics: Option<String>,
}

/// The song's lyrics, synced if LRCLIB has them so (`None` when it has none
/// at all), and the trouble met when a request failed on the way: then
/// what was found may not be all there is.
pub async fn find(http: &reqwest::Client, query: &Query) -> (Option<Lyrics>, Option<anyhow::Error>) {
    // Names as LRCLIB writes them: one plain space between words, where a
    // tag may have a wide one ("ダーリン・イン・ザ・フランキス　エンディング集").
    let spaced = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let query = &Query {
        title: spaced(&query.title),
        artist: spaced(&query.artist),
        album: query.album.as_deref().map(spaced),
        duration: query.duration,
    };
    let mut trouble = None;
    // The exact record first: title, artist, album and length.
    let mut params = vec![("track_name", query.title.clone()), ("artist_name", query.artist.clone())];
    if let Some(album) = &query.album {
        params.push(("album_name", album.clone()));
    }
    if let Some(duration) = query.duration {
        params.push(("duration", duration.to_string()));
    }
    let mut best: Option<Record> = None;
    match get(http, &format!("{API}/get"), &params).await {
        Ok(found) => {
            if let Some(record) = found.and_then(|v| serde_json::from_value::<Record>(v).ok()) {
                if record.synced_lyrics.is_some() {
                    return (to_lyrics(record), None);
                }
                best = Some(record);
            }
        }
        Err(error) => trouble = Some(error),
    }
    // Then a search, under each form of the title, for the closest length;
    // and last the whole of it as one search, which LRCLIB answers when the
    // named fields find nothing (by the same artist, then).
    let fits = |r: &Record| match (query.duration, r.duration) {
        (Some(want), Some(have)) => (f64::from(want) - have).abs() <= 3.0,
        _ => true,
    };
    let artist = crate::local::loose(&query.artist);
    let mut searches: Vec<Vec<(&str, String)>> =
        title_forms(&query.title).into_iter().map(|title| vec![("track_name", title), ("artist_name", query.artist.clone())]).collect();
    searches.push(vec![("q", format!("{} {}", query.title, query.artist))]);
    for params in searches {
        let anywhere = params[0].0 == "q";
        let records: Vec<Record> = match get(http, &format!("{API}/search"), &params).await {
            Ok(Some(found)) => serde_json::from_value(found).unwrap_or_default(),
            Ok(None) => continue,
            Err(error) => {
                trouble.get_or_insert(error);
                continue;
            }
        };
        let theirs = |r: &Record| !anywhere || r.artist_name.as_deref().is_some_and(|a| !artist.is_empty() && crate::local::loose(a).contains(&artist));
        if let Some(record) = records.into_iter().filter(fits).filter(theirs).max_by_key(|r| (r.synced_lyrics.is_some(), r.plain_lyrics.is_some())) {
            if record.synced_lyrics.is_some() {
                return (to_lyrics(record), None);
            }
            if best.is_none() {
                best = Some(record);
            }
        }
    }
    (best.and_then(to_lyrics), trouble)
}

async fn get(http: &reqwest::Client, url: &str, params: &[(&str, String)]) -> Result<Option<serde_json::Value>> {
    let mut last = GATE.lock().await;
    if let Some(at) = *last {
        let wait = SPACING.saturating_sub(at.elapsed());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
    let result = get_now(http, url, params).await;
    *last = Some(std::time::Instant::now());
    result
}

async fn get_now(http: &reqwest::Client, url: &str, params: &[(&str, String)]) -> Result<Option<serde_json::Value>> {
    // Turned away all the same (429, with a second or so to wait): wait as
    // asked, a little longer each time. A server having trouble (503, as
    // LRCLIB's does at busy times) is asked twice more, a while apart.
    let mut attempt = 0;
    let mut troubled = 0;
    let response = loop {
        let response = http
            .get(url)
            .query(params)
            .header("User-Agent", USER_AGENT)
            .timeout(Duration::from_secs(8))
            .send()
            .await?;
        if response.status().is_server_error() && troubled < 2 {
            troubled += 1;
            tokio::time::sleep(Duration::from_millis(1500 * troubled)).await;
            continue;
        }
        if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS || attempt == 5 {
            break response;
        }
        attempt += 1;
        let asked = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(1)
            .clamp(1, 5);
        tokio::time::sleep(Duration::from_secs(asked) + Duration::from_millis(600 * attempt)).await;
    };
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response.error_for_status()?;
    Ok(Some(response.json().await?))
}

fn to_lyrics(record: Record) -> Option<Lyrics> {
    if record.instrumental {
        return None;
    }
    let mut lines = record.synced_lyrics.as_deref().map(parse_lrc).unwrap_or_default();
    // Backing vocals written into their line, under it.
    for line in &mut lines {
        super::lyric_formats::split_backing(line);
    }
    let text = record
        .plain_lyrics
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"));
    if text.trim().is_empty() && lines.is_empty() {
        return None;
    }
    Some(Lyrics {
        text,
        source: Some(if lines.is_empty() { "Lyrics from LRCLIB" } else { "Synced lyrics from LRCLIB" }.into()),
        provider: Some("LRCLIB".into()),
        lines,
        ..Lyrics::default()
    })
}

/// The title as given, then without what follows a dash or sits in
/// brackets: "ゴールデンタイムラバー - Golden Time Lover" is also looked up
/// as each half, "Song (feat. Someone)" as "Song".
fn title_forms(title: &str) -> Vec<String> {
    let mut forms = vec![title.trim().to_owned()];
    let mut add = |form: String| {
        let form = form.trim().to_owned();
        if !form.is_empty() && !forms.contains(&form) {
            forms.push(form);
        }
    };
    let mut bare = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' | '（' | '【' => depth += 1,
            ')' | ']' | '）' | '】' => depth = (depth - 1).max(0),
            _ if depth == 0 => bare.push(c),
            _ => {}
        }
    }
    add(bare.clone());
    for part in bare.split(" - ") {
        add(part.to_owned());
    }
    forms.truncate(4);
    forms
}

/// LRC lines: `[mm:ss.xx] words`, perhaps with several times on one line.
pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let tag = &stripped[..end];
            rest = stripped[end + 1..].trim_start();
            if let Some(time) = parse_time(tag) {
                times.push(time);
            }
        }
        for time in times {
            lines.push(LyricLine { time, text: rest.trim().to_owned(), ..LyricLine::default() });
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    // Two empty lines in a row say no more than one.
    lines.dedup_by(|b, a| a.text.is_empty() && b.text.is_empty());
    lines
}

fn parse_time(tag: &str) -> Option<f64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes: f64 = minutes.trim().parse().ok()?;
    let seconds: f64 = seconds.trim().replace(',', ".").parse().ok()?;
    Some(minutes * 60.0 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_lines() {
        let lines = parse_lrc("[ar:Someone]\n[00:01.30] Is this the real life?\n[00:07.48][01:07.00] Caught in a landslide\n[00:12.00]\n");
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].text, "Is this the real life?");
        assert!((lines[1].time - 7.48).abs() < 1e-9);
        assert_eq!(lines[2].text, "");
        assert_eq!(lines[3].time, 67.0);
    }

    #[test]
    fn title_forms_split() {
        let forms = title_forms("ゴールデンタイムラバー - Golden Time Lover (Live)");
        assert_eq!(forms[0], "ゴールデンタイムラバー - Golden Time Lover (Live)");
        assert!(forms.contains(&"ゴールデンタイムラバー".to_owned()));
        assert!(forms.contains(&"Golden Time Lover".to_owned()));
    }
}
