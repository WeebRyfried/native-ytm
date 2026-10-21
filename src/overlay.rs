//! The island: as one song gives way to the next while the window is out
//! of sight (minimised, or closed to the taskbar's corner), a black pill
//! comes down from the top of the screen and springs open to the song
//! ending, its artwork beside its title; that slides away as the song
//! coming in slides in, and a few seconds later the island closes and goes
//! back up. Off unless turned on in Settings, Window.
//!
//! Made to cost next to nothing. The island is a layered window on a thread
//! of its own, both made for a change and gone once it has gone back up.
//! Each song's words are drawn once, with GDI, into a mask, and its artwork
//! read once from the file the media overlay has fetched already; a frame
//! is those put together in a small bitmap, made only while something
//! moves (about two seconds of the six), and nothing runs while the island
//! holds still. It never takes the focus or a click, and stays away while a
//! game, a video or a presentation fills the screen.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::theme::{ease_in_out, ease_out};

/// A song as the island shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Song {
    /// The queue's id for it, to give it its artwork if that comes later.
    pub id: u64,
    pub title: String,
    pub artist: String,
    /// Its artwork's file, when that is on disk.
    pub art: Option<PathBuf>,
}

/// One song giving way to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub from: Song,
    pub to: Song,
}

/// The island, from the app's side.
#[derive(Default)]
pub struct Overlay {
    shared: Arc<Mutex<Shared>>,
}

/// Between the app and the island's thread: the change to show next,
/// artwork come after its song's change (the song's id and the file),
/// whether the thread is there to show them, and its id, to wake it with.
#[derive(Default)]
struct Shared {
    pending: Option<Change>,
    art: Vec<(u64, PathBuf)>,
    running: bool,
    thread: u32,
}

impl Overlay {
    /// Shows `change` at the top of the screen, or moves the island already
    /// there on to it.
    pub fn show(&self, change: Change) {
        let mut shared = self.shared.lock();
        shared.pending = Some(change);
        if shared.running {
            imp::wake(shared.thread);
            return;
        }
        let state = Arc::clone(&self.shared);
        shared.running = std::thread::Builder::new().name("song-island".into()).spawn(move || imp::run(&state)).is_ok();
    }

    /// The artwork of song `id`, on disk after its change was shown.
    pub fn art(&self, id: u64, file: PathBuf) {
        let mut shared = self.shared.lock();
        if shared.running {
            shared.art.push((id, file));
            imp::wake(shared.thread);
        }
    }
}

// ---- The course of a change ---------------------------------------------------

/// A spring, as Apple gives them: the seconds of one swing, and how soon it
/// settles (1 without going past, less to go a little past and back).
#[derive(Clone, Copy, Debug)]
struct Spring {
    response: f32,
    damping: f32,
}

impl Spring {
    /// Its way from 0 to 1, `t` seconds after it was let go: exactly 1 once
    /// it stays within a thousandth of it.
    fn at(self, t: f32) -> f32 {
        if t <= 0.0 {
            return 0.0;
        }
        if t >= self.settles() {
            return 1.0;
        }
        let w = std::f32::consts::TAU / self.response;
        let decay = (-self.damping * w * t).exp();
        if self.damping < 1.0 {
            let wd = w * (1.0 - self.damping * self.damping).sqrt();
            1.0 - decay * ((wd * t).cos() + self.damping * w / wd * (wd * t).sin())
        } else {
            1.0 - decay * (1.0 + w * t)
        }
    }

    /// When it has settled: within a thousandth of 1 from then on.
    fn settles(self) -> f32 {
        let w = std::f32::consts::TAU / self.response;
        if self.damping < 1.0 {
            (1000.0 / (1.0 - self.damping * self.damping).sqrt()).ln() / (self.damping * w)
        } else {
            9.25 / w
        }
    }
}

/// The island coming down from above the screen, a little past its place
/// and back; opening from a small pill to its size a moment after it starts
/// down, springing a little past it; and its content coming in.
const DROP: Spring = Spring { response: 0.42, damping: 0.82 };
const OPEN: Spring = Spring { response: 0.5, damping: 0.66 };
const OPEN_AFTER: f32 = 0.08;
const SHOW_AFTER: f32 = 0.18;
const SHOW: f32 = 0.32;
/// When the song coming in takes over: a second after the song ending
/// shows, at once on an island showing already, or a moment after one that
/// was going opens again.
const CROSS_AT: f32 = 1.45;
const CROSS_AGAIN: f32 = 0.1;
const CROSS_REOPENED: f32 = 0.4;
/// The move on: the song ending fading quickly as it slides away, and the
/// one coming in once that has all but gone, sliding in to rest; the island
/// swelling a little as they change, and taking the new song's width.
const GO_FADE: f32 = 0.18;
const GO: f32 = 0.26;
const COME_AFTER: f32 = 0.12;
const COME: f32 = 0.4;
const COME_SLIDE: f32 = 0.5;
const SWELL: f32 = 0.5;
const RESHAPE: Spring = Spring { response: 0.5, damping: 0.78 };
/// How long the song coming in shows, still, before the island goes.
const STAY: f32 = 3.0;
/// Going: closing to the pill as its content goes, and back up.
const HIDE: f32 = 0.18;
const CLOSE: Spring = Spring { response: 0.36, damping: 1.0 };
const RISE_AFTER: f32 = 0.08;
const RISE: f32 = 0.34;
/// Artwork come after its song showed fading in.
const ART_FADE: f32 = 0.25;
/// A frame while something moves: 60 a second.
const FRAME: f32 = 1.0 / 60.0;

/// Eases in (slow, then quicker): for what leaves.
fn ease_in(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t
}

/// How the island looks at a moment of a change.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    /// How far it has come down into its place (0 all above the screen).
    drop: f32,
    /// How far it has opened from the pill to its size (0 to 1, and a
    /// little past as it springs).
    open: f32,
    /// How much of its content shows (0 to 1).
    shown: f32,
    /// The song ending: how much of it shows, and how far it has slid away
    /// (0 to 1).
    from: f32,
    from_slide: f32,
    /// The song coming in: how much of it shows, and how far it has still
    /// to slide in (1 to 0).
    to: f32,
    to_slide: f32,
    /// The island's width from the song ending's to the one coming in's
    /// (0 to 1, springing), and how far it has swelled as they change (0 to
    /// 1 and back).
    reshape: f32,
    swell: f32,
}

impl Look {
    /// The island all gone.
    const GONE: Self = Self {
        drop: 0.0,
        open: 0.0,
        shown: 0.0,
        from: 0.0,
        from_slide: 1.0,
        to: 1.0,
        to_slide: 0.0,
        reshape: 1.0,
        swell: 0.0,
    };
}

/// A change's course: where the island was as the change came (how far
/// down, how open, how much of its content showed: all 0 when there was no
/// island), and when the song coming in takes over.
#[derive(Clone, Copy, Debug)]
struct Course {
    drop: f32,
    open: f32,
    shown: f32,
    cross_at: f32,
}

/// How long the move on takes, all of it settled.
fn crossing() -> f32 {
    GO.max(COME_AFTER + COME_SLIDE).max(SWELL).max(RESHAPE.settles())
}

impl Course {
    /// A new island.
    fn new() -> Self {
        Self { drop: 0.0, open: 0.0, shown: 0.0, cross_at: CROSS_AT }
    }

    /// On from an island that looks like `look`: at once when it is open
    /// and still, a moment after it opens again when it was going.
    fn from(look: &Look) -> Self {
        let still = look.drop >= 1.0 && look.open == 1.0 && look.shown >= 1.0;
        Self {
            drop: look.drop,
            open: look.open,
            shown: look.shown,
            cross_at: if still { CROSS_AGAIN } else { CROSS_REOPENED },
        }
    }

    fn crossed_at(self) -> f32 {
        self.cross_at + crossing()
    }

    fn leave_at(self) -> f32 {
        self.crossed_at() + STAY
    }

    fn gone_at(self) -> f32 {
        self.leave_at() + RISE_AFTER + RISE
    }

    /// When what was still to come in has settled.
    fn in_by(self) -> f32 {
        let mut by: f32 = 0.0;
        if self.drop < 1.0 {
            by = by.max(DROP.settles());
        }
        if self.open != 1.0 {
            by = by.max(OPEN_AFTER * (1.0 - self.open).max(0.0) + OPEN.settles());
        }
        if self.shown < 1.0 {
            by = by.max(SHOW_AFTER * (1.0 - self.shown) + SHOW);
        }
        by
    }

    /// Whether the island is moving on at `t`: a change then waits for it.
    fn crossing(self, t: f32) -> bool {
        (self.cross_at..self.crossed_at()).contains(&t)
    }

    /// How the island looks `t` seconds in; none once it has gone.
    fn look(self, t: f32) -> Option<Look> {
        if t >= self.gone_at() {
            return None;
        }
        // From where it was, on to 1; what is still to come in, after a
        // moment.
        let toward = |from: f32, x: f32| from + (1.0 - from) * x;
        let (drop, open, shown) = if t < self.leave_at() {
            (
                toward(self.drop, DROP.at(t)),
                toward(self.open, OPEN.at(t - OPEN_AFTER * (1.0 - self.open).max(0.0))),
                toward(self.shown, ease_out((t - SHOW_AFTER * (1.0 - self.shown)) / SHOW)),
            )
        } else {
            let u = t - self.leave_at();
            (1.0 - ease_in((u - RISE_AFTER) / RISE), 1.0 - CLOSE.at(u), 1.0 - ease_in_out(u / HIDE))
        };
        let c = t - self.cross_at;
        let swell = if (0.0..SWELL).contains(&c) { (std::f32::consts::PI * c / SWELL).sin() } else { 0.0 };
        Some(Look {
            drop,
            open,
            shown,
            from: 1.0 - ease_out(c / GO_FADE),
            from_slide: ease_in(c / GO),
            to: ease_out((c - COME_AFTER) / COME),
            to_slide: 1.0 - ease_out((c - COME_AFTER) / COME_SLIDE),
            reshape: RESHAPE.at(c),
            swell: swell * swell,
        })
    }

    /// Seconds from `t` until the island looks different: a frame while it
    /// moves, until the next part while it holds still.
    fn still_for(self, t: f32) -> f32 {
        let until = |at: f32| (at - t).max(FRAME);
        if t < self.in_by() || self.crossing(t) || t >= self.leave_at() {
            FRAME
        } else if t < self.cross_at {
            until(self.cross_at)
        } else {
            until(self.leave_at())
        }
    }
}

// ---- The island drawn ---------------------------------------------------------

/// The island's measures at 100% (96 DPI), in pixels: its height open, the
/// pill it opens from, and how far below the top of the screen it rests;
/// the artwork and its corners; the room between the artwork and the words,
/// and after the words; how wide the words may run; the type; how far the
/// words (and the artwork, less) slide as one song gives way to the other,
/// and how much smaller the artwork is at the start or end of its slide.
const HEIGHT: f32 = 56.0;
const PILL_W: f32 = 104.0;
const PILL_H: f32 = 30.0;
const TOP: f32 = 10.0;
const ART: f32 = 38.0;
const ART_RADIUS: f32 = 10.0;
const WORDS_GAP: f32 = 12.0;
const END: f32 = 22.0;
const MIN_WORDS: f32 = 96.0;
const MAX_WORDS: f32 = 260.0;
const TITLE: f32 = 14.5;
const ARTIST: f32 = 12.5;
const LINE_GAP: f32 = 1.0;
const SLIDE: f32 = 18.0;
const ART_SLIDE: f32 = 10.0;
const ART_SHRINK: f32 = 0.1;
/// How much wider and higher the island is at the height of its swell.
const SWELL_W: f32 = 10.0;
const SWELL_H: f32 = 4.0;

/// The shadow under the island: how soft, how far down, how dark; and the
/// room it takes around the island (two blurs, and how far down).
const SHADOW_BLUR: f32 = 12.0;
const SHADOW_Y: f32 = 3.0;
const SHADOW: f32 = 0.35;
const ROOM: f32 = 15.0;

/// A hairline lighter at the island's edge, to show it on black.
const HAIRLINE: f32 = 0.1;

/// How white the title and the artist are, and the artwork's stand-in
/// until it comes.
const TITLE_LEVEL: u8 = 245;
const ARTIST_LEVEL: u8 = 160;
const STAND_IN: [f32; 3] = [0.17, 0.17, 0.19];

/// What was drawn, each pixel how much white (0 to 255).
#[derive(Clone, Debug, Default)]
struct Mask {
    w: usize,
    h: usize,
    a: Vec<u8>,
}

impl Mask {
    /// Its whiteness at `(x, y)`, from 0 to 1: between pixels, blended, so
    /// words move smoothly.
    fn at(&self, x: f32, y: f32) -> f32 {
        if x <= -1.0 || y <= -1.0 || x >= self.w as f32 || y >= self.h as f32 {
            return 0.0;
        }
        let (left, top) = (x.floor(), y.floor());
        let (fx, fy) = (x - left, y - top);
        let (left, top) = (left as isize, top as isize);
        let get = |x: isize, y: isize| {
            if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
                f32::from(self.a[y as usize * self.w + x as usize])
            } else {
                0.0
            }
        };
        let row = |y: isize| get(left, y) * (1.0 - fx) + get(left + 1, y) * fx;
        (row(top) * (1.0 - fy) + row(top + 1) * fy) / 255.0
    }
}

/// Artwork made ready to draw: `px` across, its pixels premultiplied, each
/// part from 0 to 1.
struct Art {
    px: usize,
    rgba: Vec<[f32; 4]>,
}

impl Art {
    /// The artwork in `file`, cut square from its middle and made `px`
    /// across.
    fn load(file: &Path, px: usize) -> Option<Self> {
        let image = image::load_from_memory(&std::fs::read(file).ok()?).ok()?;
        let side = image.width().min(image.height());
        let square = image.crop_imm((image.width() - side) / 2, (image.height() - side) / 2, side, side);
        let small = square.resize_exact(px as u32, px as u32, image::imageops::FilterType::CatmullRom).to_rgba8();
        let rgba = small
            .pixels()
            .map(|p| {
                let a = f32::from(p[3]) / 255.0;
                [f32::from(p[0]) / 255.0 * a, f32::from(p[1]) / 255.0 * a, f32::from(p[2]) / 255.0 * a, a]
            })
            .collect();
        Some(Self { px, rgba })
    }

    /// Its colour at `(x, y)`, blended between pixels, its edges held.
    fn at(&self, x: f32, y: f32) -> [f32; 4] {
        let last = self.px.saturating_sub(1);
        let (x, y) = (x.clamp(0.0, last as f32), y.clamp(0.0, last as f32));
        let (left, top) = (x.floor() as usize, y.floor() as usize);
        let (right, bottom) = ((left + 1).min(last), (top + 1).min(last));
        let (fx, fy) = (x - left as f32, y - top as f32);
        let p = |x: usize, y: usize| self.rgba[y * self.px + x];
        let (a, b, c, d) = (p(left, top), p(right, top), p(left, bottom), p(right, bottom));
        std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    }
}

/// The island's measures on a screen at `scale`, in its pixels: whole and
/// even where they meet the grid, so that the island at rest is sharp.
#[derive(Clone, Copy, Debug)]
struct Metrics {
    scale: f32,
    height: f32,
    art: f32,
    /// How far in the artwork is from the island's end (and its top), and
    /// where the words start.
    inset: f32,
    words_x: f32,
}

impl Metrics {
    fn new(scale: f32) -> Self {
        let even = |v: f32| 2.0 * (v / 2.0).round();
        let height = even(HEIGHT * scale);
        let art = even(ART * scale);
        let inset = (height - art) / 2.0;
        Self { scale, height, art, inset, words_x: inset + art + (WORDS_GAP * scale).round() }
    }

    /// The island's width open around words `words` pixels wide.
    fn width(self, words: usize) -> f32 {
        let w = self.words_x + (words as f32).max(MIN_WORDS * self.scale) + END * self.scale;
        2.0 * (w / 2.0).ceil()
    }
}

/// A song drawn: its words (its title over its artist), its artwork and
/// when that came (in the course's seconds; never, for artwork there from
/// the start), and the island's width open around it.
struct Side {
    id: u64,
    words: Mask,
    art: Option<Art>,
    art_at: f32,
    width: f32,
}

/// A change drawn: the song ending and the one coming in.
struct Island {
    from: Side,
    to: Side,
    m: Metrics,
}

/// Where the island goes on the screen, in pixels: the middle of the
/// primary screen's working area (the taskbar left out), its top, which the
/// island comes down from, and where the island rests below that.
#[derive(Clone, Copy, Debug)]
struct Screen {
    mid: i32,
    top: i32,
    rest: i32,
}

/// A frame placed: the window's place on the screen and its size, and the
/// island's place and size in it, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placed {
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    left: f32,
    top: f32,
    iw: f32,
    ih: f32,
}

/// One song's part of a frame: how much it shows, where its artwork's
/// middle is and how large it is drawn, how far its artwork has come in,
/// and where its words start.
struct Part<'a> {
    side: &'a Side,
    alpha: f32,
    art_mid: (f32, f32),
    art_scale: f32,
    art_in: f32,
    words: (f32, f32),
}

impl Part<'_> {
    /// Its colour at `(x, y)`, premultiplied, and how much it covers.
    fn at(&self, m: &Metrics, x: f32, y: f32) -> ([f32; 3], f32) {
        let (mut c, mut a) = ([0.0f32; 3], 0.0f32);
        // The artwork, its corners rounded, or its stand-in while it comes.
        let half = m.art / 2.0 * self.art_scale;
        let (ux, uy) = (x - self.art_mid.0, y - self.art_mid.1);
        if ux.abs() < half + 1.0 && uy.abs() < half + 1.0 {
            let d = rounded_box(ux, uy, half, ART_RADIUS * m.scale * self.art_scale);
            let cover = (0.5 - d).clamp(0.0, 1.0) * self.alpha;
            if cover > 0.0 {
                let k = self.art_scale;
                let art = self.side.art.as_ref().map_or([0.0; 4], |art| art.at(ux / k + m.art / 2.0 - 0.5, uy / k + m.art / 2.0 - 0.5));
                let t = self.art_in;
                c = std::array::from_fn(|i| (STAND_IN[i] * (1.0 - t) + art[i] * t) * cover);
                a = ((1.0 - t) + art[3] * t) * cover;
            }
        }
        // The words, over it.
        let words = &self.side.words;
        let (wx, wy) = (x - self.words.0 - 0.5, y - self.words.1 - 0.5);
        if wx > -1.0 && wy > -1.0 && wx < words.w as f32 && wy < words.h as f32 {
            let white = words.at(wx, wy) * self.alpha;
            c = c.map(|v| white + v * (1.0 - white));
            a = white + a * (1.0 - white);
        }
        (c, a)
    }
}

impl Island {
    /// The island's width and height with `look`, in pixels.
    fn size(&self, look: &Look) -> (f32, f32) {
        let s = self.m.scale;
        let open = self.from.width + (self.to.width - self.from.width) * look.reshape;
        (
            PILL_W * s + (open - PILL_W * s) * look.open + SWELL_W * s * look.swell,
            PILL_H * s + (self.m.height - PILL_H * s) * look.open + SWELL_H * s * look.swell,
        )
    }

    /// The most room a frame takes: the island at its widest and highest,
    /// as it springs past its size, and its shadow.
    fn room(&self) -> (usize, usize) {
        let around = 2.0 * (ROOM * self.m.scale).round() + 4.0;
        let widest = self.from.width.max(self.to.width);
        ((widest * 1.12 + around).ceil() as usize, (self.m.height * 1.12 + around).ceil() as usize)
    }

    /// Where a frame with `look` goes on `screen`; none while the island is
    /// all above it.
    fn place(&self, look: &Look, screen: &Screen) -> Option<Placed> {
        let s = self.m.scale;
        let (w, h) = self.size(look);
        let room = (ROOM * s).round();
        let left = screen.mid as f32 - w / 2.0;
        // Swelling from its middle.
        let swell = SWELL_H * s * look.swell / 2.0;
        let top = screen.rest as f32 - (1.0 - look.drop) * ((TOP + PILL_H) * s + room) - swell;
        let (x0, x1) = ((left - room).floor(), (left + w + room).ceil());
        let (y0, y1) = ((top - room).floor().max(screen.top as f32), (top + h + room).ceil());
        (y1 > y0).then(|| Placed {
            x: x0 as i32,
            y: y0 as i32,
            w: (x1 - x0) as usize,
            h: (y1 - y0) as usize,
            left: left - x0,
            top: top - y0,
            iw: w,
            ih: h,
        })
    }

    /// How far each song's artwork has faded in at `t`: none without it.
    fn fades(&self, t: f32) -> [f32; 2] {
        [&self.from, &self.to].map(|side| if side.art.is_some() { ease_in_out((t - side.art_at) / ART_FADE) } else { 0.0 })
    }

    /// The two songs' parts of a frame with `look`, the island at `left`,
    /// `top`, `h` high.
    fn parts<'a>(&'a self, look: &Look, fades: [f32; 2], left: f32, top: f32, h: f32) -> [Part<'a>; 2] {
        let m = &self.m;
        let s = m.scale;
        let mid = top + h / 2.0;
        let part = |side: &'a Side, alpha: f32, slide: f32, art_scale: f32, art_in: f32| Part {
            side,
            alpha,
            art_mid: (left + m.inset + m.art / 2.0 + slide * ART_SLIDE * s, mid),
            art_scale,
            art_in,
            words: (left + m.words_x + slide * SLIDE * s, mid - side.words.h as f32 / 2.0),
        };
        [
            part(&self.from, look.shown * look.from, -look.from_slide, 1.0 - ART_SHRINK * look.from_slide, fades[0]),
            part(&self.to, look.shown * look.to, look.to_slide, 1.0 - ART_SHRINK * look.to_slide, fades[1]),
        ]
    }

    /// Puts a frame with `look` together in `out`, premultiplied BGRA rows
    /// `stride` pixels apart, placed as `at` says.
    fn compose(&self, look: &Look, fades: [f32; 2], at: &Placed, out: &mut [u8], stride: usize) {
        let s = self.m.scale;
        let (w, h) = (at.iw, at.ih);
        let r = h / 2.0;
        let (cx, cy) = (at.left + w / 2.0, at.top + h / 2.0);
        // Half the island's straight middle; its ends are half circles.
        let straight = (w / 2.0 - r).max(0.0);
        let (blur, shadow_y) = (SHADOW_BLUR * s / 2.0, SHADOW_Y * s);
        let parts = self.parts(look, fades, at.left, at.top, h);
        let parts: Vec<&Part> = parts.iter().filter(|part| part.alpha > 0.0).collect();
        // The songs at a pixel, the one coming in over the one ending.
        let content = |px: f32, py: f32| {
            let (mut c, mut a) = ([0.0f32; 3], 0.0f32);
            for part in &parts {
                let (pc, pa) = part.at(&self.m, px, py);
                c = std::array::from_fn(|i| pc[i] + c[i] * (1.0 - pa));
                a = pa + a * (1.0 - pa);
            }
            (c, a)
        };
        // Further in than this from its edge, the island is all black but
        // for the songs: no edge, hairline or shadow to work out.
        let inner = r - 1.75;
        for y in 0..at.h {
            let py = y as f32 + 0.5;
            let (dy, shadow_dy) = (py - cy, py - cy - shadow_y);
            let row = &mut out[y * stride * 4..][..at.w * 4];
            // A row neither the island nor its shadow reaches.
            if dy.abs() >= r + 0.5 && shadow_dy.abs() >= r + 2.0 * blur {
                row.fill(0);
                continue;
            }
            let inside = if dy.abs() < inner { straight + (inner * inner - dy * dy).sqrt() } else { -1.0 };
            for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
                let px = x as f32 + 0.5;
                if (px - cx).abs() < inside {
                    let (c, _) = content(px, py);
                    for (i, channel) in [2, 1, 0].into_iter().enumerate() {
                        pixel[i] = (c[channel] * 255.0).round().min(255.0) as u8;
                    }
                    pixel[3] = 255;
                    continue;
                }
                let dx = ((px - cx).abs() - straight).max(0.0);
                let d = dx.hypot(dy) - r;
                let cover = (0.5 - d).clamp(0.0, 1.0);
                let shadow = if cover < 1.0 { SHADOW * shade((dx.hypot(shadow_dy) - r) / blur) } else { 0.0 };
                if cover <= 0.0 && shadow < 1.0 / 512.0 {
                    pixel.fill(0);
                    continue;
                }
                // The songs on the island, black, a hairline lighter at its
                // edge, and its shadow under it all.
                let (c, a) = if cover > 0.0 { content(px, py) } else { ([0.0; 3], 0.0) };
                let a = a * cover;
                let body = cover * (1.0 - a);
                let ring = (1.0 - (d + 0.75).abs()).clamp(0.0, 1.0) * HAIRLINE * body;
                let alpha = a + body;
                let alpha = alpha + shadow * (1.0 - alpha);
                for (i, channel) in [2, 1, 0].into_iter().enumerate() {
                    pixel[i] = ((c[channel] * cover + ring) * 255.0).round().min(255.0) as u8;
                }
                pixel[3] = (alpha * 255.0).round().min(255.0) as u8;
            }
        }
    }
}

/// How far `(x, y)` is outside a square reaching `half` either way of the
/// origin, its corners rounded by `r` (less than nothing inside it).
fn rounded_box(x: f32, y: f32, half: f32, r: f32) -> f32 {
    let qx = x.abs() - (half - r);
    let qy = y.abs() - (half - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// The shadow's depth at `z` blurs from the island's edge (outward): all
/// of it well inside, half about at the edge, none two blurs out.
fn shade(z: f32) -> f32 {
    1.0 - ease_in_out((z + 1.6) / 3.6)
}

// ---- Windows -----------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use std::path::Path;
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
        PM_REMOVE, PeekMessageW, PostThreadMessageW, QS_ALLINPUT, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE,
        ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_APP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    use super::{
        ART_FADE, ARTIST, ARTIST_LEVEL, Art, COME_AFTER, Change, Course, FRAME, GO_FADE, Island, LINE_GAP, Look, MAX_WORDS,
        Mask, Metrics, Screen, Shared, Side, Song, TITLE, TITLE_LEVEL, TOP,
    };

    /// Wakes the island's thread: a change, or artwork, is waiting.
    pub(super) fn wake(thread: u32) {
        if thread != 0 {
            // SAFETY: a message with no pointers in it, to a thread id; a
            // thread gone or without a queue yet only makes it fail.
            unsafe { PostThreadMessageW(thread, WM_APP, 0, 0) };
        }
    }

    /// The island's thread: shows each change as it comes, and ends once the
    /// island has gone back up with nothing more waiting.
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
            let (change, arts) = {
                let mut shared = shared.lock();
                // A change waits while the island moves on to the one before.
                let moving_on = shown.as_ref().is_some_and(Shown::crossing);
                let change = if moving_on { None } else { shared.pending.take() };
                if change.is_none() && shown.is_none() {
                    shared.running = false;
                    shared.thread = 0;
                    shared.art.clear();
                    return;
                }
                (change, std::mem::take(&mut shared.art))
            };
            if let Some(change) = change {
                match &mut shown {
                    Some(island) => island.change_to(&change),
                    // Nothing over a game, a video or a presentation.
                    None if screen_busy() => log::info!("song change not shown: something fills the screen"),
                    None => shown = Shown::new(&change),
                }
            }
            let Some(island) = &mut shown else {
                continue;
            };
            for (id, file) in arts {
                island.art_came(id, &file);
            }
            let t = island.now();
            let Some(look) = island.course.look(t) else {
                shown = None;
                continue;
            };
            island.present(&look, t);
            let wait = island.still_for(t);
            if wait <= FRAME {
                island.next_frame();
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
    /// Call on the island's thread.
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

    /// The primary screen: where the island goes on it, and its scale.
    fn screen() -> Option<(Screen, f32)> {
        // SAFETY: plain calls; the info is written by GetMonitorInfoW and
        // the DPI by GetDpiForMonitor.
        unsafe {
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
            let scale = dpi as f32 / 96.0;
            let work = info.rcWork;
            let top = work.top;
            Some((Screen { mid: (work.left + work.right) / 2, top, rest: top + (TOP * scale).round() as i32 }, scale))
        }
    }

    /// A song made ready to show: its words drawn and its artwork read.
    fn side(song: &Song, m: Metrics) -> Option<Side> {
        let words = draw_words(song, m.scale)?;
        let art = song.art.as_deref().and_then(|file| Art::load(file, m.art as usize));
        Some(Side { id: song.id, width: m.width(words.w), words, art, art_at: f32::NEG_INFINITY })
    }

    /// The island on the screen: its window, the bitmap its frames are put
    /// together in, the change drawn, and where its course is.
    struct Shown {
        window: HWND,
        surface: Surface,
        island: Island,
        course: Course,
        started: Instant,
        screen: Screen,
        /// What was last put on the screen: the look and the artwork's
        /// fades it was made for, to make it again only when those move.
        made: Option<(Look, [f32; 2])>,
        visible: bool,
        last_frame: Instant,
        /// The frames made, and the time spent making and showing them.
        frames: u32,
        busy: Duration,
    }

    impl Shown {
        fn new(change: &Change) -> Option<Self> {
            let (screen, scale) = screen()?;
            let m = Metrics::new(scale);
            let island = Island { from: side(&change.from, m)?, to: side(&change.to, m)?, m };
            let (w, h) = island.room();
            let surface = Surface::new(w, h)?;
            let class = wide("NativeYtmSongIsland");
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
                    screen.mid,
                    screen.top,
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
            log::info!("song change shown at the top of the screen ({}, {}), at {:.0}%", screen.mid, screen.rest, scale * 100.0);
            Some(Self {
                window,
                surface,
                island,
                course: Course::new(),
                started: Instant::now(),
                screen,
                made: None,
                visible: false,
                last_frame: Instant::now(),
                frames: 0,
                busy: Duration::ZERO,
            })
        }

        /// Seconds into the course.
        fn now(&self) -> f32 {
            self.started.elapsed().as_secs_f32()
        }

        fn crossing(&self) -> bool {
            self.course.crossing(self.now())
        }

        /// Another change while the island shows: on to it from where the
        /// island is.
        fn change_to(&mut self, change: &Change) {
            let t = self.now();
            let Some(to) = side(&change.to, self.island.m) else {
                return;
            };
            if t < self.course.cross_at {
                // Not moved on yet: on to this one instead.
                self.island.to = to;
            } else {
                // Moved on: from the song showing now.
                let look = self.course.look(t).unwrap_or(Look::GONE);
                let mut from = std::mem::replace(&mut self.island.to, to);
                from.art_at = f32::NEG_INFINITY;
                self.island.from = from;
                self.course = Course::from(&look);
                self.started = Instant::now();
            }
            let (w, h) = self.island.room();
            if (w > self.surface.w || h > self.surface.h)
                && let Some(surface) = Surface::new(w.max(self.surface.w), h.max(self.surface.h))
            {
                self.surface = surface;
            }
            self.made = None;
        }

        /// Artwork for song `id` come: it fades in if its song shows.
        fn art_came(&mut self, id: u64, file: &Path) {
            let t = self.now();
            let px = self.island.m.art as usize;
            let cross_at = self.course.cross_at;
            let island = &mut self.island;
            for (side, seen) in [(&mut island.from, t < cross_at + GO_FADE), (&mut island.to, t >= cross_at + COME_AFTER)] {
                if side.id == id && side.art.is_none() {
                    side.art = Art::load(file, px);
                    side.art_at = if seen { t } else { f32::NEG_INFINITY };
                    self.made = None;
                }
            }
        }

        /// Seconds until the island looks different.
        fn still_for(&self, t: f32) -> f32 {
            let fading = [&self.island.from, &self.island.to].iter().any(|side| side.art.is_some() && t - side.art_at < ART_FADE);
            if fading { FRAME } else { self.course.still_for(t) }
        }

        /// Puts the island on the screen as it looks now, making the frame
        /// again only when something in it has moved.
        fn present(&mut self, look: &Look, t: f32) {
            let fades = self.island.fades(t);
            if self.made == Some((*look, fades)) {
                return;
            }
            self.made = Some((*look, fades));
            let Some(mut at) = self.island.place(look, &self.screen) else {
                if self.visible {
                    self.visible = false;
                    // SAFETY: the window is this island's own.
                    unsafe { ShowWindow(self.window, SW_HIDE) };
                }
                return;
            };
            let started = Instant::now();
            at.w = at.w.min(self.surface.w);
            at.h = at.h.min(self.surface.h);
            let stride = self.surface.w;
            self.island.compose(look, fades, &at, self.surface.pixels(), stride);
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            // SAFETY: the window and the bitmap's DC are this island's own,
            // and the bitmap is at least the size given.
            unsafe {
                let size = SIZE { cx: at.w as i32, cy: at.h as i32 };
                UpdateLayeredWindow(
                    self.window,
                    null_mut(),
                    &POINT { x: at.x, y: at.y },
                    &size,
                    self.surface.dc,
                    &POINT { x: 0, y: 0 },
                    0,
                    &blend,
                    ULW_ALPHA,
                );
                if !self.visible {
                    self.visible = true;
                    ShowWindow(self.window, SW_SHOWNOACTIVATE);
                }
            }
            self.frames += 1;
            self.busy += started.elapsed();
        }

        /// Waits for the screen's next frame, at about 60 a second: on a
        /// screen faster than that, every second or third of its frames
        /// (72 a second at 144 Hz, 60 at 120).
        fn next_frame(&mut self) {
            let gap = Duration::from_secs_f32(FRAME * 0.7);
            // SAFETY: plain calls, on the island's thread.
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
            log::info!("island gone: {} frames, {:.1} ms to make and show them", self.frames, self.busy.as_secs_f64() * 1000.0);
            // SAFETY: the window is this island's own.
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

    /// A song's words drawn with GDI at `scale`: its title over its artist,
    /// each cut short with an ellipsis past the room there is.
    fn draw_words(song: &Song, scale: f32) -> Option<Mask> {
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
            let (title, artist) = (font(TITLE, FW_SEMIBOLD), font(ARTIST, FW_NORMAL));
            let line = measure(dc, title, "Ag").1 + (LINE_GAP * scale).round() as i32;
            let parts = [(song.title.as_str(), title, 0, TITLE_LEVEL), (song.artist.as_str(), artist, line, ARTIST_LEVEL)];
            let mut mask = words(dc, &parts, MAX_WORDS * scale);
            for object in [title, artist] {
                DeleteObject(object);
            }
            DeleteDC(dc);
            // As even in height as the island, so the words sit on whole
            // pixels in its middle.
            if mask.h % 2 == 1 {
                mask.h += 1;
                mask.a.resize(mask.w * mask.h, 0);
            }
            Some(mask)
        }
    }

    /// The face the island is set in: Windows 11's Segoe UI Variable, else
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
        let room = room.max(1.0) as i32;
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
    pub(super) fn side_for_test(song: &Song, m: Metrics) -> Option<Side> {
        side(song, m)
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
        shared.art.clear();
        shared.running = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn springs_settle_on_their_mark() {
        for spring in [DROP, OPEN, RESHAPE, CLOSE] {
            assert_eq!(spring.at(0.0), 0.0);
            assert_eq!(spring.at(spring.settles()), 1.0);
            let near = spring.at(spring.settles() - 0.001);
            assert!((near - 1.0).abs() < 2e-3, "{near}");
        }
        // A little past and back when it gives, never past when it does not.
        let past = (0..200).map(|i| OPEN.at(i as f32 * 0.005)).fold(0.0, f32::max);
        assert!((1.03..1.1).contains(&past), "{past}");
        assert!((0..200).all(|i| CLOSE.at(i as f32 * 0.005) <= 1.0));
    }

    #[test]
    fn an_island_comes_down_opens_moves_on_and_goes() {
        let course = Course::new();
        let start = course.look(0.0).unwrap();
        assert_eq!((start.drop, start.open, start.shown, start.from, start.to), (0.0, 0.0, 0.0, 1.0, 0.0));
        // Open and still, the song ending showing: nothing to draw.
        assert!(course.in_by() < course.cross_at - 0.3);
        assert_eq!(course.look(course.cross_at - 0.05).unwrap(), REST);
        assert!(course.still_for(course.in_by() + 0.01) > 0.3, "holding still, no frames");
        // Moving on, then the song coming in alone, still.
        assert_eq!(course.still_for(course.cross_at + 0.1), FRAME);
        assert!(course.crossing(course.cross_at + 0.1));
        // The song ending all but gone before the one coming in shows.
        let both = course.look(course.cross_at + COME_AFTER).unwrap();
        assert!(both.from < 0.1 && both.to < 1e-3, "{both:?}");
        assert!(course.look(course.cross_at + SWELL / 2.0).unwrap().swell > 0.99);
        assert_eq!(course.look(course.crossed_at() + 0.001).unwrap(), MOVED);
        assert!(course.still_for(course.crossed_at() + 0.01) > 2.5);
        // Going back up, then gone: about six seconds in all.
        assert!(course.look(course.leave_at() + 0.2).unwrap().shown < 0.05);
        assert!(course.look(course.gone_at() - 0.001).unwrap().drop < 0.01);
        assert_eq!(course.look(course.gone_at()), None);
        assert!((5.0..7.0).contains(&course.gone_at()), "{}", course.gone_at());
    }

    #[test]
    fn a_change_on_an_open_island_moves_on_at_once() {
        let first = Course::new();
        let open = first.look(first.crossed_at() + 1.0).unwrap();
        let course = Course::from(&open);
        assert_eq!(course.cross_at, CROSS_AGAIN);
        assert_eq!(course.look(0.0).unwrap(), REST);
        assert_eq!(course.in_by(), 0.0);
        assert!((course.still_for(0.0) - CROSS_AGAIN).abs() < 1e-6);
    }

    #[test]
    fn a_change_as_the_island_goes_opens_it_again() {
        let first = Course::new();
        let going = first.look(first.leave_at() + 0.2).unwrap();
        assert!(going.open < 0.9 && going.shown < 0.1);
        let course = Course::from(&going);
        assert_eq!(course.cross_at, CROSS_REOPENED);
        // On from where it was.
        let start = course.look(0.0).unwrap();
        assert_eq!((start.drop, start.open, start.shown), (going.drop, going.open, going.shown));
        assert!(course.look(CROSS_REOPENED).unwrap().open > 0.95);
    }

    fn block(w: usize, h: usize) -> Mask {
        Mask { w, h, a: vec![255; w * h] }
    }

    fn red(px: usize) -> Art {
        Art { px, rgba: vec![[1.0, 0.0, 0.0, 1.0]; px * px] }
    }

    fn island() -> Island {
        let m = Metrics::new(1.0);
        let side = |id, w| Side { id, words: block(w, 30), art: Some(red(m.art as usize)), art_at: f32::NEG_INFINITY, width: m.width(w) };
        Island { from: side(1, 60), to: side(2, 200), m }
    }

    const SCREEN: Screen = Screen { mid: 400, top: 0, rest: 10 };

    /// Open and still, the song ending showing; and moved on to the one
    /// coming in.
    const REST: Look = Look {
        drop: 1.0,
        open: 1.0,
        shown: 1.0,
        from: 1.0,
        from_slide: 0.0,
        to: 0.0,
        to_slide: 1.0,
        reshape: 0.0,
        swell: 0.0,
    };
    const MOVED: Look = Look { from: 0.0, from_slide: 1.0, to: 1.0, to_slide: 0.0, reshape: 1.0, ..REST };

    fn pixel(out: &[u8], stride: usize, x: usize, y: usize) -> [u8; 4] {
        out[(y * stride + x) * 4..][..4].try_into().unwrap()
    }

    #[test]
    fn the_island_opens_from_the_pill_to_the_song_showing() {
        let island = island();
        let rest = REST;
        assert_eq!(island.size(&Look { open: 0.0, ..rest }), (PILL_W, PILL_H));
        // At least the narrowest; then as wide as the song coming in.
        assert_eq!(island.size(&rest), (island.m.width(0), HEIGHT));
        // The artwork, the words, the room after them, to an even width.
        assert_eq!(island.size(&rest).0, 178.0);
        assert_eq!(island.size(&Look { reshape: 1.0, ..rest }).0, 282.0);
        // Swelling a little as the songs change.
        assert_eq!(island.size(&Look { swell: 1.0, ..rest }), (178.0 + SWELL_W, HEIGHT + SWELL_H));
        let (w, h) = island.room();
        assert!(w as f32 >= island.size(&Look { reshape: 1.0, open: 1.07, ..rest }).0 + 2.0 * ROOM);
        assert!(h as f32 >= HEIGHT * 1.07 + 2.0 * ROOM);
    }

    #[test]
    fn the_island_comes_from_above_the_screen() {
        let island = island();
        let rest = REST;
        // All above it at first, then in its place, on whole pixels.
        assert_eq!(island.place(&Look { drop: 0.0, open: 0.0, ..rest }, &SCREEN), None);
        let at = island.place(&rest, &SCREEN).unwrap();
        assert_eq!((at.x + at.left as i32, at.y + at.top as i32), (400 - island.size(&rest).0 as i32 / 2, 10));
        assert_eq!((at.left.fract(), at.top.fract()), (0.0, 0.0));
        // Never over the screen above.
        let partly = island.place(&Look { drop: 0.5, open: 0.3, ..rest }, &SCREEN).unwrap();
        assert_eq!(partly.y, 0);
        assert!(partly.top < 0.0);
    }

    #[test]
    fn the_island_is_black_round_ended_its_artwork_and_words_on_it() {
        let island = island();
        let look = REST;
        let at = island.place(&look, &SCREEN).unwrap();
        let (stride, _) = island.room();
        let mut out = vec![0u8; stride * at.h * 4];
        island.compose(&look, island.fades(0.0), &at, &mut out, stride);
        let (left, top) = (at.left as usize, at.top as usize);
        let (w, h) = (at.iw as usize, at.ih as usize);
        // Round ends: the very corner is only shadow, faint.
        assert!(pixel(&out, stride, left + 1, top + 1)[3] < 100);
        // Black between the words and the end.
        assert_eq!(pixel(&out, stride, left + w - 14, top + h / 2), [0, 0, 0, 255]);
        // The artwork, red, and the words, white.
        let art = (left + 9 + 19, top + h / 2);
        assert_eq!(pixel(&out, stride, art.0, art.1), [0, 0, 255, 255]);
        assert_eq!(pixel(&out, stride, left + 59 + 20, top + h / 2), [255, 255, 255, 255]);
        // A faint shadow below, nothing far out.
        let below = pixel(&out, stride, left + w / 2, top + h + 3);
        assert!((10..120).contains(&below[3]) && below[..3] == [0, 0, 0]);
        assert_eq!(pixel(&out, stride, 0, at.h - 1), [0, 0, 0, 0]);
        // Moved on: the song coming in where the other was, its words wider.
        let moved = MOVED;
        let at = island.place(&moved, &SCREEN).unwrap();
        island.compose(&moved, island.fades(0.0), &at, &mut out, stride);
        let (left, top) = (at.left as usize, at.top as usize);
        assert_eq!(pixel(&out, stride, left + 59 + 180, top + at.ih as usize / 2), [255, 255, 255, 255]);
    }

    #[test]
    fn artwork_still_to_come_has_a_stand_in_then_fades_in() {
        let mut island = island();
        island.to.art = None;
        assert_eq!(island.fades(1.0), [1.0, 0.0]);
        island.to.art = Some(red(38));
        island.to.art_at = 2.0;
        assert_eq!(island.fades(2.0)[1], 0.0);
        assert_eq!(island.fades(2.0 + ART_FADE)[1], 1.0);
    }

    /// Draws a change's frames with GDI over a light and a dark backdrop,
    /// stacked in a PNG for each in `NATIVE_YTM_ISLAND_DIR`, to look at;
    /// with artwork from `NATIVE_YTM_ISLAND_ART` (two files, `;` between),
    /// and the time each frame takes to put together.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn draw_frames_to_look_at() {
        let dir = std::path::PathBuf::from(std::env::var("NATIVE_YTM_ISLAND_DIR").expect("NATIVE_YTM_ISLAND_DIR"));
        let arts: Vec<PathBuf> = std::env::var("NATIVE_YTM_ISLAND_ART").unwrap_or_default().split(';').filter(|p| !p.is_empty()).map(PathBuf::from).collect();
        let song = |id, title: &str, artist: &str| Song { id, title: title.into(), artist: artist.into(), art: arts.get(id as usize - 1).cloned() };
        let change = Change { from: song(1, "Blinding Lights", "The Weeknd"), to: song(2, "アイドル", "YOASOBI") };
        for scale in [1.0, 1.25] {
            let m = Metrics::new(scale);
            let island = Island { from: imp::side_for_test(&change.from, m).unwrap(), to: imp::side_for_test(&change.to, m).unwrap(), m };
            let course = Course::new();
            let c = course.cross_at;
            let l = course.leave_at();
            let times = [
                0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.4, 0.5, 0.7, 1.2,
                c + 0.04, c + 0.08, c + 0.12, c + 0.16, c + 0.2, c + 0.25, c + 0.3, c + 0.4, c + 0.55, c + 1.5,
                l + 0.05, l + 0.1, l + 0.15, l + 0.2, l + 0.25, l + 0.3, l + 0.35, l + 0.4, l + 0.45,
            ];
            let (stride, room_h) = island.room();
            let (cell_w, cell_h) = ((440.0 * scale) as u32, (120.0 * scale) as u32);
            let screen = Screen { mid: cell_w as i32 / 2, top: 0, rest: (TOP * scale).round() as i32 };
            for (name, backdrop) in [("light", [236u8, 236, 232]), ("dark", [32, 34, 40])] {
                let mut sheet = image::RgbImage::from_pixel(cell_w, cell_h * times.len() as u32, image::Rgb(backdrop));
                let mut spent = std::time::Duration::ZERO;
                for (n, &t) in times.iter().enumerate() {
                    let Some(look) = course.look(t) else { continue };
                    let Some(at) = island.place(&look, &screen) else { continue };
                    let mut out = vec![0u8; stride * room_h * 4];
                    let started = std::time::Instant::now();
                    island.compose(&look, island.fades(t), &at, &mut out, stride);
                    spent += started.elapsed();
                    // The screen's top edge, as a line.
                    for x in 0..cell_w {
                        sheet.put_pixel(x, n as u32 * cell_h, image::Rgb([128, 128, 128]));
                    }
                    for y in 0..at.h {
                        for x in 0..at.w {
                            let p = &out[(y * stride + x) * 4..][..4];
                            let a = f32::from(p[3]) / 255.0;
                            let (sx, sy) = (at.x as u32 + x as u32, n as u32 * cell_h + 1 + at.y as u32 + y as u32);
                            let px = sheet.get_pixel_mut(sx, sy);
                            for (c, i) in [(2usize, 0usize), (1, 1), (0, 2)] {
                                px[i] = (f32::from(p[c]) + f32::from(px[i]) * (1.0 - a)).round().min(255.0) as u8;
                            }
                        }
                    }
                }
                sheet.save(dir.join(format!("island-{}-{name}.png", (scale * 100.0) as u32))).unwrap();
                eprintln!("{scale}: {:.3} ms a frame", spent.as_secs_f64() * 1000.0 / times.len() as f64);
            }
        }
    }
}
