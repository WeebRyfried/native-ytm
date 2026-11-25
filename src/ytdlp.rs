//! yt-dlp as the app keeps it: found on PATH or as the app's own copy,
//! installed in a click, kept up to date, and asked for the YouTube sign-in
//! a browser holds (`--cookies-from-browser`).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

/// Where the app's own copy goes, set once at start.
static MANAGED: OnceLock<PathBuf> = OnceLock::new();

pub fn set_managed_dir(dir: &Path) {
    let _ = MANAGED.set(dir.join("yt-dlp.exe"));
}

/// The app's own copy's place, whether or not it is there yet.
pub fn managed() -> Option<&'static Path> {
    MANAGED.get().map(PathBuf::as_path)
}

/// yt-dlp: on PATH, else the app's own copy.
pub fn find() -> Option<PathBuf> {
    crate::stream::find_on_path("yt-dlp.exe")
        .or_else(|| crate::stream::find_on_path("yt-dlp"))
        .or_else(|| managed().filter(|p| p.is_file()).map(Path::to_path_buf))
}

fn command(path: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(path);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The version installed, such as "2026.10.26".
pub async fn version(path: &Path) -> Result<String> {
    let output = tokio::time::timeout(Duration::from_secs(30), command(path).arg("--version").output())
        .await
        .context("yt-dlp took too long")??;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() {
        bail!("yt-dlp did not say its version");
    }
    Ok(text)
}

const RELEASES: &str = "https://github.com/yt-dlp/yt-dlp/releases";

/// The newest release's version: its latest page leads to its tag.
pub async fn latest(http: &reqwest::Client) -> Result<String> {
    let response = http
        .head(format!("{RELEASES}/latest"))
        .header("User-Agent", concat!("native-ytm/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    let tag = response.url().path().rsplit('/').next().unwrap_or_default().to_owned();
    if tag.is_empty() || tag == "latest" {
        bail!("GitHub did not say which version is newest");
    }
    Ok(tag)
}

/// Whether `latest` is newer than `have` (both dotted dates).
pub fn newer(latest: &str, have: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.trim().split(['.', '-']).filter_map(|p| p.parse().ok()).collect() };
    parts(latest) > parts(have)
}

/// Downloads the newest yt-dlp.exe into the app's own folder, checked
/// against the release's SHA-256 list.
pub async fn install(http: &reqwest::Client) -> Result<PathBuf> {
    let target = managed().ok_or_else(|| anyhow!("no folder for yt-dlp"))?.to_path_buf();
    let tag = latest(http).await?;
    let base = format!("{RELEASES}/download/{tag}");
    let agent = concat!("native-ytm/", env!("CARGO_PKG_VERSION"));
    let sums = http
        .get(format!("{base}/SHA2-256SUMS"))
        .header("User-Agent", agent)
        .timeout(Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let wanted = sums
        .lines()
        .find_map(|line| {
            let (hash, name) = line.trim().split_once(char::is_whitespace)?;
            (name.trim().trim_start_matches('*') == "yt-dlp.exe").then(|| hash.to_ascii_lowercase())
        })
        .ok_or_else(|| anyhow!("the release lists no checksum for yt-dlp.exe"))?;
    let bytes = http
        .get(format!("{base}/yt-dlp.exe"))
        .header("User-Agent", agent)
        .timeout(Duration::from_secs(600))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let got = format!("{:x}", Sha256::digest(&bytes));
    if got != wanted {
        bail!("the download did not match its checksum");
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = target.with_extension("exe.part");
    std::fs::write(&part, &bytes)?;
    std::fs::rename(&part, &target)?;
    log::info!("yt-dlp {tag} installed at {}", target.display());
    Ok(target)
}

/// `yt-dlp -U`: what it said last ("yt-dlp is up to date", "Updated
/// yt-dlp to ...").
pub async fn update(path: &Path) -> Result<String> {
    let output = tokio::time::timeout(Duration::from_secs(300), command(path).arg("-U").output())
        .await
        .context("the update took too long")??;
    let text = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let last = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .next_back()
        .unwrap_or("done")
        .trim_start_matches("ERROR: ")
        .to_owned();
    if !output.status.success() {
        bail!("{last}");
    }
    Ok(last)
}

/// The browsers yt-dlp reads, by its name for each and ours, in the order
/// tried: Firefox first, as Chrome and Edge now lock their cookies away
/// from other programs.
pub const BROWSERS: [(&str, &str); 7] = [
    ("firefox", "Firefox"),
    ("chrome", "Chrome"),
    ("edge", "Edge"),
    ("brave", "Brave"),
    ("vivaldi", "Vivaldi"),
    ("opera", "Opera"),
    ("chromium", "Chromium"),
];

pub fn browser_name(id: &str) -> &'static str {
    BROWSERS.iter().find(|(b, _)| *b == id).map_or("your browser", |(_, name)| name)
}

/// The browsers on this PC, by where each keeps its profile.
pub fn installed_browsers() -> Vec<&'static str> {
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let place = |id: &str| -> Option<PathBuf> {
        Some(match id {
            "firefox" => roaming.as_ref()?.join("Mozilla").join("Firefox").join("Profiles"),
            "chrome" => local.as_ref()?.join("Google").join("Chrome").join("User Data"),
            "edge" => local.as_ref()?.join("Microsoft").join("Edge").join("User Data"),
            "brave" => local.as_ref()?.join("BraveSoftware").join("Brave-Browser").join("User Data"),
            "vivaldi" => local.as_ref()?.join("Vivaldi").join("User Data"),
            "opera" => roaming.as_ref()?.join("Opera Software").join("Opera Stable"),
            "chromium" => local.as_ref()?.join("Chromium").join("User Data"),
            _ => return None,
        })
    };
    BROWSERS.iter().map(|(id, _)| *id).filter(|id| place(id).is_some_and(|p| p.is_dir())).collect()
}

/// The YouTube sign-in from `browser` (or, with none, the first browser
/// that has one): which browser it came from, and its cookies as a cookie
/// header. The error says, for each browser tried, why not.
pub async fn cookies_from_browser(path: &Path, browser: Option<&str>, scratch: &Path) -> Result<(&'static str, String), String> {
    let tried: Vec<&'static str> = match browser.filter(|b| !b.is_empty()) {
        Some(b) => BROWSERS.iter().map(|(id, _)| *id).filter(|id| *id == b).collect(),
        None => installed_browsers(),
    };
    if tried.is_empty() {
        return Err("No supported browser found on this PC".into());
    }
    let mut reasons = Vec::new();
    for id in tried {
        match from_one(path, id, scratch).await {
            Ok(header) => return Ok((browser_name(id), header)),
            Err(reason) => {
                log::info!("no sign-in from {id}: {reason:#}");
                reasons.push(format!("{}: {reason}", browser_name(id)));
            }
        }
    }
    Err(reasons.join(". "))
}

async fn from_one(path: &Path, browser: &str, scratch: &Path) -> Result<String> {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    std::fs::create_dir_all(scratch)?;
    // Named as the stream's cookie files are, so a crash's leftovers are
    // cleared the same way.
    let file = scratch.join(format!("yt-dlp-cookies-browser-{}-{}.txt", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
    // With no address yt-dlp stops at once, but still writes the cookies
    // it read to the file: no request to YouTube is made.
    let run = command(path)
        .arg("--cookies-from-browser")
        .arg(browser)
        .arg("--cookies")
        .arg(&file)
        .arg("--no-warnings")
        .output();
    let output = tokio::time::timeout(Duration::from_secs(90), run).await;
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let _ = std::fs::remove_file(&file);
    let output = output.map_err(|_| anyhow!("took too long"))??;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let cookies = crate::api::client::parse_cookies(&text);
    let auth = crate::api::client::Auth { cookies, auth_user: 0 };
    if auth.is_usable() {
        return Ok(auth.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; "));
    }
    let error = stderr.lines().rev().find(|l| l.starts_with("ERROR:")).unwrap_or_default().to_ascii_lowercase();
    if error.contains("dpapi") || error.contains("decrypt") || error.contains("app-bound") {
        bail!("it locks its sign-in away from other apps");
    }
    if error.contains("could not copy") || error.contains("permission denied") || error.contains("locked") {
        bail!("close it, then try again");
    }
    if error.contains("could not find") || error.contains("not found") {
        bail!("no profile found");
    }
    if !error.is_empty() && text.is_empty() {
        bail!("{}", error.trim_start_matches("error:").trim());
    }
    bail!("not signed in to YouTube Music")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_date() {
        assert!(newer("2026.11.02", "2026.10.26"));
        assert!(!newer("2026.10.26", "2026.10.26"));
        assert!(newer("2026.10.26.1", "2026.10.26"));
        assert!(!newer("2025.12.31", "2026.01.01"));
    }

    /// Downloads yt-dlp for real: `cargo test -- --ignored installs`.
    #[test]
    #[ignore]
    fn installs_and_runs() {
        let dir = std::env::temp_dir().join(format!("native-ytm-ytdlp-{}", std::process::id()));
        set_managed_dir(&dir);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let http = reqwest::Client::new();
        let path = runtime.block_on(install(&http)).expect("installed");
        let version = runtime.block_on(super::version(&path)).expect("runs");
        assert!(version.starts_with("20"), "{version}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
