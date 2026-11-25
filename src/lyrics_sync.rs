//! Lyrics timed to the song by listening to it: the song's audio goes to
//! Fish Audio's speech recognition (`transcribe-1-pro`), which says each
//! word it hears and when. Lyrics that came as plain text are matched to
//! those words, so each of their words takes the time it was sung at; a
//! song with no lyrics at all gets what was heard, in lines.
//!
//! Only ever on a click, for the song showing, one at a time: each costs
//! the service a song's length of listening.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::api::model::{LyricLine, Lyrics, Voice, Word};
use crate::player::StreamInfo;

/// The provider's name, among the song's sources.
pub const PROVIDER: &str = "Fish Audio";

/// A word heard, and when.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Heard {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    segments: Vec<Heard>,
}

// ---- Progress ---------------------------------------------------------------

/// The four steps of a sync, in order, for the strip that shows them.
pub const STEPS: [&str; 4] = ["Fetch", "Prepare", "Listen", "Sync"];

/// Where a sync is: which step, and a log of what it has done, newest last.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub step: usize,
    pub log: Vec<String>,
}

/// A sync's progress, shared between the task doing it and the window
/// showing it.
#[derive(Clone, Default)]
pub struct Progress(std::sync::Arc<parking_lot::Mutex<Step>>);

impl Progress {
    /// On to `step`, saying so.
    pub fn at(&self, step: usize, said: impl Into<String>) {
        let mut now = self.0.lock();
        now.step = step.min(STEPS.len() - 1);
        now.log.push(said.into());
        if now.log.len() > 40 {
            now.log.remove(0);
        }
    }

    /// The last thing said, said again with more (a count going up).
    pub fn update(&self, said: impl Into<String>) {
        let mut now = self.0.lock();
        match now.log.last_mut() {
            Some(last) => *last = said.into(),
            None => now.log.push(said.into()),
        }
    }

    pub fn get(&self) -> Step {
        self.0.lock().clone()
    }
}

// ---- The key ---------------------------------------------------------------

// The app's own key, sealed so it is not there to read as text in the
// program or its source. (Sealed, not secret: a program that can use a key
// can be made to give it up. It is for this app's use.)
const SEALED: [u8; 51] = [0xf0, 0x3f, 0x10, 0x53, 0x38, 0x8d, 0xd8, 0x7c, 0x82, 0x07, 0xc6, 0xcf, 0xce, 0xa6, 0x68, 0x68, 0x1d, 0x29, 0x40, 0xbb, 0x18, 0xa1, 0x4f, 0x31, 0x9e, 0x37, 0xbd, 0xa6, 0xa6, 0xfb, 0xe2, 0xb6, 0xb9, 0xfd, 0x2f, 0x69, 0xa8, 0x4e, 0x73, 0x9e, 0xdc, 0xb2, 0x4d, 0x84, 0x0a, 0x33, 0x27, 0x95, 0xec, 0x6f, 0xdb];
const PAD: [u8; 51] = [0xb2, 0x60, 0xe6, 0x62, 0x2a, 0x78, 0x69, 0x1e, 0xc5, 0xdc, 0xaa, 0xcc, 0x37, 0x1c, 0x29, 0x4c, 0xe8, 0x60, 0x68, 0x76, 0xbf, 0xcd, 0x10, 0xc5, 0xf4, 0x33, 0x1d, 0x22, 0xef, 0xff, 0x10, 0xfa, 0x86, 0x5e, 0xc4, 0x24, 0xf6, 0x99, 0xbd, 0xb0, 0x12, 0x3d, 0x36, 0xc5, 0xd7, 0xbe, 0x09, 0x63, 0x18, 0x26, 0xc3];

/// The key to use: the person's own, when they gave one, else the app's.
pub fn key(own: &str) -> String {
    let own = own.trim();
    if !own.is_empty() {
        return own.to_owned();
    }
    // Through black_box, so the compiler cannot unseal it ahead of time and
    // keep the answer in the program as text (it did, in a release build).
    let sealed: &[u8] = std::hint::black_box(&SEALED[..]);
    let pad: &[u8] = std::hint::black_box(&PAD[..]);
    let (step, start) = std::hint::black_box((73u32, 41u32));
    let n = sealed.len().min(pad.len());
    let bytes: Vec<u8> = (0..n)
        .rev()
        .map(|i| sealed[i] ^ pad[i] ^ ((i as u32 * step + start) & 0xff) as u8)
        .collect();
    String::from_utf8(bytes).unwrap_or_default()
}

// ---- Listening -------------------------------------------------------------

/// The song's audio, whole: from its file when it has one (kept, or on this
/// PC), else from its stream, a megabyte at a time.
pub async fn audio(http: &reqwest::Client, stream: &StreamInfo, progress: &Progress) -> Result<Vec<u8>> {
    if let Some(path) = crate::cache::file_of(&stream.url) {
        progress.at(0, format!("Reading the song from disk ({})", megabytes(stream.content_length)));
        return tokio::fs::read(path).await.context("cannot read the song's file");
    }
    const CHUNK: u64 = 1 << 20;
    let len = stream.content_length;
    if len == 0 || len > 200 << 20 {
        bail!("the song's size is unknown");
    }
    progress.at(0, format!("Downloading the song: 0 of {}", megabytes(len)));
    let mut bytes = Vec::with_capacity(len as usize);
    let headers = stream.header_map();
    while (bytes.len() as u64) < len {
        let start = bytes.len() as u64;
        let end = (start + CHUNK).min(len) - 1;
        let response = http
            .get(&stream.url)
            .headers(headers.clone())
            .header("Range", format!("bytes={start}-{end}"))
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        if !response.status().is_success() {
            bail!("the stream refused the download (HTTP {})", response.status().as_u16());
        }
        let chunk = response.bytes().await.map_err(reqwest::Error::without_url)?;
        if chunk.is_empty() {
            bail!("the stream ended early");
        }
        bytes.extend_from_slice(&chunk);
        progress.update(format!("Downloading the song: {} of {}", megabytes(bytes.len() as u64), megabytes(len)));
    }
    Ok(bytes)
}

/// Every word Fish Audio hears in `audio`, with its time.
pub async fn transcribe(http: &reqwest::Client, key: &str, audio: Vec<u8>, mime: &str) -> Result<Vec<Heard>> {
    let name = if mime.contains("wav") { "piece.wav" } else if mime.contains("webm") { "song.webm" } else if mime.contains("mp4") { "song.m4a" } else { "song" };
    let part = reqwest::multipart::Part::bytes(audio).file_name(name).mime_str(mime)?;
    let form = reqwest::multipart::Form::new()
        .part("audio", part)
        .text("ignore_timestamps", "false")
        .text("tag_audio_events", "false")
        .text("diarize", "false");
    let response = http
        .post("https://api.fish.audio/v1/asr")
        .bearer_auth(key)
        .header("model", "transcribe-1-pro")
        .multipart(form)
        .timeout(Duration::from_secs(600))
        .send()
        .await?;
    let status = response.status().as_u16();
    if status != 200 {
        let body = response.text().await.unwrap_or_default();
        log::warn!("Fish Audio: HTTP {status}: {}", body.chars().take(300).collect::<String>());
        bail!(
            "{}",
            match status {
                401 | 403 => "the transcription key was turned down".to_owned(),
                402 => "the transcription service is out of credit".to_owned(),
                413 => "the song is too large to send".to_owned(),
                429 => "the transcription service is busy; try again in a minute".to_owned(),
                500..=599 => "the transcription service had a problem; try again later".to_owned(),
                _ => format!("the transcription service said no (HTTP {status})"),
            }
        );
    }
    let answer: Answer = response.json().await.context("an answer that could not be read")?;
    Ok(answer.segments)
}


/// How the song is sent: pieces this long, each starting this long after
/// the one before, so neighbours overlap. Sent whole, a song comes back
/// with its words' times drifting badly after a minute or two (and none at
/// all for its first words); a piece this short keeps them right.
const PIECE_SECS: f64 = 40.0;
const STEP_SECS: f64 = 30.0;
/// The pieces go as 16 kHz mono WAV, which speech recognition needs no
/// more than.
const RATE: u32 = 16_000;
/// Pieces in flight at once (the service allows a few per account).
const AT_ONCE: usize = 3;

/// Where each piece starts and ends, in seconds: a last piece too short to
/// stand on its own is joined to the one before.
fn cut(total: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut start = 0.0;
    while start < total {
        let end = (start + PIECE_SECS).min(total);
        out.push((start, end));
        if end >= total {
            break;
        }
        start += STEP_SECS;
        if total - start < 12.0 {
            if let Some(last) = out.last_mut() {
                last.1 = total;
            }
            break;
        }
    }
    out
}

/// 16-bit PCM WAV of `samples` (one channel at `RATE`).
fn wav(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // one channel
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

/// The words of every piece put back into one song: each piece's words in
/// the song's time, each overlap split at its middle so a word is kept
/// once, the words a piece could not time left out.
fn stitch(pieces: &[(f64, f64)], heard: Vec<Vec<Heard>>) -> Vec<Heard> {
    let margin = (PIECE_SECS - STEP_SECS) / 2.0;
    let last = pieces.len().saturating_sub(1);
    let mut out = Vec::new();
    for (i, ((start, _), words)) in pieces.iter().zip(heard).enumerate() {
        for word in words {
            if word.start <= 0.0 && word.end <= 0.0 {
                continue;
            }
            let from = if i == 0 { f64::NEG_INFINITY } else { margin };
            let to = if i == last { f64::INFINITY } else { STEP_SECS + margin };
            if word.start >= from && word.start < to {
                out.push(Heard { text: word.text, start: start + word.start, end: start + word.end.max(word.start) });
            }
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// Every word sung in the song, with its time: the song decoded, sent in
/// pieces (a few at once, each tried again once should the service be
/// busy), and the pieces' words put back together.
pub async fn listen(http: &reqwest::Client, key: &str, audio: Vec<u8>, mime: &str, extension: &str, progress: &Progress) -> Result<Vec<Heard>> {
    progress.at(1, "Decoding the audio");
    let (mime_owned, extension_owned) = (mime.to_owned(), extension.to_owned());
    let samples = tokio::task::spawn_blocking(move || crate::player::decode_mono(audio, &mime_owned, &extension_owned, RATE))
        .await
        .context("the decoder stopped")?
        .map_err(|e| anyhow!("the song could not be decoded: {e}"))?;
    let total = samples.len() as f64 / f64::from(RATE);
    let pieces = cut(total);
    log::info!("listening to {total:.0} s of song in {} pieces", pieces.len());
    progress.update(format!("Decoded {}:{:02} of audio", total as u64 / 60, total as u64 % 60));
    progress.at(1, format!("Cut into {} pieces of {} s, overlapping", pieces.len(), PIECE_SECS as u64));
    progress.at(2, format!("Sent to Fish Audio: 0 of {} pieces heard", pieces.len()));
    let samples = std::sync::Arc::new(samples);
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
    let mut tasks = tokio::task::JoinSet::new();
    for (i, &(start, end)) in pieces.iter().enumerate() {
        let (http, key, samples, permits) = (http.clone(), key.to_owned(), samples.clone(), permits.clone());
        tasks.spawn(async move {
            let _permit = permits.acquire_owned().await.ok();
            let range = (start * f64::from(RATE)) as usize..((end * f64::from(RATE)) as usize).min(samples.len());
            let body = wav(&samples[range]);
            let mut tries = 0;
            loop {
                tries += 1;
                match transcribe(&http, &key, body.clone(), "audio/wav").await {
                    Ok(words) => return (i, Ok(words)),
                    Err(error) => {
                        let again = format!("{error}").contains("busy") || format!("{error}").contains("had a problem");
                        if !again || tries >= 2 {
                            return (i, Err(error));
                        }
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
            }
        });
    }
    let mut heard: Vec<Vec<Heard>> = vec![Vec::new(); pieces.len()];
    let mut finished = 0;
    while let Some(done) = tasks.join_next().await {
        let (i, result) = done.context("a piece stopped")?;
        heard[i] = result?;
        finished += 1;
        progress.update(format!("Sent to Fish Audio: {finished} of {} pieces heard", pieces.len()));
    }
    let words = stitch(&pieces, heard);
    progress.update(format!("Heard {} words in {} pieces", words.len(), pieces.len()));
    Ok(words)
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

// ---- Matching --------------------------------------------------------------

/// One piece of a line: a word with the space after it (or one character of
/// Chinese, Japanese or Korean), and how it is compared.
#[derive(Debug)]
struct Piece {
    text: String,
    key: String,
}

fn cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30ff // kana
        | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff // Han
        | 0xac00..=0xd7af | 0x1100..=0x11ff | 0x3130..=0x318f) // Hangul
}

/// How a piece is compared: its letters and digits, in lower case.
fn key_of(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// A line cut into pieces that, put back together, are the line.
fn pieces(line: &str) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<Piece>| {
        if !word.is_empty() {
            out.push(Piece { key: key_of(word), text: std::mem::take(word) });
        }
    };
    for c in line.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut out);
            match out.last_mut() {
                Some(last) => last.text.push(c),
                None => {}
            }
        } else if cjk(c) {
            flush(&mut word, &mut out);
            out.push(Piece { text: c.to_string(), key: key_of(&c.to_string()) });
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// What was heard, a word (or a character of CJK) at a time, each with its
/// share of its segment's time.
fn heard_pieces(heard: &[Heard]) -> Vec<(String, f64, f64)> {
    let mut out = Vec::new();
    for h in heard {
        let parts: Vec<String> = pieces(&h.text).into_iter().map(|p| p.key).filter(|k| !k.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }
        let step = (h.end - h.start).max(0.0) / parts.len() as f64;
        for (i, key) in parts.into_iter().enumerate() {
            let start = h.start + step * i as f64;
            out.push((key, start, start + step));
        }
    }
    out
}

/// How alike two words are, from 0 to 1 (by edit distance).
fn alike(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut last = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = if ca == cb { last } else { 1 + last.min(old).min(row[j]) };
            last = old;
        }
    }
    1.0 - row[b.len()] as f32 / a.len().max(b.len()) as f32
}

/// The lines of plain lyrics, timed word by word from what was heard: the
/// two lined up (as an edit distance lines up two words), each lyric word
/// taking the time of the word heard opposite it, the rest spread between
/// their neighbours. None when too few of the words were heard to trust it.
pub fn align(text: &str, heard: &[Heard]) -> Option<Vec<LyricLine>> {
    let lines: Vec<Vec<Piece>> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(pieces).collect();
    let words: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .flat_map(|(l, ps)| (0..ps.len()).map(move |p| (l, p)))
        .collect();
    let heard = heard_pieces(heard);
    let (n, m) = (words.len(), heard.len());
    if n == 0 || m == 0 {
        return None;
    }
    let key = |i: usize| lines[words[i].0][words[i].1].key.as_str();
    // Costs: a lyric word not heard, a heard word not in the lyrics (ad
    // libs, repeats left out of the text), and two words set opposite.
    const SKIP_LYRIC: f32 = 1.0;
    const SKIP_HEARD: f32 = 0.7;
    let pair = |i: usize, j: usize| -> f32 {
        let k = key(i);
        if k.is_empty() {
            return 0.6;
        }
        match alike(k, &heard[j].0) {
            s if s >= 0.8 => 0.0,
            s if s >= 0.5 => 0.5,
            _ => 1.4,
        }
    };
    let width = m + 1;
    let mut cost = vec![0f32; (n + 1) * width];
    let mut from = vec![0u8; (n + 1) * width];
    for i in 1..=n {
        cost[i * width] = i as f32 * SKIP_LYRIC;
        from[i * width] = 1;
    }
    for j in 1..=m {
        cost[j] = j as f32 * SKIP_HEARD;
        from[j] = 2;
    }
    for i in 1..=n {
        for j in 1..=m {
            let both = cost[(i - 1) * width + j - 1] + pair(i - 1, j - 1);
            let lyric = cost[(i - 1) * width + j] + SKIP_LYRIC;
            let heard_only = cost[i * width + j - 1] + SKIP_HEARD;
            let (best, way) = if both <= lyric && both <= heard_only {
                (both, 0)
            } else if lyric <= heard_only {
                (lyric, 1)
            } else {
                (heard_only, 2)
            };
            cost[i * width + j] = best;
            from[i * width + j] = way;
        }
    }
    // Back from the end: which heard word each lyric word is opposite.
    let mut times: Vec<Option<(f64, f64)>> = vec![None; n];
    let mut alike_count = 0usize;
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        match from[i * width + j] {
            0 if i > 0 && j > 0 => {
                let (k, h) = (key(i - 1), &heard[j - 1]);
                if !k.is_empty() && alike(k, &h.0) >= 0.5 {
                    alike_count += 1;
                    times[i - 1] = Some((h.1, h.2));
                } else if !k.is_empty() {
                    // Misheard, most likely, but sung there.
                    times[i - 1] = Some((h.1, h.2));
                }
                i -= 1;
                j -= 1;
            }
            1 if i > 0 => i -= 1,
            _ if j > 0 => j -= 1,
            _ => i -= 1,
        }
    }
    let spoken = (0..n).filter(|&i| !key(i).is_empty()).count().max(1);
    if (alike_count as f32) < spoken as f32 * 0.35 {
        return None;
    }
    let times = fill_gaps(&times);
    let mut at = 0;
    let mut out = Vec::with_capacity(lines.len());
    for line in &lines {
        let mut words = Vec::with_capacity(line.len());
        for piece in line {
            let (start, end) = times[at];
            at += 1;
            words.push(Word { start, end, text: piece.text.clone() });
        }
        out.push(line_of(words));
    }
    Some(out)
}

/// Times for every word: those heard as they were, the others spread evenly
/// between the words around them (or a moment before the first, after the
/// last), never going backward.
fn fill_gaps(times: &[Option<(f64, f64)>]) -> Vec<(f64, f64)> {
    let n = times.len();
    let mut out = vec![(0.0, 0.0); n];
    let mut i = 0;
    let mut last_end: Option<f64> = None;
    while i < n {
        if let Some((s, e)) = times[i] {
            let s = last_end.map_or(s, |l| s.max(l.min(s)));
            out[i] = (s, e.max(s));
            last_end = Some(e.max(s));
            i += 1;
            continue;
        }
        let gap_start = i;
        while i < n && times[i].is_none() {
            i += 1;
        }
        let count = (i - gap_start) as f64;
        let next = times.get(i).copied().flatten().map(|(s, _)| s);
        let (from, to) = match (last_end, next) {
            (Some(a), Some(b)) => (a, b.max(a)),
            (None, Some(b)) => ((b - 0.4 * count).max(0.0), b),
            (Some(a), None) => (a, a + 0.4 * count),
            (None, None) => (0.0, 0.4 * count),
        };
        let step = (to - from) / count;
        for k in 0..(i - gap_start) {
            let s = from + step * k as f64;
            out[gap_start + k] = (s, s + step);
        }
        last_end = Some(to);
    }
    // Never backward.
    for k in 1..n {
        if out[k].0 < out[k - 1].0 {
            out[k].0 = out[k - 1].0;
            out[k].1 = out[k].1.max(out[k].0);
        }
    }
    out
}

fn line_of(words: Vec<Word>) -> LyricLine {
    let text: String = words.iter().map(|w| w.text.as_str()).collect::<String>().trim_end().to_owned();
    let mut words = words;
    if let Some(last) = words.last_mut() {
        last.text = last.text.trim_end().to_owned();
    }
    LyricLine {
        time: words.first().map_or(0.0, |w| w.start),
        end: words.last().map(|w| w.end),
        text,
        words,
        backing: Vec::new(),
        translation: None,
        voice: Voice::First,
        second_voice: false,
    }
}

/// What was heard, as lyrics of its own: a line begins where the words
/// heard begin with a capital (the transcript starts each sung line with
/// one; "I" aside), where the singing stops for a while, or once a line is
/// long. A held note is no reason to break.
pub fn lines_heard(heard: &[Heard]) -> Vec<LyricLine> {
    let mut out = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut chars = 0usize;
    for (n, h) in heard.iter().enumerate() {
        let text = h.text.trim();
        if text.is_empty() {
            continue;
        }
        let asian = text.chars().all(cjk);
        let gap = words.last().map_or(0.0, |w| h.start - w.end);
        let capital = !asian && starts_a_line(text) && words.len() >= 2;
        let pause = (gap > 1.2 && words.len() >= 2) || gap > 2.5;
        let long = if asian { chars >= 18 } else { words.len() >= 10 };
        if !words.is_empty() && (capital || pause || long) {
            out.push(line_of(std::mem::take(&mut words)));
            chars = 0;
        }
        let next_asian = heard.get(n + 1).is_some_and(|h| h.text.trim().chars().all(cjk));
        let spaced = if asian && next_asian { text.to_owned() } else { format!("{text} ") };
        chars += text.chars().count();
        words.push(Word { start: h.start, end: h.end.max(h.start), text: spaced });
        // A sentence's end ends the line.
        if text.ends_with(['.', '!', '?', '\u{3002}', '\u{ff01}', '\u{ff1f}']) {
            out.push(line_of(std::mem::take(&mut words)));
            chars = 0;
        }
    }
    if !words.is_empty() {
        out.push(line_of(words));
    }
    out
}

/// Whether a word heard begins with a capital letter, as a line would: not
/// "I" and its forms.
fn starts_a_line(word: &str) -> bool {
    let letters: String = word.chars().skip_while(|c| !c.is_alphabetic()).collect();
    let bare = letters.trim_end_matches(|c: char| !c.is_alphanumeric());
    let first_person = bare == "I" || bare.starts_with("I'") || bare.starts_with("I\u{2019}");
    letters.chars().next().is_some_and(char::is_uppercase) && !first_person
}

/// Plain lyrics (when there are some) timed to the song, or else what was
/// heard, as lyrics from Fish Audio.
pub fn lyrics_from(plain: Option<&Lyrics>, heard: &[Heard]) -> Result<Lyrics> {
    if heard.is_empty() {
        bail!("no singing was heard");
    }
    let (lines, source) = match plain.filter(|p| !p.text.trim().is_empty()) {
        Some(plain) => {
            let lines = align(&plain.text, heard).ok_or_else(|| anyhow!("the singing didn't match the lyrics closely enough"))?;
            let from = plain.provider.as_deref().unwrap_or("YouTube Music");
            (lines, format!("{from}'s lyrics, timed by listening to the song (Fish Audio)"))
        }
        None => (lines_heard(heard), "Heard in the song by Fish Audio's speech recognition; it may mishear".to_owned()),
    };
    if lines.is_empty() {
        bail!("no singing was heard");
    }
    let text = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    Ok(Lyrics {
        text,
        source: Some(source),
        lines,
        provider: Some(PROVIDER.to_owned()),
        songwriters: plain.map(|p| p.songwriters.clone()).unwrap_or_default(),
        others: Vec::new(),
        translated: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(words: &[(&str, f64)]) -> Vec<Heard> {
        words.iter().map(|&(t, s)| Heard { text: t.to_owned(), start: s, end: s + 0.4 }).collect()
    }

    #[test]
    fn the_key_unseals() {
        let key = key("");
        assert!(key.starts_with("sk-fish-") && key.len() > 20, "{}", key.len());
        assert_eq!(super::key(" mine "), "mine");
    }

    #[test]
    fn pieces_put_back_together_are_the_line() {
        for line in ["Just forget it", "Ask me\"What's happening?\"", "君の名は  Kimi no na wa", "  spaced   out  "] {
            let joined: String = pieces(line.trim()).iter().map(|p| p.text.as_str()).collect();
            assert_eq!(joined, line.trim());
        }
        assert_eq!(pieces("君の").len(), 2);
        assert_eq!(key_of("What's,"), "whats");
    }

    #[test]
    fn plain_lyrics_take_the_times_they_were_sung_at() {
        let text = "Just forget it\nYou make it so hard\n\nCould I take the risk?";
        // Heard with a word misheard, one missed, and an ad lib.
        let h = heard(&[
            ("just", 10.0),
            ("forget", 10.5),
            ("it", 11.0),
            ("yeah", 12.0),
            ("you", 13.0),
            ("make", 13.5),
            ("so", 14.5),
            ("hard", 15.0),
            ("could", 20.0),
            ("I", 20.5),
            ("tick", 21.0),
            ("the", 21.5),
            ("risk", 22.0),
        ]);
        let lines = align(text, &h).expect("aligned");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].time, 10.0);
        assert_eq!(lines[1].time, 13.0);
        assert_eq!(lines[2].time, 20.0);
        // "it", not heard in the second line, falls between "make" and "so".
        let it = &lines[1].words[2];
        assert!(it.start >= 13.9 && it.start <= 14.5, "{}", it.start);
        // "take", misheard as "tick", keeps that time.
        assert_eq!(lines[2].words[2].start, 21.0);
        // Words put back together are the line, for the sweep.
        for line in &lines {
            let joined: String = line.words.iter().map(|w| w.text.as_str()).collect();
            assert_eq!(joined, line.text);
        }
        // Times never go backward.
        let all: Vec<f64> = lines.iter().flat_map(|l| l.words.iter().map(|w| w.start)).collect();
        assert!(all.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn lyrics_of_another_song_are_turned_down() {
        let h = heard(&[("hello", 1.0), ("darkness", 1.5), ("my", 2.0), ("old", 2.5), ("friend", 3.0)]);
        assert!(align("Never gonna give you up\nNever gonna let you down", &h).is_none());
    }

    /// Sends a song to Fish Audio for real (it costs its length in credit):
    /// NATIVE_YTM_SYNC_AUDIO=song.webm [NATIVE_YTM_SYNC_TEXT=lyrics.txt]
    /// cargo test -- --ignored listens
    #[test]
    #[ignore]
    fn listens_to_a_song() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let http = reqwest::Client::new();
        let started = std::time::Instant::now();
        // NATIVE_YTM_SYNC_JSON: an answer kept from before, read again.
        let heard = match std::env::var("NATIVE_YTM_SYNC_JSON") {
            Ok(path) => serde_json::from_slice::<Answer>(&std::fs::read(path).expect("json")).expect("answer").segments,
            Err(_) => {
                let audio = std::fs::read(std::env::var("NATIVE_YTM_SYNC_AUDIO").expect("audio")).expect("audio file");
                runtime.block_on(listen(&http, &key(""), audio, "audio/webm", "webm", &Progress::default())).expect("heard")
            }
        };
        println!("heard {} words in {:.1} s", heard.len(), started.elapsed().as_secs_f32());
        for h in heard.iter().take(8) {
            println!("  heard {:7.2} to {:7.2}, {} letters", h.start, h.end, h.text.chars().count());
        }
        // With NATIVE_YTM_SYNC_TITLE and _ARTIST: LRCLIB's timed lyrics,
        // their text synced again here and the times compared with theirs.
        let reference = std::env::var("NATIVE_YTM_SYNC_TITLE").ok().and_then(|title| {
            let query = crate::api::lrclib::Query { title, artist: std::env::var("NATIVE_YTM_SYNC_ARTIST").unwrap_or_default(), album: None, duration: None };
            runtime.block_on(crate::api::lrclib::find(&http, &query)).0
        });
        let reference_lines: Vec<(f64, String)> = reference.iter().flat_map(|l| l.lines.iter()).filter(|l| !l.text.trim().is_empty()).map(|l| (l.time, l.text.clone())).collect();
        let plain = (!reference_lines.is_empty()).then(|| reference_lines.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("
"));
        let lyrics = Lyrics { text: plain.clone().unwrap_or_default(), source: None, lines: Vec::new(), provider: None, songwriters: Vec::new(), others: Vec::new(), translated: Default::default() };
        let made = lyrics_from(plain.as_ref().map(|_| &lyrics), &heard).expect("lyrics");
        for line in made.lines.iter().take(14) {
            println!("  line at {:7.2}, {} words, to {:7.2}", line.time, line.words.len(), line.end.unwrap_or(0.0));
        }
        println!("{} lines", made.lines.len());
        if made.lines.len() == reference_lines.len() && !reference_lines.is_empty() {
            for (a, (t, _)) in made.lines.iter().zip(&reference_lines).step_by(4) {
                println!("  synced {:7.2}  LRCLIB {:7.2}", a.time, t);
            }
            let errors: Vec<f64> = made.lines.iter().zip(&reference_lines).map(|(a, (t, _))| (a.time - t).abs()).collect();
            let mean = errors.iter().sum::<f64>() / errors.len() as f64;
            let within = errors.iter().filter(|e| **e < 0.5).count();
            println!("against LRCLIB: mean {mean:.2} s off, {within} of {} lines within half a second", errors.len());
        }
        assert!(made.by_word());
    }

    #[test]
    fn a_song_is_cut_into_overlapping_pieces_and_put_back_together() {
        assert_eq!(cut(25.0), vec![(0.0, 25.0)]);
        assert_eq!(cut(100.0), vec![(0.0, 40.0), (30.0, 70.0), (60.0, 100.0)]);
        // A last piece under twelve seconds joins the one before.
        assert_eq!(cut(95.0), vec![(0.0, 40.0), (30.0, 70.0), (60.0, 95.0)]);
        assert_eq!(cut(71.0), vec![(0.0, 40.0), (30.0, 71.0)]);
        let w = |t: &str, s: f64| Heard { text: t.into(), start: s, end: s + 0.3 };
        let pieces = cut(70.0);
        // The overlap (30 to 40 s) is heard twice; each word is kept once,
        // and words a piece could not time are left out.
        let heard = vec![
            vec![Heard { text: "lost".into(), start: 0.0, end: 0.0 }, w("a", 10.0), w("b", 34.0), w("c", 36.0)],
            vec![w("b", 4.0), w("c", 6.0), w("d", 20.0)],
        ];
        let words: Vec<(String, f64)> = stitch(&pieces, heard).into_iter().map(|h| (h.text, h.start)).collect();
        assert_eq!(words, vec![("a".into(), 10.0), ("b".into(), 34.0), ("c".into(), 36.0), ("d".into(), 50.0)]);
        assert_eq!(&wav(&[0.0; 4])[..4], b"RIFF");
        assert_eq!(wav(&[0.0; 4]).len(), 44 + 8);
    }

    #[test]
    fn what_was_heard_breaks_into_lines_at_pauses() {
        let h = heard(&[("I", 1.0), ("see", 1.4), ("you.", 1.8), ("Over", 5.0), ("there", 5.4), ("now", 9.0)]);
        let lines = lines_heard(&h);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["I see you.", "Over there", "now"]);
        assert_eq!(lines[1].time, 5.0);
        // A capital starts a line; "I" does not, nor a held note.
        let h = heard(&[("One", 1.0), ("last", 1.4), ("time", 1.8), ("'Cause", 2.3), ("this", 2.7), ("love", 3.1), ("of", 3.5), ("ours", 4.9), ("I", 5.3), ("know", 5.7)]);
        let texts: Vec<String> = lines_heard(&h).into_iter().map(|l| l.text).collect();
        assert_eq!(texts, ["One last time", "'Cause this love of ours I know"]);
    }
}
