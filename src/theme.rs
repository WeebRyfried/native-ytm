//! Colours, sizes, fonts and icons. YouTube Music is dark only, and so is
//! this: four dark looks to choose from, each near-black surfaces, light
//! text, and one colour for what is playing.

use std::sync::atomic::{AtomicU8, Ordering};

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

/// One look of the app.
pub struct Palette {
    pub bg: Color32,
    pub sidebar: Color32,
    pub player: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub surface_active: Color32,
    pub popup: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    /// The accent as text on dark surfaces: a little lighter, to read.
    pub accent_text: Color32,
    pub divider: Color32,
    /// The visualiser's second colour: the accent turns into it across.
    pub glow: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The looks on offer, in Settings' order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    /// YouTube Music's own: black and red.
    #[default]
    Classic,
    /// Blue-black, with a blue that turns violet.
    Midnight,
    /// Plum, with cherry-blossom pink.
    Sakura,
    /// Green-black, with mint.
    Matcha,
}

impl Theme {
    pub const ALL: [Theme; 4] = [Theme::Classic, Theme::Midnight, Theme::Sakura, Theme::Matcha];

    pub fn name(self) -> &'static str {
        match self {
            Theme::Classic => "Classic",
            Theme::Midnight => "Midnight",
            Theme::Sakura => "Sakura",
            Theme::Matcha => "Matcha",
        }
    }

    pub fn palette(self) -> &'static Palette {
        &PALETTES[self as usize]
    }
}

const PALETTES: [Palette; 4] = [
    Palette {
        bg: rgb(0x0b0b0b),
        sidebar: rgb(0x030303),
        player: rgb(0x171717),
        surface: rgb(0x1d1d1d),
        surface_hover: rgb(0x292929),
        surface_active: rgb(0x333333),
        popup: rgb(0x212121),
        text: rgb(0xf1f1f1),
        text_dim: rgb(0xaaaaaa),
        text_faint: rgb(0x717171),
        accent: rgb(0xff0033),
        accent_text: rgb(0xff4d5e),
        divider: rgb(0x262626),
        glow: rgb(0xff6a8e),
    },
    Palette {
        bg: rgb(0x0a0d15),
        sidebar: rgb(0x05070d),
        player: rgb(0x131926),
        surface: rgb(0x172030),
        surface_hover: rgb(0x212b3e),
        surface_active: rgb(0x2a354b),
        popup: rgb(0x1a2333),
        text: rgb(0xeef2fa),
        text_dim: rgb(0x9ba7bd),
        text_faint: rgb(0x667189),
        accent: rgb(0x4c8dff),
        accent_text: rgb(0x7eaaff),
        divider: rgb(0x1f2839),
        glow: rgb(0xa47bff),
    },
    Palette {
        bg: rgb(0x110b0f),
        sidebar: rgb(0x0a0609),
        player: rgb(0x1c1319),
        surface: rgb(0x23171f),
        surface_hover: rgb(0x2f2029),
        surface_active: rgb(0x3a2934),
        popup: rgb(0x271a23),
        text: rgb(0xf8eef3),
        text_dim: rgb(0xbca6b1),
        text_faint: rgb(0x816c77),
        accent: rgb(0xff5c9d),
        accent_text: rgb(0xff8ab8),
        divider: rgb(0x2d1f28),
        glow: rgb(0xffb3d0),
    },
    Palette {
        bg: rgb(0x0a0f0c),
        sidebar: rgb(0x050906),
        player: rgb(0x131a16),
        surface: rgb(0x18211c),
        surface_hover: rgb(0x212c25),
        surface_active: rgb(0x2a372f),
        popup: rgb(0x1b251f),
        text: rgb(0xeef5f0),
        text_dim: rgb(0xa0b3a7),
        text_faint: rgb(0x6a7c71),
        accent: rgb(0x2fcf8a),
        accent_text: rgb(0x6be3b0),
        divider: rgb(0x1f2a23),
        glow: rgb(0x5ee0e6),
    },
];

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Takes on `theme`; `apply` then redoes egui's own colours.
pub fn set(theme: Theme) {
    CURRENT.store(theme as u8, Ordering::Relaxed);
}

pub fn current() -> Theme {
    Theme::ALL[usize::from(CURRENT.load(Ordering::Relaxed)).min(Theme::ALL.len() - 1)]
}

fn palette() -> &'static Palette {
    current().palette()
}

pub fn bg() -> Color32 {
    palette().bg
}
pub fn sidebar() -> Color32 {
    palette().sidebar
}
pub fn player() -> Color32 {
    palette().player
}
pub fn surface() -> Color32 {
    palette().surface
}
pub fn surface_hover() -> Color32 {
    palette().surface_hover
}
pub fn surface_active() -> Color32 {
    palette().surface_active
}
pub fn popup() -> Color32 {
    palette().popup
}
pub fn text() -> Color32 {
    palette().text
}
pub fn text_dim() -> Color32 {
    palette().text_dim
}
pub fn text_faint() -> Color32 {
    palette().text_faint
}
pub fn accent() -> Color32 {
    palette().accent
}
pub fn accent_text() -> Color32 {
    palette().accent_text
}
pub fn divider() -> Color32 {
    palette().divider
}
pub fn glow() -> Color32 {
    palette().glow
}

pub const SCRIM: Color32 = Color32::from_black_alpha(160);

pub const PLAYER_HEIGHT: f32 = 80.0;
pub const SIDEBAR_WIDTH: f32 = 232.0;
pub const TOPBAR_HEIGHT: f32 = 60.0;
pub const SIDE_PANEL_WIDTH: f32 = 360.0;
pub const ROW_HEIGHT: f32 = 56.0;
pub const PAGE_MARGIN: f32 = 32.0;
pub const CARD: f32 = 176.0;

/// A repaint after `delay`, really: egui takes a predicted frame off every
/// delay asked for, which would make a "33 ms" animation run at 60 frames
/// a second and a "16 ms" one at the screen's full rate.
pub fn repaint_in(ctx: &egui::Context, delay: std::time::Duration) {
    let frame = ctx.input(|i| i.predicted_dt);
    ctx.request_repaint_after(delay + std::time::Duration::from_secs_f32(frame.max(0.0)));
}

/// The next frame of an animation the app drives itself, at 60 frames a
/// second at most: smooth, and on a 144 Hz screen less than half the
/// frames of an immediate repaint. Input still repaints at once.
pub fn next_frame(ctx: &egui::Context) {
    repaint_in(ctx, std::time::Duration::from_micros(16_667));
}

/// Eases out (fast, then settling): for things arriving.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Eases in and out (slow, quicker, slow): for things that move and stop,
/// and for fading away gently.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Eases in and out over the first and last fifth, at an even pace between
/// (a fifth faster than going straight through): for fading out, so that no
/// part of the fade rushes, as the middle of [`ease_in_out`] does.
pub fn ease_even(t: f32) -> f32 {
    const EASE: f32 = 0.2;
    let t = t.clamp(0.0, 1.0);
    let top = 1.0 / (1.0 - EASE);
    if t < EASE {
        top * t * t / (2.0 * EASE)
    } else if t <= 1.0 - EASE {
        top * (t - EASE / 2.0)
    } else {
        1.0 - top * (1.0 - t) * (1.0 - t) / (2.0 * EASE)
    }
}

/// How far, from 0 to 1 and eased, something has come in since `key` last
/// changed in `slot` (a new page, a new song), over `secs`. Frames keep
/// coming only until it is done.
pub fn appear(ctx: &egui::Context, slot: egui::Id, key: u64, secs: f32) -> f32 {
    let now = ctx.input(|i| i.time);
    let start = ctx.data_mut(|d| {
        let entry = d.get_temp_mut_or_insert_with(slot, || (key, now));
        if entry.0 != key {
            *entry = (key, now);
        }
        entry.1
    });
    let t = ((now - start) as f32 / secs).clamp(0.0, 1.0);
    if t < 1.0 {
        next_frame(ctx);
    }
    ease_out(t)
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn medium(size: f32) -> FontId {
    fastframe_fonts::Weight::Medium.font_id(size)
}

pub fn semibold(size: f32) -> FontId {
    fastframe_fonts::Weight::SemiBold.font_id(size)
}

pub fn bold(size: f32) -> FontId {
    fastframe_fonts::Weight::Bold.font_id(size)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = fastframe_fonts::FontSetup::default()
        .primary(fastframe_fonts::Primary::Inter)
        .definitions();
    fastframe_text::detect().apply_to(&mut fonts);
    ctx.set_fonts(fonts);
    egui_extras::install_image_loaders(ctx);
    fastframe_icons::install::<Icon>(ctx);
    apply(ctx);
    style(ctx);
}

/// egui's own colours, from the theme: on start, and on a change of theme.
pub fn apply(ctx: &egui::Context) {
    let p = palette();
    let mut visuals = Visuals::dark();
    visuals.panel_fill = p.bg;
    visuals.window_fill = p.popup;
    visuals.extreme_bg_color = p.surface;
    visuals.faint_bg_color = p.surface;
    visuals.override_text_color = Some(p.text);
    visuals.hyperlink_color = p.text;
    visuals.selection.bg_fill = p.accent.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, p.text);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_stroke = Stroke::new(1.0, p.divider);
    visuals.window_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    visuals.popup_shadow = visuals.window_shadow;
    let w = &mut visuals.widgets;
    w.noninteractive.bg_fill = p.bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.divider);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text_dim);
    for (state, fill) in [
        (&mut w.inactive, p.surface),
        (&mut w.hovered, p.surface_hover),
        (&mut w.active, p.surface_active),
        (&mut w.open, p.surface_hover),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke = Stroke::new(1.5, p.text);
        state.corner_radius = CornerRadius::same(8);
        state.expansion = 0.0;
    }
    w.hovered.bg_stroke = Stroke::NONE;
    fastframe_text::detect().apply_to_visuals(&mut visuals);
    // Dark whatever Windows uses for apps: a light system theme would
    // otherwise hand egui its light visuals.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);
}

fn style(ctx: &egui::Context) {
    ctx.global_style_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, regular(12.0)),
            (TextStyle::Body, regular(14.0)),
            (TextStyle::Button, regular(14.0)),
            (TextStyle::Heading, bold(22.0)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size.y = 28.0;
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = true;
        style.spacing.scroll.floating_allocated_width = 0.0;
        style.spacing.scroll.dormant_handle_opacity = 0.0;
        style.spacing.scroll.active_handle_opacity = 0.6;
        style.spacing.scroll.interact_handle_opacity = 0.9;
        style.spacing.menu_margin = egui::Margin::same(6);
        style.interaction.selectable_labels = false;
        style.interaction.tooltip_delay = 0.5;
        style.animation_time = 0.12;
    });
}

fastframe_icons::icons! {
    /// Every icon the interface draws.
    pub enum Icon {
        prefix: "ytm-icon-",
        directory: "../assets/icons/",
        Play => "play-fill",
        Pause => "pause-fill",
        Previous => "skip-back",
        Next => "skip-forward",
        Shuffle => "shuffle",
        Repeat => "repeat",
        RepeatOne => "repeat-1",
        Home => "house",
        Explore => "compass",
        Library => "library",
        Like => "thumbs-up",
        Liked => "thumbs-up-fill",
        Dislike => "thumbs-down",
        Disliked => "thumbs-down-fill",
        Queue => "list-music",
        Lyrics => "mic-vocal",
        More => "ellipsis-vertical",
        Music => "music",
        AddToQueue => "list-plus",
        PlayNext => "list-start",
        Radio => "radio",
        Save => "bookmark",
        Saved => "bookmark-fill",
        Heart => "heart",
        History => "history",
        SignIn => "log-in",
        VolumeLow => "volume-1",
        Forward => "arrow-right",
        Album => "disc",
        Podcast => "mic",
        Account => "circle-user",
        ClearQueue => "list-x",
        Grip => "grip",
        Chart => "chart",
        Mix => "sparkles",
        Visualiser => "audio-lines",
        Back => lucide "arrow-left",
        Search => lucide "search",
        Settings => lucide "settings",
        Close => lucide "x",
        Plus => lucide "plus",
        Check => lucide "check",
        Volume => lucide "volume-2",
        Mute => lucide "volume-x",
        ChevronLeft => lucide "chevron-left",
        ChevronRight => lucide "chevron-right",
        ChevronDown => lucide "chevron-down",
        Trash => lucide "trash-2",
        Link => lucide "copy",
        External => lucide "external-link",
        SignOut => lucide "log-out",
        User => lucide "user",
        Alert => lucide "circle-alert",
        Refresh => lucide "refresh-cw",
        Sidebar => lucide "panel-left",
        Expand => lucide "maximize-2",
        Collapse => lucide "minimize-2",
        Download => "download",
        Offline => "wifi-off",
        LastFm => "lastfm",
        CircleCheck => lucide "circle-check",
        ChevronUp => lucide "chevron-up",
        Info => lucide "info",
        Fullscreen => "maximize",
        ExitFullscreen => "minimize",
        Folder => "folder",
        FolderPlus => "folder-plus",
        Computer => lucide "monitor",
        Update => "circle-arrow-down",
        Clock => lucide "clock",
        Pencil => lucide "pencil",
        Minus => lucide "minus",
    }
}

/// The app's logo: a music note with a window in its flag, white on blue,
/// in a rounded square. Blue for the executable (assets/app.ico) and the
/// README; the window, the tray and the sidebar show it in the look's
/// colour.
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// The hue of the logo's blue, in OKLCh's degrees, as its colours average
/// out.
const LOGO_HUE: f32 = 251.6;

/// The logo as drawn, straight RGBA, `size` pixels square.
pub fn logo_rgba(size: usize) -> Vec<u8> {
    let size = size.max(1);
    match image::load_from_memory(ICON_PNG) {
        Ok(icon) => icon
            .resize_exact(size as u32, size as u32, image::imageops::FilterType::Lanczos3)
            .to_rgba8()
            .into_raw(),
        Err(_) => vec![0; size * size * 4],
    }
}

/// The logo in the current look's colour, straight RGBA, `size` pixels
/// square: for the window, the tray and the sidebar.
pub fn app_icon_rgba(size: usize) -> Vec<u8> {
    let mut rgba = logo_rgba(size);
    tint(&mut rgba, accent());
    rgba
}

/// Turns `rgba` from the logo's blue to `accent`'s hue, each pixel as light
/// and as colourful as it was (in OKLab, where those hold as a hue turns),
/// with less colour where the turn would leave what a screen can show.
fn tint(rgba: &mut [u8], accent: Color32) {
    use egui::ecolor::{gamma_u8_from_linear_f32, linear_f32_from_gamma_u8};
    let [_, a, b] = oklab([accent.r(), accent.g(), accent.b()].map(linear_f32_from_gamma_u8));
    let (sin, cos) = (b.atan2(a) - LOGO_HUE.to_radians()).sin_cos();
    for pixel in rgba.chunks_exact_mut(4).filter(|pixel| pixel[3] > 0) {
        let [l, a, b] = oklab([pixel[0], pixel[1], pixel[2]].map(linear_f32_from_gamma_u8));
        let (a, b) = (a * cos - b * sin, a * sin + b * cos);
        let mut colour = 1.0;
        let mut rgb = linear_from_oklab([l, a, b]);
        while colour > 0.3 && rgb.iter().any(|v| !(-1e-4..=1.0001).contains(v)) {
            colour *= 0.95;
            rgb = linear_from_oklab([l, a * colour, b * colour]);
        }
        for (out, v) in pixel.iter_mut().zip(rgb) {
            *out = gamma_u8_from_linear_f32(v);
        }
    }
}

/// Linear sRGB to OKLab (Björn Ottosson's).
fn oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OKLab to linear sRGB, past 0 and 1 where it leaves what sRGB holds.
fn linear_from_oklab([l, a, b]: [f32; 3]) -> [f32; 3] {
    let lms = [
        l + 0.396_337_78 * a + 0.215_803_76 * b,
        l - 0.105_561_346 * a - 0.063_854_17 * b,
        l - 0.089_484_18 * a - 1.291_485_5 * b,
    ]
    .map(|v| v * v * v);
    let [l, m, s] = lms;
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

pub fn app_icon(size: usize) -> egui::IconData {
    egui::IconData {
        rgba: app_icon_rgba(size),
        width: size as u32,
        height: size as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_takes_each_looks_colour() {
        // The logo's blue, its white, and a clear pixel.
        let logo = [62u8, 169, 252, 255, 236, 244, 255, 200, 9, 9, 9, 0];
        let tinted = |theme: Theme| {
            let mut rgba = logo;
            tint(&mut rgba, theme.palette().accent);
            rgba
        };
        let [r, g, b, a] = tinted(Theme::Classic)[..4].try_into().unwrap();
        assert!(r > 200 && g < 120 && b < 140 && a == 255, "red: {r} {g} {b}");
        let [r, g, b, _] = tinted(Theme::Midnight)[..4].try_into().unwrap();
        assert!(b > 240 && b > g && g > r, "still blue: {r} {g} {b}");
        let [r, g, b, _] = tinted(Theme::Sakura)[..4].try_into().unwrap();
        assert!(r > g + 60 && b > g, "pink: {r} {g} {b}");
        let [r, g, b, _] = tinted(Theme::Matcha)[..4].try_into().unwrap();
        assert!(g > r + 40 && g > b, "green: {r} {g} {b}");
        for theme in Theme::ALL {
            let rgba = tinted(theme);
            // The white stays near white, as see-through as it was; the
            // clear pixel is left alone.
            assert!(rgba[4..7].iter().all(|&v| v > 200) && rgba[7] == 200, "{theme:?}: {:?}", &rgba[4..8]);
            assert_eq!(rgba[8..], logo[8..]);
        }
    }

    #[test]
    fn the_logo_is_drawn_at_any_size() {
        for size in [16, 30, 38, 256] {
            let rgba = app_icon_rgba(size);
            assert_eq!(rgba.len(), size * size * 4);
            // Clear at the very corner, solid in the middle.
            assert_eq!(rgba[3], 0);
            assert!(rgba[(size / 2 * size + size / 2) * 4 + 3] >= 250);
        }
    }
}
