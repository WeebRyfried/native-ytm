//! Updates: the newest release on GitHub, and moving to it in one click.
//!
//! The check asks GitHub's API for the latest release. When the repository
//! is private GitHub answers "not found"; the request then goes again with
//! the GitHub CLI's sign-in (`gh auth token`), if there is one. That token
//! is held in memory for the run, sent only to GitHub, and never written.
//!
//! An update downloads beside the running program and is checked against
//! the SHA-256 digest GitHub keeps for it. Windows lets a running program
//! be renamed, so the old one steps aside, the new one takes its name and
//! starts, and this one quits. The new one waits for the old to be gone,
//! then carries on with the queue where it was.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use sha2::{Digest, Sha256};

/// Where releases are published.
pub const REPO: &str = "WeebRyfried/native-ytm";

/// The program's file in a release.
const ASSET: &str = "native-ytm.exe";

/// How often a running app looks again.
const EVERY: Duration = Duration::from_secs(12 * 3600);

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// "0.2.4".
    pub version: String,
    /// The release's first line, for people.
    pub summary: String,
    /// Its page on GitHub.
    pub page: String,
    /// The program's address through the API (works for private
    /// repositories), and for browsers.
    api_url: String,
    download_url: String,
    size: u64,
    /// "sha256:…", as GitHub gives it.
    digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Checking,
    /// The newest release is the one running.
    UpToDate,
    Available(Release),
    /// Downloading, from 0 to 1.
    Downloading(Release, f32),
    /// The new program is in place: restart into it.
    Ready(PathBuf),
    Failed(String),
}

pub struct Updater {
    state: Arc<Mutex<State>>,
    runtime: tokio::runtime::Handle,
    http: reqwest::Client,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// The GitHub CLI's token, once a private repository needed it.
    token: Arc<Mutex<Option<String>>>,
    last_check: Option<Instant>,
}

impl Updater {
    pub fn new(runtime: tokio::runtime::Handle, http: reqwest::Client, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::Idle)),
            runtime,
            http,
            wake,
            token: Arc::default(),
            last_check: None,
        }
    }

    pub fn state(&self) -> State {
        self.state.lock().clone()
    }

    /// The version running: `NATIVE_YTM_VERSION` stands in for it in tests
    /// (dev), to be offered the release already out.
    pub fn current() -> String {
        std::env::var("NATIVE_YTM_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned())
    }

    /// Looks now, or when automatic and the last look is old enough.
    pub fn check(&mut self, automatic: bool) {
        if automatic && self.last_check.is_some_and(|at| at.elapsed() < EVERY) {
            return;
        }
        {
            let mut state = self.state.lock();
            if matches!(*state, State::Checking | State::Downloading(..) | State::Ready(_)) {
                return;
            }
            // An automatic look leaves a failure from a manual one alone.
            if automatic && matches!(*state, State::Failed(_)) {
                *state = State::Idle;
            }
            *state = State::Checking;
        }
        self.last_check = Some(Instant::now());
        let (state, http, token, wake) = (self.state.clone(), self.http.clone(), self.token.clone(), self.wake.clone());
        self.runtime.spawn(async move {
            let result = latest(&http, &token).await;
            *state.lock() = match result {
                Ok(release) if newer(&release.version, &Updater::current()) => {
                    log::info!("update: {} is out", release.version);
                    State::Available(release)
                }
                Ok(_) => State::UpToDate,
                Err(error) => {
                    log::warn!("update check: {error}");
                    if automatic { State::Idle } else { State::Failed(error) }
                }
            };
            wake();
        });
    }

    /// Downloads the release, checks it, and puts it in this program's
    /// place; the state turns Ready when done.
    pub fn install(&mut self) {
        let State::Available(release) = self.state() else {
            return;
        };
        *self.state.lock() = State::Downloading(release.clone(), 0.0);
        let (state, http, token, wake) = (self.state.clone(), self.http.clone(), self.token.clone(), self.wake.clone());
        self.runtime.spawn(async move {
            let result = download_and_swap(&http, &token, &release, &state, &*wake).await;
            *state.lock() = match result {
                Ok(exe) => State::Ready(exe),
                Err(error) => {
                    log::warn!("update: {error}");
                    State::Failed(error)
                }
            };
            wake();
        });
    }

    /// Forgets a failure, so the next look starts afresh.
    pub fn dismiss_failure(&mut self) {
        let mut state = self.state.lock();
        if matches!(*state, State::Failed(_)) {
            *state = State::Idle;
        }
    }
}

/// "0.2.10" is newer than "0.2.9".
pub fn newer(candidate: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim()
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .take(3)
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parts(candidate) > parts(current)
}

/// A request to GitHub. The token goes to api.github.com alone: reqwest
/// drops it when a download redirects to GitHub's file storage.
async fn get(http: &reqwest::Client, url: &str, token: Option<&str>, accept: &str) -> reqwest::Result<reqwest::Response> {
    let mut request = http
        .get(url)
        .header("User-Agent", concat!("native-ytm/", env!("CARGO_PKG_VERSION")))
        .header("Accept", accept)
        .header("X-GitHub-Api-Version", "2022-11-28");
    if accept != "application/octet-stream" {
        // A whole download takes longer; it is watched for stalls instead.
        request = request.timeout(Duration::from_secs(30));
    }
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    request.send().await
}

/// The latest release, asked for openly first, then with the GitHub CLI's
/// sign-in if the repository turns out to be private.
async fn latest(http: &reqwest::Client, token: &Mutex<Option<String>>) -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let held = token.lock().clone();
    let mut response = get(http, &url, held.as_deref(), "application/vnd.github+json").await.map_err(|e| offline(&e))?;
    if matches!(response.status().as_u16(), 401 | 403 | 404) && held.is_none() {
        if let Some(found) = cli_token().await {
            response = get(http, &url, Some(&found), "application/vnd.github+json").await.map_err(|e| offline(&e))?;
            if response.status().is_success() {
                *token.lock() = Some(found);
            }
        } else if response.status().as_u16() == 404 {
            return Err("The releases are private. Sign in to the GitHub CLI (gh auth login) to get updates.".into());
        }
    }
    if !response.status().is_success() {
        return Err(format!("GitHub answered {}", response.status()));
    }
    let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    let version = body["tag_name"].as_str().unwrap_or_default().trim_start_matches('v').to_owned();
    let asset = body["assets"]
        .as_array()
        .and_then(|assets| assets.iter().find(|a| a["name"].as_str() == Some(ASSET)))
        .ok_or("The latest release has no program to download")?;
    let summary = body["body"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or_default()
        .to_owned();
    Ok(Release {
        version,
        summary,
        page: body["html_url"].as_str().unwrap_or_default().to_owned(),
        api_url: asset["url"].as_str().unwrap_or_default().to_owned(),
        download_url: asset["browser_download_url"].as_str().unwrap_or_default().to_owned(),
        size: asset["size"].as_u64().unwrap_or(0),
        digest: asset["digest"].as_str().map(str::to_owned),
    })
}

fn offline(error: &reqwest::Error) -> String {
    if error.is_connect() || error.is_timeout() {
        "Cannot reach GitHub. Check the connection.".into()
    } else {
        error.to_string()
    }
}

/// The GitHub CLI's token, if it is installed and signed in.
async fn cli_token() -> Option<String> {
    let mut command = tokio::process::Command::new("gh");
    command
        .args(["auth", "token"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = tokio::time::timeout(Duration::from_secs(10), command.output()).await.ok()?.ok()?;
    let token = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && !token.is_empty()).then_some(token)
}

async fn download_and_swap(
    http: &reqwest::Client,
    token: &Mutex<Option<String>>,
    release: &Release,
    state: &Mutex<State>,
    wake: &(dyn Fn() + Send + Sync),
) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let partial = exe.with_extension("exe.download");
    let held = token.lock().clone();
    let response = match &held {
        // A private repository's files come through the API.
        Some(token) => get(http, &release.api_url, Some(token), "application/octet-stream").await,
        None => get(http, &release.download_url, None, "application/octet-stream").await,
    }
    .map_err(|e| offline(&e))?;
    if !response.status().is_success() {
        return Err(format!("The download failed: GitHub answered {}", response.status()));
    }
    let total = response.content_length().unwrap_or(release.size).max(1);
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|e| format!("Cannot write beside the program ({e}). Download it from the release page instead."))?;
    let mut hasher = Sha256::new();
    let mut got = 0u64;
    let mut response = response;
    let mut last_note = Instant::now();
    loop {
        let chunk = match tokio::time::timeout(Duration::from_secs(30), response.chunk()).await {
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                let _ = tokio::fs::remove_file(&partial).await;
                return Err(offline(&error));
            }
            Err(_) => {
                let _ = tokio::fs::remove_file(&partial).await;
                return Err("The download stalled".into());
            }
        };
        hasher.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await.map_err(|e| e.to_string())?;
        got += chunk.len() as u64;
        if last_note.elapsed() > Duration::from_millis(120) {
            last_note = Instant::now();
            *state.lock() = State::Downloading(release.clone(), (got as f32 / total as f32).min(1.0));
            wake();
        }
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await.map_err(|e| e.to_string())?;
    drop(file);
    let digest = format!("sha256:{:x}", hasher.finalize());
    let size_ok = release.size == 0 || got == release.size;
    let digest_ok = release.digest.as_ref().is_none_or(|d| d.eq_ignore_ascii_case(&digest));
    if !size_ok || !digest_ok || got < 1024 * 1024 {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err("The download did not match what GitHub published, so it was not used".into());
    }
    swap(&exe, &partial).map_err(|e| format!("Cannot replace the program ({e})"))?;
    log::info!("update: {} is in place ({got} bytes, {digest})", release.version);
    Ok(exe)
}

/// The old program steps aside under another name, and the new one takes
/// its place.
fn swap(exe: &Path, new: &Path) -> std::io::Result<()> {
    let mut old = exe.with_extension("exe.old");
    if old.exists() && std::fs::remove_file(&old).is_err() {
        old = exe.with_extension(format!("exe.old-{}", std::process::id()));
    }
    std::fs::rename(exe, &old)?;
    if let Err(error) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(error);
    }
    Ok(())
}

/// After an update: once the old program has gone, its file goes too.
pub fn clean_up_after(old_pid: Option<u32>) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::thread::Builder::new().name("update-clean".into()).spawn(move || {
        if let Some(pid) = old_pid {
            crate::platform::wait_for_exit(pid, Duration::from_secs(20));
        }
        let Some(dir) = exe.parent() else { return };
        let Some(name) = exe.file_name().map(|n| n.to_string_lossy().into_owned()) else { return };
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if file.starts_with(&format!("{name}.old")) || file == format!("{name}.download") {
                for _ in 0..10 {
                    if std::fs::remove_file(entry.path()).is_ok() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("0.2.10", "0.2.9"));
        assert!(newer("v0.3.0", "0.2.9"));
        assert!(newer("1.0.0", "0.9.99"));
        assert!(!newer("0.2.3", "0.2.3"));
        assert!(!newer("0.2.2", "0.2.3"));
    }

    #[test]
    fn the_old_program_steps_aside() {
        let dir = std::env::temp_dir().join(format!("native-ytm-swap-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (exe, new) = (dir.join("native-ytm.exe"), dir.join("native-ytm.exe.download"));
        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        swap(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.join("native-ytm.exe.old")).unwrap(), b"old");
        assert!(!new.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
