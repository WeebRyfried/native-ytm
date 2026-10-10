//! Finding a playable audio stream for a video id.
//!
//! First natively: the player request as a client YouTube still serves
//! direct addresses to (no JavaScript challenge, no proof-of-origin token),
//! checked with a request near the file's end, since some clients' addresses
//! only work for the first megabyte. Then, if yt-dlp is installed, through
//! yt-dlp, which keeps up with YouTube's changes. Only AAC in MP4 is taken,
//! the format the decoder plays.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::api::client::{Innertube, PlayerClient};
use crate::player::StreamInfo;

/// Where the stream came from, for the log and Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Native,
    YtDlp,
}

pub struct Resolver {
    pub clients: Vec<PlayerClient>,
    pub ytdlp: Option<PathBuf>,
    pub use_ytdlp: bool,
}

impl Resolver {
    pub fn new(ytdlp_override: Option<PathBuf>, use_ytdlp: bool) -> Self {
        Self {
            clients: PlayerClient::defaults(),
            ytdlp: ytdlp_override.or_else(|| find_on_path("yt-dlp.exe").or_else(|| find_on_path("yt-dlp"))),
            use_ytdlp,
        }
    }

    pub async fn resolve(&self, api: &Innertube, video_id: &str) -> Result<(StreamInfo, Via)> {
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
        if self.use_ytdlp {
            if let Some(path) = &self.ytdlp {
                match ytdlp(api, path, video_id).await {
                    Ok(stream) => return Ok((stream, Via::YtDlp)),
                    Err(error) => {
                        log::warn!("yt-dlp could not resolve {video_id}: {error:#}");
                        errors.push(format!("yt-dlp: {error}"));
                    }
                }
            } else {
                errors.push("yt-dlp: not installed".into());
            }
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
    let format = pick_format(formats).ok_or_else(|| anyhow!("no AAC audio with an address"))?;
    let url = format["url"].as_str().unwrap_or_default().to_owned();
    let content_length = format["contentLength"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok());
    let duration = format["approxDurationMs"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_millis);
    // Per format, in dB above YouTube's target; older answers kept it in the
    // audio config.
    let loudness_db = format["loudnessDb"]
        .as_f64()
        .or_else(|| {
            response
                .pointer("/playerConfig/audioConfig/loudnessDb")
                .and_then(Value::as_f64)
        })
        .map(|v| v as f32);
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
    })
}

/// AAC LC at 128 kb/s (itag 140) first, then any other AAC in MP4.
fn pick_format(formats: &[Value]) -> Option<&Value> {
    let aac = |f: &&Value| {
        f["mimeType"]
            .as_str()
            .is_some_and(|m| m.starts_with("audio/mp4") && m.contains("mp4a"))
            && f["url"].is_string()
    };
    formats
        .iter()
        .filter(aac)
        .find(|f| f["itag"].as_u64() == Some(140))
        .or_else(|| {
            formats
                .iter()
                .filter(aac)
                .max_by_key(|f| f["bitrate"].as_u64().unwrap_or(0))
        })
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

async fn ytdlp(api: &Innertube, path: &PathBuf, video_id: &str) -> Result<StreamInfo> {
    let mut command = tokio::process::Command::new(path);
    command
        .arg("-f")
        .arg("140/bestaudio[acodec^=mp4a]")
        .arg("-j")
        .arg("--no-playlist")
        .arg("--no-warnings")
        .arg("--no-progress");
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
    let json: Value = serde_json::from_slice(&output.stdout)?;
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
    Ok(StreamInfo {
        url,
        headers,
        content_length,
        duration,
        loudness_db: None,
    })
}

pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}
