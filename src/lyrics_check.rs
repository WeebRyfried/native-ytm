//! Better Lyrics' lyrics server wants a Cloudflare Turnstile check passed
//! before it answers, and this passes it the way its browser extension
//! does: the server's own check page, in Edge's WebView2, whose widget
//! hands over a token once it is satisfied. Most times that happens unseen;
//! when Cloudflare wants a click, the window comes up for the person to
//! give it. Nothing here solves or makes up the check.
//!
//! The page runs in a process of its own (`native-ytm --lyrics-check DIR`),
//! so WebView2 and its window stay apart from the app's: it prints the
//! token, or why there is none, and ends.

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};

/// The argument that starts the check instead of the app, with the folder
/// WebView2 keeps its profile in after it.
pub const FLAG: &str = "--lyrics-check";

/// The server's check page.
const PAGE: &str = "https://lyrics.api.dacubeking.com/challenge";

/// How long the check may pass unseen before its window comes up.
const SHOW_AFTER: Duration = Duration::from_secs(5);
/// How long a shown check waits for a click before it is given up.
const GIVE_UP_AFTER: Duration = Duration::from_secs(120);

/// The page tells the frame it expects to be in, `window.parent`; at the
/// top that is the page itself, so its messages are passed on to the app.
const BRIDGE: &str = r#"
window.addEventListener('message', function (e) {
  if (e.data && typeof e.data.type === 'string' && e.data.type.indexOf('turnstile-') === 0) {
    window.ipc.postMessage(JSON.stringify(e.data));
  }
});
"#;

/// Runs the check in a process of its own: its token, or why not.
pub async fn obtain(profile: &Path) -> Result<String> {
    let exe = std::env::current_exe()?;
    let out = tokio::process::Command::new(exe)
        .arg(FLAG)
        .arg(profile)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().last().unwrap_or_default();
    if let Some(token) = line.strip_prefix("token ") {
        Ok(token.trim().to_owned())
    } else if let Some(why) = line.strip_prefix("failed ") {
        bail!("the lyrics check failed: {why}")
    } else {
        bail!("the lyrics check ended ({})", out.status)
    }
}

/// The check's own process: prints `token …` or `failed …`.
pub fn child(profile: Option<&str>) {
    match run(profile.map(std::path::PathBuf::from)) {
        Ok(token) => println!("token {token}"),
        Err(error) => println!("failed {error:#}"),
    }
}

#[cfg(not(windows))]
fn run(_profile: Option<std::path::PathBuf>) -> Result<String> {
    bail!("WebView2 is Windows' own")
}

#[cfg(windows)]
fn run(profile: Option<std::path::PathBuf>) -> Result<String> {
    use std::cell::RefCell;
    use std::time::Instant;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetSystemMetrics, IDC_ARROW, KillTimer, LoadCursorW,
        LoadIconW, MSG, PostQuitMessage, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, SetForegroundWindow, SetTimer, ShowWindow,
        TranslateMessage, WM_APP, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_CAPTION, WS_MINIMIZEBOX, WS_SYSMENU,
    };

    /// What the page said: a token, a failure, or that its token ran out
    /// and the widget should start again.
    enum Said {
        Token(String),
        Failed(String),
        Expired,
    }
    thread_local! {
        static SAID: RefCell<Vec<Said>> = const { RefCell::new(Vec::new()) };
        static CLOSED: RefCell<bool> = const { RefCell::new(false) };
    }
    /// A message to the loop that the page said something.
    const WM_SAID: u32 = WM_APP + 1;

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_DESTROY {
            // Closed by the person: no click coming.
            CLOSED.with(|c| *c.borrow_mut() = true);
            unsafe { PostQuitMessage(0) };
            return 0;
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    struct Window(HWND);
    impl raw_window_handle::HasWindowHandle for Window {
        fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
            let hwnd = std::num::NonZeroIsize::new(self.0 as isize).ok_or(raw_window_handle::HandleError::Unavailable)?;
            let handle = raw_window_handle::Win32WindowHandle::new(hwnd);
            // SAFETY: the window outlives the web view, which is dropped first.
            Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(handle.into()) })
        }
    }

    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let class = wide("NativeYtmLyricsCheck");
    let title = wide("Native YTM: a check for lyrics");
    let hwnd = unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            // The app's icon, the executable's first.
            hIcon: LoadIconW(instance, 1 as _),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let (w, h) = (420, 520);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let (x, y) = if sw > 0 && sh > 0 { ((sw - w) / 2, (sh - h) / 2) } else { (CW_USEDEFAULT, CW_USEDEFAULT) };
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            x,
            y,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        bail!("no window for the check");
    }
    crate::platform::style_title_bar(hwnd as isize, crate::theme::sidebar());

    let mut context = wry::WebContext::new(profile);
    let window = Window(hwnd);
    let target = hwnd as usize;
    let webview = wry::WebViewBuilder::new_with_web_context(&mut context)
        .with_url(PAGE)
        .with_initialization_script(BRIDGE)
        .with_background_color((17, 17, 17, 255))
        .with_ipc_handler(move |request| {
            let said: serde_json::Value = serde_json::from_str(request.body()).unwrap_or_default();
            let what = said["type"].as_str().unwrap_or_default();
            let said = match what {
                "turnstile-token" => match said["token"].as_str() {
                    Some(token) if !token.is_empty() => Said::Token(token.to_owned()),
                    _ => Said::Failed("an empty token".into()),
                },
                "turnstile-expired" => Said::Expired,
                "turnstile-error" => Said::Failed(format!("Cloudflare: {}", said["error"])),
                "turnstile-timeout" => Said::Failed("Cloudflare timed out".into()),
                _ => return,
            };
            SAID.with(|s| s.borrow_mut().push(said));
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(target as HWND, WM_SAID, 0, 0) };
        })
        .build(&window)?;

    let started = Instant::now();
    let mut shown: Option<Instant> = None;
    unsafe { SetTimer(hwnd, 1, 250, None) };
    let mut result: Option<Result<String>> = None;
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while result.is_none() && unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        for said in SAID.with(|s| std::mem::take(&mut *s.borrow_mut())) {
            match said {
                Said::Token(token) => result = Some(Ok(token)),
                Said::Failed(why) => result = Some(Err(anyhow::anyhow!(why))),
                Said::Expired => {
                    let _ = webview.evaluate_script("window.postMessage({ type: 'reset-turnstile' }, '*')");
                }
            }
        }
        if msg.message == WM_TIMER && result.is_none() {
            // Not passed unseen: up it comes, for a click.
            if shown.is_none() && started.elapsed() >= SHOW_AFTER {
                shown = Some(Instant::now());
                unsafe {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                }
            }
            if shown.is_some_and(|at| at.elapsed() >= GIVE_UP_AFTER) {
                result = Some(Err(anyhow::anyhow!("nobody answered it")));
            }
        }
    }
    unsafe { KillTimer(hwnd, 1) };
    drop(webview);
    match result {
        Some(result) => result,
        None if CLOSED.with(|c| *c.borrow()) => bail!("its window was closed"),
        None => bail!("it ended early"),
    }
}
