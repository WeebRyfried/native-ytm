//! The little Windows needs beyond what eframe and fastframe do: keeping the
//! sign-in cookie encrypted to this Windows account (DPAPI), a title bar in
//! the app's colours, and the app's identity for the media overlay.

use std::path::Path;

/// Encrypts `data` so only this Windows user can read it back.
#[cfg(windows)]
pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).ok()?,
        pbData: data.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points at `data`, alive for the call; DPAPI allocates
    // `output` with LocalAlloc, copied out and freed below.
    unsafe {
        let ok = CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 || output.pbData.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        Some(bytes)
    }
}

#[cfg(windows)]
pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).ok()?,
        pbData: data.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe {
        let ok = CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 || output.pbData.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        Some(bytes)
    }
}

#[cfg(not(windows))]
pub fn protect(_data: &[u8]) -> Option<Vec<u8>> {
    None
}

#[cfg(not(windows))]
pub fn unprotect(_data: &[u8]) -> Option<Vec<u8>> {
    None
}

/// The saved cookie, if there is one this user can decrypt.
pub fn load_cookie(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let plain = unprotect(&bytes)?;
    String::from_utf8(plain).ok()
}

pub fn save_cookie(path: &Path, cookie: &str) -> bool {
    match protect(cookie.as_bytes()) {
        Some(sealed) => {
            crate::settings::write_atomic(path, &sealed);
            true
        }
        None => false,
    }
}

pub fn forget_cookie(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// A dark title bar whose caption matches the window, on Windows 11 (and
/// the dark frame alone on Windows 10).
#[cfg(windows)]
pub fn style_title_bar(hwnd: isize, caption: egui::Color32) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DwmSetWindowAttribute,
    };
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    let dark: i32 = 1;
    // COLORREF is 0x00BBGGRR.
    let color: u32 =
        u32::from(caption.r()) | (u32::from(caption.g()) << 8) | (u32::from(caption.b()) << 16);
    // SAFETY: each attribute is a plain value whose size is passed with it;
    // a Windows that does not know one returns an error and draws as before.
    let results = unsafe {
        [
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                (&raw const dark).cast(),
                4,
            ),
            DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR as u32, (&raw const color).cast(), 4),
            DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as u32, (&raw const color).cast(), 4),
        ]
    };
    log::debug!("title bar attributes: {results:?}");
}

#[cfg(not(windows))]
pub fn style_title_bar(_hwnd: isize, _caption: egui::Color32) {}

/// Opens a web address in the default browser. Only http and https: a
/// link from YouTube's data never gets to start anything else.
#[cfg(windows)]
pub fn open_url(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        log::warn!("not opening a non-web link");
        return;
    }
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (verb, target) = (wide("open"), wide(url.trim()));
    // SAFETY: both strings are NUL-terminated and outlive the call; the
    // shell copies what it needs.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    if result as isize <= 32 {
        log::warn!("the browser did not open ({})", result as isize);
    }
}

#[cfg(not(windows))]
pub fn open_url(_url: &str) {}

/// Opens one of the app's own folders in Explorer.
#[cfg(windows)]
pub fn open_folder(path: &Path) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if !path.is_dir() {
        return;
    }
    let wide = |s: &std::ffi::OsStr| {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>()
    };
    let (verb, target) = (wide("explore".as_ref()), wide(path.as_os_str()));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(not(windows))]
pub fn open_folder(_path: &Path) {}

/// Hands the memory the process is not using back to Windows. After the
/// window is resized, the OpenGL driver is left holding buffers for every
/// size the window passed through (Intel's, seen at several hundred MB),
/// which it never touches again and frees only slowly. Pages still in use
/// come straight back.
#[cfg(windows)]
pub fn release_unused_memory() {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    // SAFETY: the current process's pseudo handle; (-1, -1) asks Windows to
    // take back what pages it can.
    unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

#[cfg(not(windows))]
pub fn release_unused_memory() {}

/// The app's identity to Windows. The media overlay names an app after the
/// Start menu shortcut carrying its id; without one it says "Unknown app".
pub const APP_ID: &str = "NativeYTM.Desktop";
pub const APP_NAME: &str = "Native YTM";

/// Tells Windows who this process is, and keeps a Start menu shortcut with
/// that id pointing at this executable. Call before any window exists.
#[cfg(windows)]
pub fn register_app_identity(config: &Path) {
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    // SAFETY: a valid, NUL-terminated string for the call.
    if let Err(error) = unsafe { SetCurrentProcessExplicitAppUserModelID(&windows::core::HSTRING::from(APP_ID)) } {
        log::warn!("app id: {error}");
    }
    // COM on a thread of its own, so it never meets the window's apartment.
    let marker = config.join("shortcut.txt");
    std::thread::spawn(move || {
        if let Err(error) = ensure_shortcut(&marker) {
            log::warn!("Start menu shortcut: {error}");
        }
    });
}

#[cfg(not(windows))]
pub fn register_app_identity(_config: &Path) {}

#[cfg(windows)]
/// `marker` remembers which executable the shortcut was made for.
fn ensure_shortcut(marker: &Path) -> windows::core::Result<()> {
    use std::mem::ManuallyDrop;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{GUID, HSTRING, Interface, PWSTR};

    let Some(dir) = std::env::var_os("APPDATA").map(|appdata| {
        std::path::PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs")
    }) else {
        return Ok(());
    };
    let exe = std::env::current_exe().map_err(|_| windows::core::Error::empty())?;
    let link = dir.join(format!("{APP_NAME}.lnk"));
    let made_for = std::fs::read_to_string(marker).unwrap_or_default();
    if link.exists() && made_for == exe.to_string_lossy() {
        return Ok(());
    }
    // SAFETY: COM is initialised for this thread and torn down at the end;
    // every interface comes from CoCreateInstance or `cast`, and the
    // property value points at `id`, which outlives its use, and is never
    // cleared by PROPVARIANT's drop (ManuallyDrop).
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let result = (|| {
            let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let persist: IPersistFile = shell_link.cast()?;
            shell_link.SetPath(&HSTRING::from(exe.as_path()))?;
            if let Some(parent) = exe.parent() {
                shell_link.SetWorkingDirectory(&HSTRING::from(parent))?;
            }
            shell_link.SetDescription(&HSTRING::from("A lightweight YouTube Music player"))?;
            shell_link.SetIconLocation(&HSTRING::from(exe.as_path()), 0)?;
            let store: IPropertyStore = shell_link.cast()?;
            let mut id: Vec<u16> = APP_ID.encode_utf16().chain(std::iter::once(0)).collect();
            let value = ManuallyDrop::new(PROPVARIANT {
                Anonymous: PROPVARIANT_0 {
                    Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                        vt: VT_LPWSTR,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: PROPVARIANT_0_0_0 { pwszVal: PWSTR(id.as_mut_ptr()) },
                    }),
                },
            });
            // System.AppUserModel.ID
            let key = PROPERTYKEY {
                fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
                pid: 5,
            };
            store.SetValue(&key, &*value)?;
            store.Commit()?;
            let _ = std::fs::create_dir_all(&dir);
            persist.Save(&HSTRING::from(link.as_path()), true)?;
            crate::settings::write_atomic(marker, exe.to_string_lossy().as_bytes());
            log::info!("Start menu shortcut written: {}", link.display());
            Ok(())
        })();
        CoUninitialize();
        result
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn dpapi_round_trip() {
        let sealed = super::protect(b"SAPISID=secret").expect("protect");
        assert_ne!(sealed, b"SAPISID=secret");
        assert_eq!(super::unprotect(&sealed).expect("unprotect"), b"SAPISID=secret");
    }
}
