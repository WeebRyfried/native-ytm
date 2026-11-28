//! Whether a window is wholly covered by the windows above it (native-ytm
//! patch). Windows says nothing of it (winit's `Occluded` is not sent
//! there), so it is worked out as Chromium does: the windows above this one,
//! front to back, each that can hide what is behind it taken off what of
//! this one shows, until nothing does. A covered window need not draw: what
//! it would draw is not seen, and each frame it presents still costs the
//! GPU and Windows' compositor (DWM) as much as one seen.
//!
//! A window counts only when it surely hides what is behind it: shown, not
//! minimised, not cloaked (on another virtual desktop, or one of the system's
//! that is put away), not see-through (layered with transparency, or letting
//! the mouse through), and not a tool window (overlays often are).

#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GW_HWNDPREV, GWL_EXSTYLE, GetLayeredWindowAttributes, GetWindow, GetWindowLongW, GetWindowRect, IsIconic,
    IsWindowVisible, LWA_ALPHA, LWA_COLORKEY, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

/// Past this many windows above, or pieces of this one still showing, it is
/// taken to show: the answer is not worth more looking.
const MOST_WINDOWS: usize = 512;
const MOST_PIECES: usize = 64;

/// Whether the window `hwnd` (a raw `HWND`) is cloaked or wholly covered.
pub fn covered(hwnd: isize) -> bool {
    let hwnd = hwnd as HWND;
    if cloaked(hwnd) {
        return true;
    }
    let Some(own) = bounds(hwnd) else {
        return false;
    };
    let mut showing = vec![own];
    let mut above = unsafe { GetWindow(hwnd, GW_HWNDPREV) };
    let mut looked = 0;
    while !above.is_null() && looked < MOST_WINDOWS {
        looked += 1;
        if hides(above)
            && let Some(cover) = bounds(above)
        {
            showing = subtract(&showing, &cover);
            if showing.is_empty() {
                return true;
            }
            if showing.len() > MOST_PIECES {
                return false;
            }
        }
        above = unsafe { GetWindow(above, GW_HWNDPREV) };
    }
    false
}

fn cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    let result = unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, (&raw mut cloaked).cast(), size_of::<u32>() as u32)
    };
    result >= 0 && cloaked != 0
}

/// Whether a window above surely hides what is behind it.
fn hides(hwnd: HWND) -> bool {
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return false;
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & (WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW) != 0 {
            return false;
        }
        if ex & WS_EX_LAYERED != 0 {
            // Faded as a whole, keyed by a colour, or drawn with an alpha of
            // its own per pixel (when this answers nothing): see-through.
            let (mut key, mut alpha, mut flags) = (0u32, 0u8, 0u32);
            if GetLayeredWindowAttributes(hwnd, &raw mut key, &raw mut alpha, &raw mut flags) == 0
                || flags & LWA_COLORKEY != 0
                || (flags & LWA_ALPHA != 0 && alpha < 255)
                || flags & LWA_ALPHA == 0
            {
                return false;
            }
        }
    }
    !cloaked(hwnd)
}

/// Where a window shows on screen: its frame as DWM draws it, without the
/// shadow; none when it shows nothing.
fn bounds(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let dwm = unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&raw mut rect).cast(), size_of::<RECT>() as u32)
    };
    if dwm < 0 && unsafe { GetWindowRect(hwnd, &raw mut rect) } == 0 {
        return None;
    }
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

/// What of `pieces` is left with `cover` taken off.
fn subtract(pieces: &[RECT], cover: &RECT) -> Vec<RECT> {
    let mut left = Vec::with_capacity(pieces.len() + 4);
    for r in pieces {
        if cover.right <= r.left || cover.left >= r.right || cover.bottom <= r.top || cover.top >= r.bottom {
            left.push(*r);
            continue;
        }
        // Above, below, then the sides between.
        if cover.top > r.top {
            left.push(RECT { left: r.left, top: r.top, right: r.right, bottom: cover.top });
        }
        if cover.bottom < r.bottom {
            left.push(RECT { left: r.left, top: cover.bottom, right: r.right, bottom: r.bottom });
        }
        let (top, bottom) = (r.top.max(cover.top), r.bottom.min(cover.bottom));
        if cover.left > r.left {
            left.push(RECT { left: r.left, top, right: cover.left, bottom });
        }
        if cover.right < r.right {
            left.push(RECT { left: cover.right, top, right: r.right, bottom });
        }
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }

    fn area(pieces: &[RECT]) -> i64 {
        pieces.iter().map(|r| i64::from(r.right - r.left) * i64::from(r.bottom - r.top)).sum()
    }

    #[test]
    fn taking_off_what_covers() {
        let own = [rect(0, 0, 100, 100)];
        // Wholly covered.
        assert!(subtract(&own, &rect(-10, -10, 110, 110)).is_empty());
        // Not at all.
        assert_eq!(area(&subtract(&own, &rect(200, 0, 300, 100))), 10_000);
        // A hole in the middle: the frame around it is left.
        assert_eq!(area(&subtract(&own, &rect(25, 25, 75, 75))), 10_000 - 2_500);
        // Two halves, one after the other.
        let half = subtract(&own, &rect(0, 0, 50, 100));
        assert_eq!(area(&half), 5_000);
        assert!(subtract(&half, &rect(50, 0, 100, 100)).is_empty());
    }
}
