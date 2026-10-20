//! A note in the top right corner of the screen as one song gives way to
//! the next while the window is out of sight (minimised, or closed to the
//! taskbar's corner): the song ending with an arrow after it, then the one
//! coming in sliding over it, and a few seconds later nothing. Off unless
//! turned on in Settings, Window.
//!
//! Made to cost next to nothing. The note is a layered window on a thread
//! of its own, both made for a change and gone once it has faded. Its words
//! are drawn once per change, with GDI, into masks; a frame is those masks
//! and a rounded plate put together in a small bitmap, made only while
//! something moves (about a second and a half of the five), and nothing
//! runs while the note holds still. It never takes the focus or a click,
//! and stays away while a game, a video or a presentation fills the screen.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::theme::ease_in_out;

/// A song as the note names it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Song {
    pub title: String,
    pub artist: String,
}

/// One song giving way to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub from: Song,
    pub to: Song,
}

/// The note, from the app's side.
#[derive(Default)]
pub struct Overlay {
    shared: Arc<Mutex<Shared>>,
}

/// Between the app and the note's thread: the change to show next, whether
/// the thread is there to show it, and its id, to wake it with.
#[derive(Default)]
struct Shared {
    pending: Option<Change>,
    running: bool,
    thread: u32,
}

impl Overlay {
    /// Shows `change` at the corner of the screen, or moves the note
    /// already there on to it.
    pub fn show(&self, change: Change) {
        let mut shared = self.shared.lock();
        shared.pending = Some(change);
        if shared.running {
            imp::wake(shared.thread);
            return;
        }
        let state = Arc::clone(&self.shared);
        shared.running = std::thread::Builder::new().name("song-note".into()).spawn(move || imp::run(&state)).is_ok();
    }
}

// ---- The course of a change ---------------------------------------------------

/// How long each part takes, in seconds: the note coming in, the song
/// ending held with its arrow (shorter when the note was showing already),
/// the next sliding over it, held, and the note fading away.
const APPEAR: f32 = 0.3;
const HOLD: f32 = 0.9;
const HOLD_AGAIN: f32 = 0.4;
const CROSS: f32 = 0.7;
/// How much of the slide the song ending takes to go, before the next
/// comes in.
const OUT: f32 = 0.45;
const STAY: f32 = 3.2;
const LEAVE: f32 = 0.5;
/// A frame while something moves: 60 a second.
const FRAME: f32 = 1.0 / 60.0;

/// How the note looks at a moment of a change.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    /// The whole note's opacity.
    opacity: f32,
    /// How far it has come down into its place (0 to 1).
    settled: f32,
    /// The arrow's opacity, after the song ending.
    arrow: f32,
    /// How far the song coming in has slid over the one ending (0 to 1).
    cross: f32,
}

/// A change's course, from the note's opacity when it came: 0 when there
/// was no note, more when one was showing (it carries on from there).
#[derive(Clone, Copy, Debug)]
struct Course {
    from: f32,
}

impl Course {
    fn again(self) -> bool {
        self.from > 0.0
    }

    fn cross_at(self) -> f32 {
        APPEAR + if self.again() { HOLD_AGAIN } else { HOLD }
    }

    fn leave_at(self) -> f32 {
        self.cross_at() + CROSS + STAY
    }

    /// How the note looks `t` seconds in; none once it has gone.
    fn look(self, t: f32) -> Option<Look> {
        if t >= self.leave_at() + LEAVE {
            return None;
        }
        let came = crate::theme::ease_out(t / APPEAR);
        let opacity = if t < self.leave_at() {
            self.from + (1.0 - self.from) * came
        } else {
            1.0 - ease_in_out((t - self.leave_at()) / LEAVE)
        };
        Some(Look {
            opacity,
            settled: if self.again() { 1.0 } else { came },
            // A note showing already had no arrow: it comes in.
            arrow: if self.again() { came } else { 1.0 },
            cross: ease_in_out((t - self.cross_at()) / CROSS),
        })
    }

    /// Seconds from `t` until the note looks different: a frame while it
    /// moves, until the next part while it holds still.
    fn still_for(self, t: f32) -> f32 {
        let until = |at: f32| (at - t).max(FRAME);
        if t < APPEAR {
            FRAME
        } else if t < self.cross_at() {
            until(self.cross_at())
        } else if t < self.cross_at() + CROSS {
            FRAME
        } else if t < self.leave_at() {
            until(self.leave_at())
        } else {
            FRAME
        }
    }
}

// ---- The note drawn ---------------------------------------------------------

/// The note's measures at 100% (96 DPI), in pixels: the room around the
/// words, the type, the gap before the arrow, how wide the words may run,
/// the corners, how far from the screen's corner, how far a song slides
/// and how far the note drops into place.
const PAD_X: f32 = 18.0;
const PAD_Y: f32 = 12.0;
const TITLE: f32 = 15.0;
const ARTIST: f32 = 12.5;
const LINE_GAP: f32 = 1.0;
const ARROW_GAP: f32 = 10.0;
const MIN_TEXT: f32 = 120.0;
const MAX_TEXT: f32 = 320.0;
const RADIUS: f32 = 10.0;
const MARGIN: f32 = 16.0;
const SLIDE: f32 = 14.0;
const DROP: f32 = 6.0;

/// The plate under the words: near black, the desktop showing through, a
/// hairline lighter at its edge.
const PLATE: [f32; 3] = [20.0, 20.0, 23.0];
const PLATE_ALPHA: f32 = 0.74;
const HAIRLINE: f32 = 0.12;

/// How white each part is drawn: the title, the artist, the arrow.
const TITLE_LEVEL: u8 = 244;
const ARTIST_LEVEL: u8 = 152;
const ARROW_LEVEL: u8 = 150;

/// What was drawn, each pixel how much white (0 to 255).
#[derive(Clone, Debug, Default)]
struct Mask {
    w: usize,
    h: usize,
    a: Vec<u8>,
}

impl Mask {
    /// Its whiteness at column `x` (between two columns, blended, so words
    /// slide smoothly) of row `y`, from 0 to 1.
    fn at(&self, x: f32, y: usize) -> f32 {
        if y >= self.h || x <= -1.0 || x >= self.w as f32 {
            return 0.0;
        }
        let left = x.floor();
        let f = x - left;
        let left = left as isize;
        let get = |x: isize| if x >= 0 && (x as usize) < self.w { f32::from(self.a[y * self.w + x as usize]) } else { 0.0 };
        (get(left) * (1.0 - f) + get(left + 1) * f) / 255.0
    }
}

/// A change drawn: the song ending and the one coming in (each its title
/// over its artist), and the arrow, which sits `arrow_x` after the start
/// of the song ending. All are placed from the top left of the words.
struct Note {
    from: Mask,
    arrow: Mask,
    arrow_x: f32,
    to: Mask,
    /// The screen's pixels to one at 100%.
    scale: f32,
    /// The plate's height, in pixels.
    height: usize,
}

impl Note {
    /// The plate's width with `look`, in pixels: around the song ending and
    /// its arrow, and growing or shrinking to the song coming in as it
    /// slides over.
    fn width(&self, look: &Look) -> usize {
        let alone = self.from.w as f32;
        let from = alone + ((self.arrow_x + self.arrow.w as f32).max(alone) - alone) * look.arrow;
        let words = from + (self.to.w as f32 - from) * look.cross;
        (words.max(MIN_TEXT * self.scale) + 2.0 * PAD_X * self.scale).ceil() as usize
    }

    /// The widest the plate gets.
    fn widest(&self) -> usize {
        let full = Look { opacity: 1.0, settled: 1.0, arrow: 1.0, cross: 0.0 };
        self.width(&full).max(self.width(&Look { cross: 1.0, ..full }))
    }

    /// Puts the note together for `look` in `out`, premultiplied BGRA rows
    /// `stride` pixels apart, and says how wide it is.
    fn compose(&self, look: &Look, out: &mut [u8], stride: usize) -> usize {
        let (w, h) = (self.width(look), self.height);
        let s = self.scale;
        let (pad_x, pad_y) = (PAD_X * s, (PAD_Y * s).round() as usize);
        let radius = RADIUS * s;
        // The song ending goes out before the one coming in comes, so the
        // two are never seen over one another.
        let from_a = 1.0 - ease_in_out(look.cross / OUT);
        let to_a = ease_in_out((look.cross - OUT) / (1.0 - OUT));
        let from_x = pad_x - look.cross * SLIDE * s;
        let to_x = pad_x + (1.0 - look.cross) * SLIDE * s;
        // Words fade toward the plate's sides, where they slide in and out.
        let edge = pad_x * 0.8;
        for y in 0..h {
            let row = &mut out[y * stride * 4..][..w * 4];
            let words_y = y.checked_sub(pad_y);
            for x in 0..w {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = rounded_rect(px, py, w as f32, h as f32, radius);
                let cover = (0.5 - d).clamp(0.0, 1.0);
                let pixel = &mut row[x * 4..x * 4 + 4];
                if cover <= 0.0 {
                    pixel.fill(0);
                    continue;
                }
                let mut text = 0.0;
                if let Some(wy) = words_y {
                    let fx = x as f32;
                    let a = self.from.at(fx - from_x, wy) * from_a;
                    let b = self.arrow.at(fx - from_x - self.arrow_x, wy) * from_a * look.arrow;
                    let c = self.to.at(fx - to_x, wy) * to_a;
                    text = (1.0 - (1.0 - a) * (1.0 - b) * (1.0 - c)) * (px / edge).min((w as f32 - px) / edge).clamp(0.0, 1.0);
                }
                text *= cover;
                let ring = (1.0 - (d + 0.5).abs()).clamp(0.0, 1.0) * HAIRLINE;
                let plate = cover * PLATE_ALPHA;
                for (i, channel) in [2, 1, 0].into_iter().enumerate() {
                    let base = PLATE[channel] + (255.0 - PLATE[channel]) * ring;
                    pixel[i] = (255.0 * text + base * plate * (1.0 - text)).round() as u8;
                }
                pixel[3] = (255.0 * (text + plate * (1.0 - text))).round() as u8;
            }
        }
        w
    }
}

/// How far `(px, py)` is outside a `w` by `h` rectangle at the origin with
/// corners rounded by `r` (less than nothing inside it).
fn rounded_rect(px: f32, py: f32, w: f32, h: f32, r: f32) -> f32 {
    let (cx, cy) = (w / 2.0, h / 2.0);
    let qx = (px - cx).abs() - (cx - r);
    let qy = (py - cy).abs() - (cy - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

// ---- Windows -----------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use std::ptr::{null, null_mut};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT, SIZE};
    use windows_sys::Win32::Graphics::Dwm::DwmFlush;
    use windows_sys::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS,
        DT_CALCRECT, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DeleteDC, DeleteObject, DrawTextW, FW_NORMAL,
        FW_SEMIBOLD, GdiFlush, GetMonitorInfoW, GetTextFaceW, HBITMAP, HDC, HFONT, HGDIOBJ, MONITOR_DEFAULTTOPRIMARY,
        MONITORINFO, MonitorFromPoint, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::Shell::{
        QUNS_BUSY, QUNS_NOT_PRESENT, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_NOREMOVE,
        PM_REMOVE, PeekMessageW, PostThreadMessageW, QS_ALLINPUT, RegisterClassW, SW_SHOWNOACTIVATE, ShowWindow,
        ULW_ALPHA, UpdateLayeredWindow, WM_APP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    use super::{
        ARROW_GAP, ARROW_LEVEL, ARTIST, ARTIST_LEVEL, Change, Course, DROP, FRAME, LINE_GAP, Look, MARGIN, MAX_TEXT,
        Mask, Note, PAD_Y, Shared, Song, TITLE, TITLE_LEVEL,
    };

    /// Wakes the note's thread: a change is waiting.
    pub(super) fn wake(thread: u32) {
        if thread != 0 {
            // SAFETY: a message with no pointers in it, to a thread id; a
            // thread gone or without a queue yet only makes it fail.
            unsafe { PostThreadMessageW(thread, WM_APP, 0, 0) };
        }
    }

    /// The note's thread: shows each change as it comes, and ends once the
    /// last has faded with nothing more waiting.
    pub(super) fn run(shared: &Mutex<Shared>) {
        // SAFETY: plain calls; the message is written by PeekMessageW. The
        // first makes the thread's queue, for `wake` to post to.
        unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let mut msg: MSG = std::mem::zeroed();
            PeekMessageW(&mut msg, null_mut(), 0, 0, PM_NOREMOVE);
            shared.lock().thread = GetCurrentThreadId();
        }
        let mut shown: Option<Shown> = None;
        loop {
            let change = {
                let mut shared = shared.lock();
                match shared.pending.take() {
                    Some(change) => Some(change),
                    None if shown.is_none() => {
                        shared.running = false;
                        shared.thread = 0;
                        return;
                    }
                    None => None,
                }
            };
            if let Some(change) = change {
                match &mut shown {
                    Some(note) => note.change_to(&change),
                    // Nothing over a game, a video or a presentation.
                    None if screen_busy() => log::info!("song change not shown: something fills the screen"),
                    None => shown = Shown::new(&change),
                }
            }
            let Some(note) = &mut shown else {
                continue;
            };
            let t = note.started.elapsed().as_secs_f32();
            let Some(look) = note.course.look(t) else {
                shown = None;
                continue;
            };
            note.present(&look);
            let wait = note.course.still_for(t);
            if wait <= FRAME {
                note.next_frame();
            } else {
                wait_for(Duration::from_secs_f32(wait));
            }
        }
    }

    /// Whether something fills the screen (a game, a video in full screen,
    /// a presentation) or no one is there (locked, the screen saver).
    fn screen_busy() -> bool {
        let mut state = 0;
        // SAFETY: writes one value.
        let ok = unsafe { SHQueryUserNotificationState(&mut state) } == 0;
        ok && matches!(state, QUNS_NOT_PRESENT | QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
    }

    /// Waits up to `limit`, or until a change is posted, then lets the
    /// thread's messages through.
    fn wait_for(limit: Duration) {
        let ms = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: no handles; the message is written by PeekMessageW.
        unsafe {
            MsgWaitForMultipleObjects(0, null(), 0, ms, QS_ALLINPUT);
            pump();
        }
    }

    /// # Safety
    ///
    /// Call on the note's thread.
    unsafe fn pump() {
        // SAFETY: as in `wait_for`.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                DispatchMessageW(&msg);
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The note on the screen: its window, the bitmap its frames are put
    /// together in, the change drawn, and where its course is.
    struct Shown {
        window: HWND,
        surface: Surface,
        note: Note,
        course: Course,
        started: Instant,
        /// The top right corner of the screen's working area, in pixels.
        corner: (i32, i32),
        /// What was last put on the screen: the look it was made for (to
        /// make it again only when that moves), its width, its opacity.
        made: Option<(f32, f32)>,
        width: usize,
        opacity: f32,
        visible: bool,
        last_frame: Instant,
    }

    impl Shown {
        fn new(change: &Change) -> Option<Self> {
            // The primary screen: its working area (the taskbar left out)
            // and its scale.
            // SAFETY: plain calls; the info is written by GetMonitorInfoW
            // and the DPI by GetDpiForMonitor.
            let (corner, scale) = unsafe {
                let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
                let mut info: MONITORINFO = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if GetMonitorInfoW(monitor, &mut info) == 0 {
                    return None;
                }
                let (mut dpi, mut _y) = (96, 96);
                if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y) != 0 {
                    dpi = 96;
                }
                ((info.rcWork.right, info.rcWork.top), dpi as f32 / 96.0)
            };
            let note = draw(change, scale)?;
            let surface = Surface::new(note.widest(), note.height)?;
            let class = wide("NativeYtmSongNote");
            // SAFETY: the class and title outlive the calls; the window is
            // destroyed when this is dropped.
            let window = unsafe {
                let instance = GetModuleHandleW(null());
                let class_info = WNDCLASSW {
                    lpfnWndProc: Some(DefWindowProcW),
                    hInstance: instance,
                    lpszClassName: class.as_ptr(),
                    ..std::mem::zeroed()
                };
                // Once a run: the second time it says it already is.
                RegisterClassW(&class_info);
                let title = wide("Native YTM");
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_POPUP,
                    corner.0,
                    corner.1,
                    1,
                    1,
                    null_mut(),
                    null_mut(),
                    instance,
                    null(),
                )
            };
            if window.is_null() {
                return None;
            }
            log::info!("song change shown at the corner of the screen ({}, {}), at {:.0}%", corner.0, corner.1, scale * 100.0);
            Some(Self {
                window,
                surface,
                note,
                course: Course { from: 0.0 },
                started: Instant::now(),
                corner,
                made: None,
                width: 0,
                opacity: 0.0,
                visible: false,
                last_frame: Instant::now(),
            })
        }

        /// Another change while the note shows: on to it from where it is.
        fn change_to(&mut self, change: &Change) {
            let Some(note) = draw(change, self.note.scale) else {
                return;
            };
            if note.widest() > self.surface.w || note.height != self.surface.h {
                let Some(surface) = Surface::new(note.widest().max(self.surface.w), note.height) else {
                    return;
                };
                self.surface = surface;
            }
            self.note = note;
            self.course = Course { from: self.opacity.max(0.01) };
            self.started = Instant::now();
            self.made = None;
        }

        /// Puts the note on the screen as it looks now, making the frame
        /// again only when the words have moved.
        fn present(&mut self, look: &Look) {
            if self.made != Some((look.cross, look.arrow)) {
                self.made = Some((look.cross, look.arrow));
                let stride = self.surface.w;
                self.width = self.note.compose(look, self.surface.pixels(), stride);
            }
            self.opacity = look.opacity;
            let s = self.note.scale;
            let at = POINT {
                x: self.corner.0 - (MARGIN * s).round() as i32 - self.width as i32,
                y: self.corner.1 + (MARGIN * s).round() as i32 - ((1.0 - look.settled) * DROP * s).round() as i32,
            };
            let size = SIZE { cx: self.width as i32, cy: self.note.height as i32 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: (look.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            // SAFETY: the window and the bitmap's DC are this note's own,
            // and the bitmap is at least `size`.
            unsafe {
                UpdateLayeredWindow(self.window, null_mut(), &at, &size, self.surface.dc, &POINT { x: 0, y: 0 }, 0, &blend, ULW_ALPHA);
                if !self.visible {
                    self.visible = true;
                    ShowWindow(self.window, SW_SHOWNOACTIVATE);
                }
            }
        }

        /// Waits for the screen's next frame, at no more than 60 a second.
        fn next_frame(&mut self) {
            let gap = Duration::from_secs_f32(FRAME * 0.9);
            // SAFETY: plain calls, on the note's thread.
            unsafe {
                loop {
                    let before = Instant::now();
                    let flushed = DwmFlush() == 0;
                    if self.last_frame.elapsed() >= gap {
                        break;
                    }
                    // No frame of the screen's to wait for: sleep instead.
                    if !flushed || before.elapsed() < Duration::from_millis(1) {
                        std::thread::sleep(gap.saturating_sub(self.last_frame.elapsed()));
                        break;
                    }
                }
                pump();
            }
            self.last_frame = Instant::now();
        }
    }

    impl Drop for Shown {
        fn drop(&mut self) {
            // SAFETY: the window is this note's own.
            unsafe { DestroyWindow(self.window) };
        }
    }

    /// A bitmap to put frames together in, and a DC holding it.
    struct Surface {
        dc: HDC,
        bitmap: HBITMAP,
        old: HGDIOBJ,
        bits: *mut u8,
        w: usize,
        h: usize,
    }

    impl Surface {
        fn new(w: usize, h: usize) -> Option<Self> {
            // SAFETY: the bitmap's memory is GDI's, `w * h * 4` bytes at
            // `bits` for as long as the bitmap lives, freed in Drop.
            unsafe {
                let dc = CreateCompatibleDC(null_mut());
                if dc.is_null() {
                    return None;
                }
                let (bitmap, bits) = dib(dc, w, h);
                if bitmap.is_null() {
                    DeleteDC(dc);
                    return None;
                }
                let old = SelectObject(dc, bitmap);
                Some(Self { dc, bitmap, old, bits, w, h })
            }
        }

        fn pixels(&mut self) -> &mut [u8] {
            // SAFETY: see `new`; borrowed mutably through `self`.
            unsafe { std::slice::from_raw_parts_mut(self.bits, self.w * self.h * 4) }
        }
    }

    impl Drop for Surface {
        fn drop(&mut self) {
            // SAFETY: this surface's own objects.
            unsafe {
                SelectObject(self.dc, self.old);
                DeleteObject(self.bitmap);
                DeleteDC(self.dc);
            }
        }
    }

    /// A 32-bit top-down bitmap `w` by `h`, and its pixels.
    ///
    /// # Safety
    ///
    /// `dc` is a live DC; the bitmap is the caller's to delete.
    unsafe fn dib(dc: HDC, w: usize, h: usize) -> (HBITMAP, *mut u8) {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = null_mut();
        // SAFETY: as the caller promises; GDI writes `bits`.
        let bitmap = unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
        (bitmap, bits.cast())
    }

    /// The change's words drawn with GDI, at `scale`.
    fn draw(change: &Change, scale: f32) -> Option<Note> {
        // SAFETY: every GDI object made here is deleted before returning.
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return None;
            }
            let face = face(dc);
            let font = |size: f32, weight: u32| {
                CreateFontW(
                    -(size * scale).round() as i32,
                    0,
                    0,
                    0,
                    weight as i32,
                    0,
                    0,
                    0,
                    u32::from(DEFAULT_CHARSET),
                    u32::from(OUT_DEFAULT_PRECIS),
                    u32::from(CLIP_DEFAULT_PRECIS),
                    u32::from(ANTIALIASED_QUALITY),
                    0,
                    face.as_ptr(),
                )
            };
            let (title, artist, arrow) = (font(TITLE, FW_SEMIBOLD), font(ARTIST, FW_NORMAL), font(TITLE, FW_NORMAL));
            let arrow_mask = words(dc, &[("\u{2192}", arrow, 0, ARROW_LEVEL)], f32::INFINITY);
            let room = MAX_TEXT * scale - ARROW_GAP * scale - arrow_mask.w as f32;
            let line = measure(dc, title, "Ag").1 + (LINE_GAP * scale).round() as i32;
            let song = |song: &Song| words(dc, &[(&song.title, title, 0, TITLE_LEVEL), (&song.artist, artist, line, ARTIST_LEVEL)], room);
            let (from, to) = (song(&change.from), song(&change.to));
            for object in [title, artist, arrow] {
                DeleteObject(object);
            }
            DeleteDC(dc);
            let title_w = measure_width(&from, line);
            let height = from.h.max(to.h).max(arrow_mask.h) + 2 * (PAD_Y * scale).round() as usize;
            Some(Note { arrow_x: title_w + ARROW_GAP * scale, from, arrow: arrow_mask, to, scale, height })
        }
    }

    /// How far the title (the rows above `line`) runs in `mask`.
    fn measure_width(mask: &Mask, line: i32) -> f32 {
        let rows = (line.max(0) as usize).min(mask.h);
        (0..mask.w).rev().find(|&x| (0..rows).any(|y| mask.a[y * mask.w + x] > 8)).map_or(0.0, |x| x as f32 + 1.0)
    }

    /// The face the note is set in: Windows 11's Segoe UI Variable, else
    /// Segoe UI. Both fall back to Meiryo UI and others for Japanese,
    /// Chinese and Korean.
    fn face(dc: HDC) -> &'static [u16] {
        static FACE: OnceLock<Vec<u16>> = OnceLock::new();
        FACE.get_or_init(|| {
            let wanted = "Segoe UI Variable Text";
            let name = wide(wanted);
            // SAFETY: the font is selected only for the question and
            // deleted after; the name is written into `got`.
            let found = unsafe {
                let font = CreateFontW(-16, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0, 1, 0, 0, 4, 0, name.as_ptr());
                let old = SelectObject(dc, font);
                let mut got = [0u16; 64];
                let n = GetTextFaceW(dc, got.len() as i32, got.as_mut_ptr());
                SelectObject(dc, old);
                DeleteObject(font);
                String::from_utf16_lossy(&got[..n.max(0) as usize]).trim_end_matches('\0').to_owned()
            };
            wide(if found.eq_ignore_ascii_case(wanted) { wanted } else { "Segoe UI" })
        })
    }

    /// How wide and high `text` is in `font`.
    ///
    /// # Safety
    ///
    /// `dc` and `font` are live.
    unsafe fn measure(dc: HDC, font: HFONT, text: &str) -> (i32, i32) {
        let text: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: as the caller promises.
        unsafe {
            let old = SelectObject(dc, font);
            DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut rect, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
            SelectObject(dc, old);
        }
        (rect.right - rect.left, rect.bottom - rect.top)
    }

    /// `parts` (each its text, font, top, and how white) drawn white on
    /// black, each cut short with an ellipsis past `room`, and read back.
    ///
    /// # Safety
    ///
    /// `dc` and the fonts are live.
    unsafe fn words(dc: HDC, parts: &[(&str, HFONT, i32, u8)], room: f32) -> Mask {
        let room = if room.is_finite() { room.max(1.0) as i32 } else { i32::MAX };
        // SAFETY: as the caller promises.
        let sizes: Vec<(i32, i32)> = parts.iter().map(|(text, font, ..)| unsafe { measure(dc, *font, text) }).collect();
        let w = sizes.iter().map(|(w, _)| (*w).min(room)).max().unwrap_or(0).max(1) as usize;
        let h = parts.iter().zip(&sizes).map(|((_, _, top, _), (_, h))| top + h).max().unwrap_or(0).max(1) as usize;
        // SAFETY: the bitmap is `w * h * 4` bytes at `bits` until deleted
        // at the end, and GDI is done drawing into it once flushed.
        unsafe {
            let (bitmap, bits) = dib(dc, w, h);
            if bitmap.is_null() {
                return Mask::default();
            }
            let old_bitmap = SelectObject(dc, bitmap);
            std::ptr::write_bytes(bits, 0, w * h * 4);
            SetBkMode(dc, TRANSPARENT as i32);
            for ((text, font, top, level), (width, height)) in parts.iter().zip(&sizes) {
                if text.is_empty() {
                    continue;
                }
                let text: Vec<u16> = text.encode_utf16().collect();
                let mut rect = RECT { left: 0, top: *top, right: (*width).min(room), bottom: top + height };
                let old_font = SelectObject(dc, *font);
                SetTextColor(dc, u32::from(*level) * 0x0001_0101);
                DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut rect, DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
                SelectObject(dc, old_font);
            }
            GdiFlush();
            let pixels = std::slice::from_raw_parts(bits, w * h * 4);
            let a = pixels.chunks_exact(4).map(|p| p[0].max(p[1]).max(p[2])).collect();
            SelectObject(dc, old_bitmap);
            DeleteObject(bitmap);
            Mask { w, h, a }
        }
    }

    #[cfg(test)]
    pub(super) fn draw_for_test(change: &Change, scale: f32) -> Option<Note> {
        draw(change, scale)
    }
}

#[cfg(not(windows))]
mod imp {
    use parking_lot::Mutex;

    use super::Shared;

    pub(super) fn wake(_thread: u32) {}

    pub(super) fn run(shared: &Mutex<Shared>) {
        let mut shared = shared.lock();
        shared.pending = None;
        shared.running = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_comes_in_holds_slides_over_and_goes() {
        let course = Course { from: 0.0 };
        let start = course.look(0.0).unwrap();
        assert_eq!((start.opacity, start.settled, start.arrow, start.cross), (0.0, 0.0, 1.0, 0.0));
        // The song ending, with its arrow, held still: nothing to draw.
        let held = course.look(APPEAR + HOLD / 2.0).unwrap();
        assert_eq!((held.opacity, held.settled, held.cross), (1.0, 1.0, 0.0));
        assert!(course.still_for(APPEAR + 0.1) > 0.5, "holding still, no frames");
        // Halfway over, and then the song coming in alone.
        let halfway = course.look(course.cross_at() + CROSS / 2.0).unwrap();
        assert!((halfway.cross - 0.5).abs() < 0.01);
        assert_eq!(course.still_for(course.cross_at() + 0.1), FRAME);
        assert_eq!(course.look(course.leave_at() - 0.1).unwrap().cross, 1.0);
        assert!(course.still_for(course.cross_at() + CROSS + 0.1) > 3.0);
        // Fading away, then gone: about five seconds in all.
        assert!(course.look(course.leave_at() + LEAVE / 2.0).unwrap().opacity < 0.6);
        assert_eq!(course.look(course.leave_at() + LEAVE), None);
        assert!((4.5..6.0).contains(&(course.leave_at() + LEAVE)));
    }

    #[test]
    fn a_change_while_the_note_shows_carries_on_from_it() {
        // Half faded away when the next change came.
        let course = Course { from: 0.5 };
        let start = course.look(0.0).unwrap();
        assert_eq!((start.opacity, start.settled, start.arrow), (0.5, 1.0, 0.0));
        assert_eq!(course.look(APPEAR).unwrap().opacity, 1.0);
        assert_eq!(course.look(APPEAR).unwrap().arrow, 1.0);
        assert!(course.cross_at() < Course { from: 0.0 }.cross_at(), "a shorter hold");
    }

    fn block(w: usize, h: usize) -> Mask {
        Mask { w, h, a: vec![255; w * h] }
    }

    fn note() -> Note {
        Note { from: block(60, 30), arrow: block(12, 18), arrow_x: 70.0, to: block(200, 30), scale: 1.0, height: 54 }
    }

    fn alpha(out: &[u8], stride: usize, x: usize, y: usize) -> u8 {
        out[(y * stride + x) * 4 + 3]
    }

    #[test]
    fn the_plate_fits_the_song_ending_then_the_one_coming_in() {
        let note = note();
        let look = Look { opacity: 1.0, settled: 1.0, arrow: 1.0, cross: 0.0 };
        // At least the narrowest; then as wide as the song coming in.
        assert_eq!(note.width(&look), (MIN_TEXT + 2.0 * PAD_X) as usize);
        assert_eq!(note.width(&Look { cross: 1.0, ..look }), (200.0 + 2.0 * PAD_X) as usize);
        assert_eq!(note.widest(), note.width(&Look { cross: 1.0, ..look }));
    }

    #[test]
    fn words_show_on_the_plate_and_the_corners_are_round() {
        let note = note();
        let stride = note.widest();
        let mut out = vec![0u8; stride * note.height * 4];
        let look = Look { opacity: 1.0, settled: 1.0, arrow: 1.0, cross: 0.0 };
        let w = note.compose(&look, &mut out, stride);
        // The very corner is outside the rounding; the plate lets the
        // desktop through; the words are white over it.
        assert_eq!(alpha(&out, stride, 0, 0), 0);
        let plate = alpha(&out, stride, w - 6, note.height / 2);
        assert_eq!(plate, (PLATE_ALPHA * 255.0).round() as u8);
        let word = (PAD_X as usize + 20, PAD_Y as usize + 10);
        assert_eq!(alpha(&out, stride, word.0, word.1), 255);
        assert_eq!(out[(word.1 * stride + word.0) * 4], 255);
        // Slid all the way over: the song coming in where the other was.
        let over = note.compose(&Look { cross: 1.0, ..look }, &mut out, stride);
        assert_eq!(over, stride);
        assert_eq!(alpha(&out, stride, PAD_X as usize + 150, PAD_Y as usize + 10), 255);
    }

    /// Draws a note's frames with GDI over a light and a dark backdrop, as
    /// PNGs in `NATIVE_YTM_NOTE_DIR`, to look at.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn draw_frames_to_look_at() {
        let dir = std::path::PathBuf::from(std::env::var("NATIVE_YTM_NOTE_DIR").expect("NATIVE_YTM_NOTE_DIR"));
        let change = Change {
            from: Song { title: "Blinding Lights".into(), artist: "The Weeknd".into() },
            to: Song { title: "トリカゴ".into(), artist: "XX:me".into() },
        };
        for scale in [1.0, 1.25] {
            let note = imp::draw_for_test(&change, scale).expect("drawn");
            let course = Course { from: 0.0 };
            let stride = note.widest();
            for (n, t) in [0.15, 0.8, course.cross_at() + 0.15, course.cross_at() + 0.3, course.cross_at() + 0.45, 3.0].into_iter().enumerate() {
                let look = course.look(t).unwrap();
                let mut out = vec![0u8; stride * note.height * 4];
                let w = note.compose(&look, &mut out, stride);
                let pad = 24;
                for (name, backdrop) in [("light", [236u8, 236, 232]), ("dark", [32, 34, 40])] {
                    let (iw, ih) = (w as u32 + pad * 2, note.height as u32 + pad * 2);
                    let mut image = image::RgbImage::from_pixel(iw, ih, image::Rgb(backdrop));
                    for y in 0..note.height {
                        for x in 0..w {
                            let p = &out[(y * stride + x) * 4..][..4];
                            let a = f32::from(p[3]) / 255.0 * look.opacity;
                            let px = image.get_pixel_mut(x as u32 + pad, y as u32 + pad);
                            for (c, i) in [(2usize, 0usize), (1, 1), (0, 2)] {
                                let src = f32::from(p[c]) * look.opacity;
                                px[i] = (src + f32::from(px[i]) * (1.0 - a)).round().min(255.0) as u8;
                            }
                        }
                    }
                    image.save(dir.join(format!("note-{}-{n}-{name}.png", (scale * 100.0) as u32))).unwrap();
                }
            }
        }
    }
}
