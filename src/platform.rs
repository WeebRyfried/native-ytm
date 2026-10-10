//! The little Windows needs beyond what eframe and fastframe do: keeping the
//! sign-in cookie encrypted to this Windows account (DPAPI), and a title bar
//! in the app's colours.

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

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn dpapi_round_trip() {
        let sealed = super::protect(b"SAPISID=secret").expect("protect");
        assert_ne!(sealed, b"SAPISID=secret");
        assert_eq!(super::unprotect(&sealed).expect("unprotect"), b"SAPISID=secret");
    }
}
