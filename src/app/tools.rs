//! yt-dlp for the app: installing it, keeping it up to date, and signing
//! in with the YouTube sign-in a browser holds.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{App, SignInDialog};
use crate::backend::Request;
use crate::model::ToastKind;

/// What yt-dlp is busy with, for the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Installing,
    Checking,
    Updating,
}

enum Event {
    Installed(Result<std::path::PathBuf, String>),
    Checked { version: Option<String>, latest: Result<String, String>, auto: bool },
    Updated(Result<String, String>),
    Cookies { auto: bool, result: Result<(&'static str, String), String> },
}

pub struct Tools {
    /// The version installed, and the newest, as last checked.
    pub version: Option<String>,
    pub latest: Option<String>,
    pub job: Option<Job>,
    /// A browser is being asked for its sign-in.
    pub signing: bool,
    /// The sign-in being asked for was cancelled: its answer is let go.
    cancelled: bool,
    /// What the last check or update said, and whether it went wrong.
    pub said: Option<(String, bool)>,
    /// Signed out by hand: not signed back in from the browser by itself
    /// until the app starts again.
    pub paused: bool,
    /// Sign in from the browser once yt-dlp is installed.
    sign_in_after_install: bool,
    /// The last time the browser was asked by itself, so a sign-in that
    /// YouTube turns down is not asked for again and again.
    auto_tried: Option<Instant>,
    /// When to check for a yt-dlp update by itself.
    check_due: Option<Instant>,
    done: (Sender<Event>, Receiver<Event>),
}

impl Tools {
    /// The sign-in from a browser under way is not wanted any more.
    pub fn cancel_sign_in(&mut self) {
        if self.signing {
            self.cancelled = true;
        }
    }
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            version: None,
            latest: None,
            job: None,
            signing: false,
            cancelled: false,
            said: None,
            paused: false,
            sign_in_after_install: false,
            auto_tried: None,
            check_due: Some(Instant::now() + Duration::from_secs(20)),
            done: channel(),
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl App {
    /// Whether the offer to install yt-dlp shows.
    pub fn ytdlp_prompt_shown(&self) -> bool {
        self.settings.ytdlp_prompt && self.backend.ytdlp_found().is_none() && self.settings.browser_sign_in.is_some()
    }

    /// The first start's question, once asked nowhere else.
    pub fn browser_question_shown(&self) -> bool {
        self.settings.browser_sign_in.is_none() && self.whats_new.is_none() && self.sign_in.as_ref().is_none_or(|d| d.silent)
    }

    fn send_tool(&self) -> (Sender<Event>, impl Fn() + Send + Sync + 'static) {
        let waker = self.waker.clone();
        (self.tools.done.0.clone(), move || waker.wake())
    }

    pub(super) fn tools_tick(&mut self) {
        while let Ok(event) = self.tools.done.1.try_recv() {
            self.on_tool(event);
        }
        if self.tools.check_due.is_some_and(|at| Instant::now() >= at) {
            self.tools.check_due = None;
            let day = 24 * 3600;
            if self.settings.ytdlp_auto_update
                && self.online()
                && self.backend.ytdlp_found().is_some()
                && unix_now().saturating_sub(self.settings.ytdlp_checked) > day
            {
                self.check_ytdlp(true);
            }
        }
    }

    /// Signed in from the browser by itself, when that is on: at the
    /// start, and when a sign-in ends.
    pub(super) fn auto_sign_in(&mut self) {
        if self.settings.browser_sign_in != Some(true)
            || self.signed_in
            || self.tools.paused
            || self.tools.signing
            || self.backend.ytdlp_found().is_none()
            || self.tools.auto_tried.is_some_and(|at| at.elapsed() < Duration::from_secs(600))
        {
            return;
        }
        self.tools.auto_tried = Some(Instant::now());
        self.sign_in_from_browser(None, true);
    }

    pub(super) fn sign_in_from_browser(&mut self, browser: Option<String>, auto: bool) {
        let Some(path) = self.backend.ytdlp_found() else {
            if !auto {
                self.tools.sign_in_after_install = true;
                self.install_ytdlp();
            }
            return;
        };
        if self.tools.signing {
            return;
        }
        if let Some(dialog) = &mut self.sign_in {
            dialog.error = None;
            dialog.busy = true;
        }
        self.tools.signing = true;
        self.tools.cancelled = false;
        let browser = browser.unwrap_or_else(|| self.settings.sign_in_browser.clone());
        let scratch = self.paths.scratch();
        let (tx, wake) = self.send_tool();
        self.backend.runtime.spawn(async move {
            let browser = (!browser.is_empty()).then_some(browser);
            let result = crate::ytdlp::cookies_from_browser(&path, browser.as_deref(), &scratch).await;
            let _ = tx.send(Event::Cookies { auto, result });
            wake();
        });
    }

    pub(super) fn install_ytdlp(&mut self) {
        if self.tools.job == Some(Job::Installing) {
            return;
        }
        self.tools.job = Some(Job::Installing);
        self.tools.said = None;
        let http = self.backend.http.clone();
        let (tx, wake) = self.send_tool();
        self.backend.runtime.spawn(async move {
            let result = crate::ytdlp::install(&http).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Installed(result));
            wake();
        });
    }

    /// The version installed and the newest; `auto` updates when there is
    /// a newer one.
    pub(super) fn check_ytdlp(&mut self, auto: bool) {
        let Some(path) = self.backend.ytdlp_found() else {
            return;
        };
        if self.tools.job.is_some() {
            return;
        }
        self.tools.job = Some(Job::Checking);
        self.tools.said = None;
        let http = self.backend.http.clone();
        let (tx, wake) = self.send_tool();
        self.backend.runtime.spawn(async move {
            let (version, latest) = tokio::join!(crate::ytdlp::version(&path), crate::ytdlp::latest(&http));
            let _ = tx.send(Event::Checked {
                version: version.ok(),
                latest: latest.map_err(|e| format!("{e:#}")),
                auto,
            });
            wake();
        });
    }

    pub(super) fn update_ytdlp(&mut self) {
        let Some(path) = self.backend.ytdlp_found() else {
            self.install_ytdlp();
            return;
        };
        if self.tools.job.is_some() {
            return;
        }
        self.tools.job = Some(Job::Updating);
        self.tools.said = None;
        let (tx, wake) = self.send_tool();
        self.backend.runtime.spawn(async move {
            let result = crate::ytdlp::update(&path).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Updated(result));
            wake();
        });
    }

    fn on_tool(&mut self, event: Event) {
        if matches!(event, Event::Cookies { .. }) {
            self.tools.signing = false;
        } else {
            self.tools.job = None;
        }
        match event {
            Event::Installed(Ok(path)) => {
                self.backend.send(Request::SetYtDlp { enabled: self.settings.use_ytdlp, path: Some(path.clone()) });
                self.js_runtime = None;
                self.toast("yt-dlp is installed", ToastKind::Info);
                self.settings.ytdlp_checked = unix_now();
                self.settings_dirty = Some(Instant::now());
                self.check_ytdlp(false);
                if std::mem::take(&mut self.tools.sign_in_after_install) {
                    self.sign_in_from_browser(None, false);
                } else if self.settings.browser_sign_in == Some(true) && !self.signed_in {
                    self.auto_sign_in();
                }
            }
            Event::Installed(Err(error)) => {
                log::warn!("yt-dlp install: {error}");
                self.tools.sign_in_after_install = false;
                self.tools.said = Some((format!("Couldn't install yt-dlp: {error}"), true));
                self.toast("Couldn't install yt-dlp. Check the connection and try again", ToastKind::Error);
                if let Some(dialog) = &mut self.sign_in {
                    dialog.busy = false;
                }
            }
            Event::Checked { version, latest, auto } => {
                self.tools.version = version.clone();
                self.settings.ytdlp_checked = unix_now();
                self.settings_dirty = Some(Instant::now());
                match latest {
                    Ok(latest) => {
                        let newer = version.as_deref().is_some_and(|v| crate::ytdlp::newer(&latest, v));
                        self.tools.latest = Some(latest.clone());
                        if newer && auto {
                            log::info!("yt-dlp {latest} is out: updating");
                            self.update_ytdlp();
                        } else if !newer && !auto {
                            self.tools.said = Some(("Up to date".into(), false));
                        }
                    }
                    Err(error) => {
                        log::info!("yt-dlp's newest version: {error}");
                        if !auto {
                            self.tools.said = Some(("Couldn't reach GitHub to check".into(), true));
                        }
                    }
                }
            }
            Event::Updated(result) => {
                match result {
                    Ok(said) => {
                        log::info!("yt-dlp -U: {said}");
                        if let Some(latest) = self.tools.latest.clone()
                            && (said.contains("Updated") || said.contains("up to date"))
                        {
                            self.tools.version = Some(latest);
                        }
                        self.tools.said = Some((said, false));
                    }
                    Err(error) => {
                        log::warn!("yt-dlp -U: {error}");
                        self.tools.said = Some((error, true));
                    }
                }
            }
            Event::Cookies { .. } if std::mem::take(&mut self.tools.cancelled) => {
                log::info!("a sign-in from the browser came after it was cancelled: let go");
            }
            Event::Cookies { auto, result } => match result {
                Ok((browser, header)) => {
                    if self.signed_in && self.sign_in.is_none() {
                        return;
                    }
                    let dialog = self.sign_in.get_or_insert_with(|| SignInDialog::silent());
                    dialog.from = Some(browser.to_owned());
                    dialog.silent |= auto;
                    self.actions.push(crate::model::Action::SignIn(header));
                }
                Err(reason) => {
                    log::info!("no sign-in from a browser: {reason}");
                    match &mut self.sign_in {
                        Some(dialog) if !dialog.silent => {
                            dialog.busy = false;
                            dialog.error = Some(format!("{reason}."));
                        }
                        _ => {
                            self.sign_in = None;
                            if !auto {
                                self.toast(format!("Couldn't sign in from a browser. {reason}."), ToastKind::Error);
                            }
                        }
                    }
                }
            },
        }
    }
}
