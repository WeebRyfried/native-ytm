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

pub const PLAYER_HEIGHT: f32 = 76.0;
pub const SIDEBAR_WIDTH: f32 = 232.0;
pub const TOPBAR_HEIGHT: f32 = 60.0;
pub const SIDE_PANEL_WIDTH: f32 = 360.0;
pub const ROW_HEIGHT: f32 = 56.0;
pub const PAGE_MARGIN: f32 = 32.0;
pub const CARD: f32 = 176.0;

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
    }
}

/// The app's icon, drawn rather than shipped: a red disc with a white ring
/// and play triangle, at any size, supersampled for clean edges.
pub fn app_icon_rgba(size: usize) -> Vec<u8> {
    const SUB: usize = 4;
    let mut out = vec![0u8; size * size * 4];
    let n = size as f32;
    let c = n / 2.0;
    let r = n / 2.0;
    let ring_r = r * 0.64;
    let ring_w = (n * 0.06).max(1.2);
    let s = r * 0.40;
    let tri = [(-0.40 * s, -0.62 * s), (-0.40 * s, 0.62 * s), (0.66 * s, 0.0)];
    let edge = |p: (f32, f32), a: (f32, f32), b: (f32, f32)| {
        (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
    };
    for y in 0..size {
        for x in 0..size {
            let (mut disc, mut white) = (0.0f32, 0.0f32);
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let px = x as f32 + (sx as f32 + 0.5) / SUB as f32 - c;
                    let py = y as f32 + (sy as f32 + 0.5) / SUB as f32 - c;
                    let d = (px * px + py * py).sqrt();
                    if d > r {
                        continue;
                    }
                    disc += 1.0;
                    let on_ring = (d - ring_r).abs() < ring_w / 2.0;
                    let p = (px + s * 0.08, py);
                    let e0 = edge(p, tri[0], tri[1]);
                    let e1 = edge(p, tri[1], tri[2]);
                    let e2 = edge(p, tri[2], tri[0]);
                    let in_tri = (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0) || (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0);
                    if on_ring || in_tri {
                        white += 1.0;
                    }
                }
            }
            if disc == 0.0 {
                continue;
            }
            let w = white / disc;
            let mix = |v: f32| (v * (1.0 - w) + 255.0 * w).round() as u8;
            let i = (y * size + x) * 4;
            out[i] = mix(255.0);
            out[i + 1] = mix(0.0);
            out[i + 2] = mix(51.0);
            out[i + 3] = (disc / (SUB * SUB) as f32 * 255.0).round() as u8;
        }
    }
    out
}

pub fn app_icon(size: usize) -> egui::IconData {
    egui::IconData {
        rgba: app_icon_rgba(size),
        width: size as u32,
        height: size as u32,
    }
}
