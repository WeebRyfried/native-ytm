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

/// Opens Explorer on a file's folder with the file selected.
pub fn show_in_folder(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // explorer.exe takes `/select,` and the path as one argument, quoted.
        let mut command = std::process::Command::new("explorer.exe");
        command.raw_arg(format!("/select,\"{}\"", path.display()));
        let _ = command.spawn();
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// The main window, for dialogs to belong to.
static MAIN_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub fn set_main_window(hwnd: isize) {
    MAIN_WINDOW.store(hwnd, std::sync::atomic::Ordering::Relaxed);
}

/// The main window's title bar in `caption`, after a change of theme.
pub fn restyle_main_window(caption: egui::Color32) {
    let hwnd = MAIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    if hwnd != 0 {
        style_title_bar(hwnd, caption);
    }
}

/// The main window's icons drawn anew by `draw` (straight RGBA, square, at
/// the size asked), after a change of theme: the large one the taskbar and
/// Alt+Tab show and the small one, each at the size Windows shows it at the
/// window's scale. (egui's own command changes only the small one.)
#[cfg(windows)]
pub fn set_main_window_icon(draw: fn(usize) -> Vec<u8>) {
    use std::sync::atomic::{AtomicIsize, Ordering};

    use image::ImageEncoder;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, DestroyIcon, ICON_BIG, ICON_SMALL, LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON,
        SendMessageW, WM_SETICON,
    };
    // The icons made here last, let go once replaced. (Those the window
    // starts with are eframe's, which keeps no hold on them either.)
    static MADE: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];
    let hwnd = MAIN_WINDOW.load(Ordering::Relaxed);
    if hwnd == 0 {
        return;
    }
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    // SAFETY: the window is the app's own; each icon is made from a whole
    // PNG in memory (which CreateIconFromResourceEx takes as an icon's
    // image), and the one it replaces was made here and is no longer used.
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        for (made, (which, metric)) in MADE.iter().zip([(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)]) {
            let size = GetSystemMetricsForDpi(metric, dpi).max(16) as u32;
            let mut png = Vec::new();
            let encoder = image::codecs::png::PngEncoder::new(&mut png);
            if encoder.write_image(&draw(size as usize), size, size, image::ExtendedColorType::Rgba8).is_err() {
                continue;
            }
            let icon = CreateIconFromResourceEx(png.as_ptr(), png.len() as u32, 1, 0x0003_0000, size as i32, size as i32, LR_DEFAULTCOLOR);
            if icon.is_null() {
                continue;
            }
            SendMessageW(hwnd, WM_SETICON, which as usize, icon as isize);
            let old = made.swap(icon as isize, Ordering::Relaxed);
            if old != 0 {
                DestroyIcon(old as _);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn set_main_window_icon(_draw: fn(usize) -> Vec<u8>) {}

/// Windows' folder picker, over the main window. Blocks until it closes,
/// so call it from a thread of its own.
#[cfg(windows)]
pub fn pick_folder(title: &str) -> Option<std::path::PathBuf> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
    use windows::core::HSTRING;

    let owner = HWND(MAIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed) as *mut std::ffi::c_void);
    // SAFETY: COM is initialised for this thread (a dialog wants a single-
    // threaded apartment) and torn down at the end; the dialog and the item
    // come from COM, and the path it allocates is freed with CoTaskMemFree.
    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_err() {
            return None;
        }
        let picked = (|| -> windows::core::Result<std::path::PathBuf> {
            let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
            dialog.SetTitle(&HSTRING::from(title))?;
            dialog.Show(Some(owner))?;
            let item = dialog.GetResult()?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = name.to_string().map_err(|_| windows::core::Error::empty());
            CoTaskMemFree(Some(name.0.cast()));
            Ok(std::path::PathBuf::from(path?))
        })();
        CoUninitialize();
        picked.ok()
    }
}

#[cfg(not(windows))]
pub fn pick_folder(_title: &str) -> Option<std::path::PathBuf> {
    None
}

/// Waits up to `limit` for another process to end.
/// Where Windows looks for what to start when this user signs in, and the
/// app's name there.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "Native YTM";

/// What starts the app with Windows: this program, into the tray.
pub fn autostart_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --background", exe.display()))
}

/// What Windows starts for the app when this user signs in, if anything.
pub fn autostart() -> Option<String> {
    read_string(RUN_KEY, RUN_NAME)
}

/// Starts the app with Windows, or no longer. True once done.
pub fn set_autostart(on: bool) -> bool {
    match (on, autostart_command()) {
        (true, Some(command)) => write_string(RUN_KEY, RUN_NAME, &command),
        (true, None) => false,
        (false, _) => delete_value(RUN_KEY, RUN_NAME),
    }
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// A text value under HKEY_CURRENT_USER.
#[cfg(windows)]
fn read_string(key: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let (key, name) = (wide(key), wide(name));
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: NUL-terminated names, and a buffer of `size` bytes.
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut size)
    };
    if status != 0 {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1).min(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

#[cfg(windows)]
fn write_string(key: &str, name: &str, value: &str) -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
    let (key, name, value) = (wide(key), wide(name), wide(value));
    // SAFETY: NUL-terminated names, and the value's bytes with its NUL.
    let status = unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32)
    };
    status == 0
}

#[cfg(windows)]
fn delete_value(key: &str, name: &str) -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
    let (key, name) = (wide(key), wide(name));
    // SAFETY: NUL-terminated names.
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
    // Gone already counts.
    status == 0 || status == 2
}

#[cfg(not(windows))]
fn read_string(_key: &str, _name: &str) -> Option<String> {
    None
}

#[cfg(not(windows))]
fn write_string(_key: &str, _name: &str, _value: &str) -> bool {
    false
}

#[cfg(not(windows))]
fn delete_value(_key: &str, _name: &str) -> bool {
    false
}

/// The space free on the drive `path` is on, for this user.
#[cfg(windows)]
pub fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is a NUL-terminated path alive for the call; the
    // totals not wanted are null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

#[cfg(not(windows))]
pub fn free_space(_path: &Path) -> Option<u64> {
    None
}

#[cfg(windows)]
pub fn wait_for_exit(pid: u32, limit: std::time::Duration) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: a handle opened for waiting only, closed after the wait.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return;
        }
        WaitForSingleObject(handle, limit.as_millis().min(u128::from(u32::MAX - 1)) as u32);
        CloseHandle(handle);
    }
}

#[cfg(not(windows))]
pub fn wait_for_exit(_pid: u32, _limit: std::time::Duration) {}

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
mod registry_tests {
    use super::*;

    /// Under a key of the tests' own, which is removed after.
    #[test]
    fn writes_reads_and_deletes_a_value() {
        let key = r"Software\native-ytm-test";
        assert!(write_string(key, "Command", "\"C:\\Program Files\\x.exe\" --background"));
        assert_eq!(read_string(key, "Command").as_deref(), Some("\"C:\\Program Files\\x.exe\" --background"));
        assert!(delete_value(key, "Command"));
        assert_eq!(read_string(key, "Command"), None);
        assert!(delete_value(key, "Command"));
        let wide_key = wide(key);
        // SAFETY: a NUL-terminated name of the test's own key.
        unsafe { windows_sys::Win32::System::Registry::RegDeleteKeyW(windows_sys::Win32::System::Registry::HKEY_CURRENT_USER, wide_key.as_ptr()) };
        assert!(autostart_command().is_some_and(|c| c.ends_with(" --background")));
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
