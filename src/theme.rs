//! Colours, sizes, fonts and icons. YouTube Music is dark only, and so is
//! this: near-black surfaces, white text, and red for what is playing.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

pub const BG: Color32 = Color32::from_rgb(0x0b, 0x0b, 0x0b);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x03, 0x03, 0x03);
pub const PLAYER: Color32 = Color32::from_rgb(0x17, 0x17, 0x17);
pub const SURFACE: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x1d);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x29, 0x29, 0x29);
pub const SURFACE_ACTIVE: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
pub const POPUP: Color32 = Color32::from_rgb(0x21, 0x21, 0x21);
pub const TEXT: Color32 = Color32::from_rgb(0xf1, 0xf1, 0xf1);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xaa, 0xaa, 0xaa);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x71, 0x71, 0x71);
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x00, 0x33);
/// The accent as text on dark surfaces: a little lighter, to read.
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0xff, 0x4d, 0x5e);
pub const DIVIDER: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
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

    let mut visuals = Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = POPUP;
    visuals.extreme_bg_color = SURFACE;
    visuals.faint_bg_color = SURFACE;
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = TEXT;
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, TEXT);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_stroke = Stroke::new(1.0, DIVIDER);
    visuals.window_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    visuals.popup_shadow = visuals.window_shadow;
    let w = &mut visuals.widgets;
    w.noninteractive.bg_fill = BG;
    w.noninteractive.bg_stroke = Stroke::new(1.0, DIVIDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    for (state, fill) in [
        (&mut w.inactive, SURFACE),
        (&mut w.hovered, SURFACE_HOVER),
        (&mut w.active, SURFACE_ACTIVE),
        (&mut w.open, SURFACE_HOVER),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke = Stroke::new(1.5, TEXT);
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
    }
}

/// The app's icon: a player window with a red disc, a play mark and a
/// note on it. One picture for the window, the tray, the sidebar, the
/// executable (assets/app.ico) and the README.
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// The icon as straight RGBA, `size` pixels square.
pub fn app_icon_rgba(size: usize) -> Vec<u8> {
    let size = size.max(1);
    match image::load_from_memory(ICON_PNG) {
        Ok(icon) => icon
            .resize_exact(size as u32, size as u32, image::imageops::FilterType::Lanczos3)
            .to_rgba8()
            .into_raw(),
        Err(_) => vec![0; size * size * 4],
    }
}

pub fn app_icon(size: usize) -> egui::IconData {
    egui::IconData {
        rgba: app_icon_rgba(size),
        width: size as u32,
        height: size as u32,
    }
}
