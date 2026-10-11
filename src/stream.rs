//! Finding a playable audio stream for a video id.
//!
//! Natively first: the player request as a client YouTube still serves
//! direct addresses to (no JavaScript challenge, no proof-of-origin token),
//! checked with a request near the file's end, since some clients' addresses
//! only work for the first megabyte. Opus (itag 251) is taken over AAC
//! (itag 140): it sounds better at the same rate.
//!
//! Then yt-dlp, which keeps up with YouTube's changes. Signed in, yt-dlp is
//! also how the account's own streams are reached: Premium's 256 kb/s audio
//! (itag 774, Opus, or 141, AAC) when Premium quality is on, and songs
//! YouTube keeps for Premium members, which the signed-out clients are
//! refused. Those need the web player's JavaScript, which yt-dlp runs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::api::client::{Auth, Innertube, PlayerClient};
use crate::player::StreamInfo;

/// Where the stream came from, for the log and the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Native,
    YtDlp,
    /// yt-dlp, signed in with the account.
    Account,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quality {
    /// What any account gets: Opus at about 130 kb/s, or AAC at 128.
    Standard,
    /// Premium's 256 kb/s streams, when the account has them.
    Premium,
}

#[derive(Clone)]
pub struct Resolver {
    pub clients: Vec<PlayerClient>,
    pub ytdlp: Option<PathBuf>,
    pub use_ytdlp: bool,
    /// Ask for Premium's 256 kb/s streams when signed in.
    pub premium: bool,
    /// Where yt-dlp's short-lived cookie files go.
    pub scratch: PathBuf,
}

impl Resolver {
    pub fn new(ytdlp_override: Option<PathBuf>, use_ytdlp: bool, premium: bool, scratch: PathBuf) -> Self {
        Self {
            clients: PlayerClient::defaults(),
            ytdlp: ytdlp_override.or_else(|| find_on_path("yt-dlp.exe").or_else(|| find_on_path("yt-dlp"))),
            use_ytdlp,
            premium,
            scratch,
        }
    }

    /// The signed-out clients alone: the quickest stream, to start on.
    pub async fn resolve_native(&self, api: &Innertube, video_id: &str) -> Result<(StreamInfo, Via)> {
        let mut errors = Vec::new();
        for client in &self.clients {
            match native(api, client, video_id).await {
                Ok(stream) => return Ok((stream, Via::Native)),
                Err(error) => {
                    log::info!("{} could not resolve {video_id}: {error:#}", client.name);
                    errors.push(format!("{}: {error}", client.name));
                }
            }
        }
        Err(anyhow!(errors.join("; ")))
    }

    /// The account's own stream through yt-dlp: Premium's 256 kb/s, or a
    /// song only Premium members can play.
    pub async fn resolve_account(&self, api: &Innertube, video_id: &str) -> Result<(StreamInfo, Via)> {
        let auth = api.auth().filter(Auth::is_usable).ok_or_else(|| anyhow!("not signed in"))?;
        let path = self.ytdlp.as_ref().ok_or_else(|| anyhow!("yt-dlp: not installed"))?;
        let quality = if self.premium { Quality::Premium } else { Quality::Standard };
        ytdlp(api, path, video_id, Some(&auth), quality, &self.scratch)
            .await
            .map(|stream| (stream, Via::Account))
    }

    pub async fn resolve(&self, api: &Innertube, video_id: &str) -> Result<(StreamInfo, Via)> {
        let auth = api.auth().filter(Auth::is_usable);
        let mut errors = Vec::new();
        let mut tried_account = false;
        if self.premium
            && let (Some(auth), Some(path)) = (&auth, &self.ytdlp)
        {
            tried_account = true;
            match ytdlp(api, path, video_id, Some(auth), Quality::Premium, &self.scratch).await {
                Ok(stream) => return Ok((stream, Via::Account)),
                Err(error) => {
                    log::warn!("yt-dlp with the account could not resolve {video_id}: {error:#}");
                    errors.push(format!("yt-dlp with your account: {error}"));
                }
            }
        }
        let mut members_only = false;
        for client in &self.clients {
            match native(api, client, video_id).await {
                Ok(stream) => return Ok((stream, Via::Native)),
                Err(error) => {
                    let text = format!("{error:#}");
                    members_only |= text.contains("Premium");
                    log::info!("{} could not resolve {video_id}: {text}", client.name);
                    errors.push(format!("{}: {error}", client.name));
                }
            }
        }
        // A song for Premium members needs the account; anything else yt-dlp
        // may find signed out.
        let account = auth.as_ref().filter(|_| members_only && !tried_account);
        if self.use_ytdlp || account.is_some() {
            match &self.ytdlp {
                Some(path) => match ytdlp(api, path, video_id, account, Quality::Standard, &self.scratch).await {
                    Ok(stream) => {
                        return Ok((stream, if account.is_some() { Via::Account } else { Via::YtDlp }));
                    }
                    Err(error) => {
                        log::warn!("yt-dlp could not resolve {video_id}: {error:#}");
                        errors.push(format!("yt-dlp: {error}"));
                    }
                },
                None => errors.push("yt-dlp: not installed".into()),
            }
        }
        if members_only {
            bail!(
                "only for Music Premium members{}",
                if auth.is_none() { "; sign in with a Premium account" } else { "" }
            );
        }
        Err(anyhow!(errors.join("; ")))
    }
}

async fn native(api: &Innertube, client: &PlayerClient, video_id: &str) -> Result<StreamInfo> {
    let response = api.player_as(client, video_id).await?;
    let status = response
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    if status != "OK" {
        let reason = response
            .pointer("/playabilityStatus/reason")
            .and_then(Value::as_str)
            .unwrap_or("");
        bail!("{status} {reason}");
    }
    let formats = response
        .pointer("/streamingData/adaptiveFormats")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("no adaptive formats"))?;
    let format = pick_format(formats).ok_or_else(|| anyhow!("no Opus or AAC audio with an address"))?;
    let url = format["url"].as_str().unwrap_or_default().to_owned();
    let content_length = format["contentLength"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok());
    let duration = format["approxDurationMs"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_millis);
    let loudness_db = loudness_of(&response, Some(format)).map(|v| v as f32);
    let mime = format["mimeType"]
        .as_str()
        .and_then(|m| m.split(';').next())
        .unwrap_or("audio/mp4")
        .trim()
        .to_owned();
    let kbps = format["averageBitrate"]
        .as_u64()
        .or_else(|| format["bitrate"].as_u64())
        .map(|bps| (bps as f64 / 1000.0).round() as u32);
    let headers = vec![("User-Agent".to_owned(), client.user_agent.clone())];
    let content_length = match content_length {
        Some(len) => len,
        None => probe_length(api, &url, &headers).await?,
    };
    verify(api, &url, &headers, content_length).await?;
    Ok(StreamInfo {
        url,
        headers,
        content_length,
        duration,
        loudness_db,
        quality: describe(&mime, kbps),
        mime,
        kbps,
        cache_key: None,
    })
}

/// Opus at about 130 kb/s (itag 251) first, then AAC at 128 (itag 140),
/// then whatever else of either has an address.
fn pick_format(formats: &[Value]) -> Option<&Value> {
    let playable = |f: &&Value| {
        f["url"].is_string()
            && f["mimeType"].as_str().is_some_and(|m| {
                m.starts_with("audio/webm") && m.contains("opus")
                    || m.starts_with("audio/mp4") && m.contains("mp4a")
            })
    };
    let itag = |n: u64| formats.iter().filter(playable).find(|f| f["itag"].as_u64() == Some(n));
    itag(251).or_else(|| itag(140)).or_else(|| {
        formats
            .iter()
            .filter(playable)
            .max_by_key(|f| f["bitrate"].as_u64().unwrap_or(0))
    })
}

/// YouTube Music's loudness target, as its web app reports it. The other
/// clients measure against -14, so the track's absolute loudness is used
/// where the answer has it, to treat every stream alike.
const TARGET_LKFS: f64 = -7.0;

/// How far the track is above the target, in dB.
fn loudness_of(response: &Value, format: Option<&Value>) -> Option<f64> {
    let config = response.pointer("/playerConfig/audioConfig");
    config
        .and_then(|c| c["trackAbsoluteLoudnessLkfs"].as_f64())
        .map(|lkfs| lkfs - TARGET_LKFS)
        .or_else(|| format.and_then(|f| f["loudnessDb"].as_f64()))
        .or_else(|| config.and_then(|c| c["loudnessDb"].as_f64()))
}

/// "Opus 143 kb/s", for the interface.
fn describe(mime: &str, kbps: Option<u32>) -> String {
    let codec = if mime.contains("webm") { "Opus" } else { "AAC" };
    match kbps {
        Some(kbps) => format!("{codec} {kbps} kb/s"),
        None => codec.to_owned(),
    }
}

/// Asks for the last kilobyte: an address that only serves the first part
/// of the file fails here rather than mid-song.
async fn verify(api: &Innertube, url: &str, headers: &[(String, String)], len: u64) -> Result<()> {
    let start = len.saturating_sub(1024);
    let mut request = api
        .http
        .get(url)
        .timeout(Duration::from_secs(8))
        .header("Range", format!("bytes={start}-{}", len.saturating_sub(1)));
    for (name, value) in headers {
        request = request.header(name, value);
    }
    // Stream addresses carry the client's IP: keep them out of errors.
    let response = request.send().await.map_err(reqwest::Error::without_url)?;
    if !response.status().is_success() {
        bail!("stream refused: HTTP {}", response.status());
    }
    Ok(())
}

async fn probe_length(api: &Innertube, url: &str, headers: &[(String, String)]) -> Result<u64> {
    let mut request = api
        .http
        .get(url)
        .timeout(Duration::from_secs(8))
        .header("Range", "bytes=0-0");
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request.send().await.map_err(reqwest::Error::without_url)?;
    let range = response
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow!("no content range"))?;
    range
        .rsplit('/')
        .next()
        .and_then(|total| total.parse().ok())
        .ok_or_else(|| anyhow!("bad content range {range}"))
}

async fn ytdlp(
    api: &Innertube,
    path: &Path,
    video_id: &str,
    auth: Option<&Auth>,
    quality: Quality,
    scratch: &Path,
) -> Result<StreamInfo> {
    let cookies = auth.map(|auth| CookieFile::write(scratch, auth)).transpose()?;
    let format = match quality {
        Quality::Premium => "774/141/251/140/bestaudio",
        Quality::Standard => "251/140/bestaudio[acodec=opus]/bestaudio[acodec^=mp4a]",
    };
    // Signed in, the web music client alone is enough and the quickest;
    // should it fail, yt-dlp's own choice of clients.
    let attempts: &[Option<&str>] = if cookies.is_some() {
        &[Some("youtube:player_client=web_music"), None]
    } else {
        &[None]
    };
    let mut last_error = None;
    let mut found = None;
    for extractor_args in attempts {
        let run = run_ytdlp(path, video_id, format, cookies.as_ref().map(|c| c.path.as_path()), *extractor_args);
        // The loudness yt-dlp leaves out, from the web player's answer.
        let (output, loudness) = tokio::join!(run, web_loudness(api, video_id, auth.is_some()));
        match output {
            Ok(json) => {
                found = Some((json, loudness));
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    drop(cookies);
    let Some((json, loudness_db)) = found else {
        return Err(last_error.unwrap_or_else(|| anyhow!("failed")));
    };
    let url = json["url"]
        .as_str()
        .ok_or_else(|| anyhow!("no url"))?
        .to_owned();
    let headers: Vec<(String, String)> = json["http_headers"]
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let content_length = match json["filesize"].as_u64() {
        Some(len) => len,
        None => probe_length(api, &url, &headers).await?,
    };
    if verify(api, &url, &headers, content_length).await.is_err() {
        bail!("its stream is refused past the start; update yt-dlp (yt-dlp -U, or your package manager)");
    }
    let duration = json["duration"].as_f64().map(Duration::from_secs_f64);
    let mime = if json["ext"].as_str() == Some("webm") { "audio/webm" } else { "audio/mp4" }.to_owned();
    let kbps = json["abr"]
        .as_f64()
        .or_else(|| json["tbr"].as_f64())
        .map(|k| k.round() as u32);
    log::info!(
        "yt-dlp chose format {} ({})",
        json["format_id"].as_str().unwrap_or("?"),
        describe(&mime, kbps)
    );
    Ok(StreamInfo {
        url,
        headers,
        content_length,
        duration,
        loudness_db,
        quality: describe(&mime, kbps),
        mime,
        kbps,
        cache_key: None,
    })
}

async fn run_ytdlp(
    path: &Path,
    video_id: &str,
    format: &str,
    cookies: Option<&Path>,
    extractor_args: Option<&str>,
) -> Result<Value> {
    let mut command = tokio::process::Command::new(path);
    command
        .arg("-f")
        .arg(format)
        .arg("-j")
        .arg("--no-playlist")
        .arg("--no-warnings")
        .arg("--no-progress");
    if let Some(cookies) = cookies {
        command.arg("--cookies").arg(cookies);
    }
    if let Some(args) = extractor_args {
        command.arg("--extractor-args").arg(args);
    }
    // yt-dlp needs a JavaScript runtime for YouTube; it only looks for Deno
    // unless told otherwise.
    if find_on_path("deno.exe").is_none() && find_on_path("node.exe").is_some() {
        command.arg("--js-runtimes").arg("node");
    }
    command
        .arg(format!("https://music.youtube.com/watch?v={video_id}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console window flashing up from a windowed app.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .context("yt-dlp timed out")??;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", stderr.lines().last().unwrap_or("failed"));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// The track's loudness from the web player's own answer, signed in when
/// the stream is the account's. Best effort: a short wait, then none.
async fn web_loudness(api: &Innertube, video_id: &str, signed_in: bool) -> Option<f32> {
    if !signed_in {
        return None;
    }
    let request = api.post("player", json!({ "videoId": video_id }));
    let response = tokio::time::timeout(Duration::from_secs(4), request).await.ok()?.ok()?;
    loudness_of(&response, None).map(|v| v as f32)
}

/// The account's cookies as a Netscape cookies.txt, which is how yt-dlp
/// takes them, for as long as one run of yt-dlp. Removed when dropped.
struct CookieFile {
    path: PathBuf,
}

impl CookieFile {
    fn write(dir: &Path, auth: &Auth) -> Result<Self> {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!(
            "yt-dlp-cookies-{}-{}.txt",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        let expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 365 * 24 * 3600;
        let mut text = String::from("# Netscape HTTP Cookie File\n");
        for (name, value) in &auth.cookies {
            text.push_str(&format!(".youtube.com\tTRUE\t/\tTRUE\t{expires}\t{name}\t{value}\n"));
        }
        std::fs::write(&path, text).context("cannot write yt-dlp's cookie file")?;
        Ok(Self { path })
    }
}

impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Removes cookie files a crash left behind.
pub fn clean_scratch(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("yt-dlp-cookies-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_comes_first() {
        let formats = vec![
            json!({"itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "url": "a", "bitrate": 130000}),
            json!({"itag": 251, "mimeType": "audio/webm; codecs=\"opus\"", "url": "b", "bitrate": 140000}),
            json!({"itag": 248, "mimeType": "video/webm; codecs=\"vp9\"", "url": "c", "bitrate": 900000}),
        ];
        assert_eq!(pick_format(&formats).and_then(|f| f["itag"].as_u64()), Some(251));
        assert_eq!(pick_format(&formats[..1]).and_then(|f| f["itag"].as_u64()), Some(140));
        assert_eq!(describe("audio/webm", Some(143)), "Opus 143 kb/s");
    }
}
