//! The pieces every page is made of: artwork, buttons, song rows, cards,
//! and sections of either.

use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, Id, Layout as EguiLayout, Pos2, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

use std::collections::HashMap;

use crate::api::model::*;
use crate::api::music::Rating;
use crate::cache::Mark;
use crate::images::Images;
use crate::model::Action;
use crate::queue::Source;
use crate::theme::{self, Icon};

/// What views share: artwork, the action list, and what is playing.
pub struct Cx<'a> {
    pub images: &'a mut Images,
    pub actions: &'a mut Vec<Action>,
    pub current: Option<&'a str>,
    pub playing: bool,
    pub signed_in: bool,
    /// Songs on this PC and downloading, by video id.
    pub offline: &'a HashMap<String, Mark>,
    pub online: bool,
    /// Show titles in Latin letters when there are some.
    pub romanized: bool,
}

impl Cx<'_> {
    pub fn push(&mut self, action: Action) {
        self.actions.push(action);
    }

    /// A song's title as the settings would have it shown.
    pub fn title<'t>(&self, title: &'t str) -> &'t str {
        if self.romanized { romanized_title(title) } else { title }
    }
}

// ---- Artwork ----------------------------------------------------------------

/// Artwork filling `rect`, cropped to its shape, or a quiet placeholder
/// while it loads.
pub fn paint_art(ui: &Ui, images: &mut Images, url: Option<&str>, rect: Rect, radius: f32, round: bool) {
    paint_art_faded(ui, images, url, rect, radius, round, 1.0);
}

/// The same, at `opacity`, for crossfades.
pub fn paint_art_faded(ui: &Ui, images: &mut Images, url: Option<&str>, rect: Rect, radius: f32, round: bool, opacity: f32) {
    let painter = ui.painter_at(rect.expand(1.0));
    let tint = Color32::WHITE.gamma_multiply(opacity);
    let corner = if round {
        CornerRadius::same((rect.width() / 2.0).min(255.0) as u8)
    } else {
        CornerRadius::same(radius.min(255.0) as u8)
    };
    let px = (rect.width().max(rect.height()) * ui.ctx().pixels_per_point()).round() as u32;
    let texture = url.and_then(|url| images.get_or_smaller(url, px));
    match texture {
        Some(texture) => {
            let size = texture.size_vec2();
            let uv = crop_uv(size, rect.size());
            let mut mesh = egui::epaint::RectShape::filled(rect, corner, tint);
            mesh.brush = Some(std::sync::Arc::new(egui::epaint::Brush {
                fill_texture_id: texture.id(),
                uv,
            }));
            painter.add(mesh);
        }
        None => {
            painter.rect_filled(rect, corner, theme::SURFACE.gamma_multiply(opacity));
            let icon = (rect.width() * 0.36).clamp(14.0, 64.0);
            let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(icon));
            Icon::Music
                .image(theme::TEXT_FAINT.gamma_multiply(opacity), icon)
                .paint_at(ui, icon_rect);
        }
    }
}

/// The part of an image of `size` that fills a box of `target` shape.
fn crop_uv(size: Vec2, target: Vec2) -> Rect {
    let image_aspect = size.x / size.y.max(1.0);
    let target_aspect = target.x / target.y.max(1.0);
    if image_aspect > target_aspect {
        let w = target_aspect / image_aspect;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = image_aspect / target_aspect;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    }
}

// ---- Buttons ----------------------------------------------------------------

/// An icon on a round hover highlight.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, tint: Color32, tip: &str) -> Response {
    icon_button_scaled(ui, icon, size, tint, tip, 1.0)
}

/// The same, its icon drawn at `scale` (for a pop) in the same room.
pub fn icon_button_scaled(ui: &mut Ui, icon: Icon, size: f32, tint: Color32, tip: &str, scale: f32) -> Response {
    let pad = (size * 0.45).max(6.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size + pad * 2.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        // The hover circle fades in and out rather than blinking.
        let hover = ui.ctx().animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);
        if hover > 0.0 {
            ui.painter()
                .circle_filled(rect.center(), rect.width() / 2.0, Color32::from_white_alpha((22.0 * hover) as u8));
        }
        // Pressed: dimmer and a touch smaller.
        let pressed = response.is_pointer_button_down_on();
        let tint = if pressed { tint.gamma_multiply(0.8) } else { tint };
        let drawn = size * scale * if pressed { 0.92 } else { 1.0 };
        icon.image(tint, drawn)
            .paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(drawn)));
    }
    if tip.is_empty() {
        response
    } else {
        response.on_hover_text(tip)
    }
}

/// A rounded pill: white for the main action, outlined for the others.
pub fn pill(ui: &mut Ui, icon: Option<Icon>, text: &str, primary: bool) -> Response {
    let font = theme::medium(14.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::WHITE);
    let icon_w = if icon.is_some() { 20.0 + 8.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 36.0, 36.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let (fill, fg, stroke) = if primary {
            (
                if hovered { Color32::from_gray(230) } else { Color32::WHITE },
                Color32::BLACK,
                Stroke::NONE,
            )
        } else {
            (
                if hovered { Color32::from_white_alpha(26) } else { Color32::from_white_alpha(10) },
                theme::TEXT,
                Stroke::new(1.0, Color32::from_white_alpha(40)),
            )
        };
        ui.painter()
            .rect(rect, CornerRadius::same(18), fill, stroke, StrokeKind::Inside);
        let mut x = rect.left() + 18.0;
        if let Some(icon) = icon {
            icon.image(fg, 20.0)
                .paint_at(ui, Rect::from_min_size(pos2(x, rect.center().y - 10.0), Vec2::splat(20.0)));
            x += 28.0;
        }
        ui.painter().galley_with_override_text_color(
            pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            fg,
        );
    }
    response
}

/// A chip, as for search filters and moods.
pub fn chip(ui: &mut Ui, text: &str, selected: bool) -> Response {
    let font = theme::regular(13.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let size = vec2(galley.size().x + 24.0, 32.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let (fill, fg) = if selected {
            (Color32::WHITE, Color32::BLACK)
        } else if response.hovered() {
            (Color32::from_white_alpha(38), theme::TEXT)
        } else {
            (Color32::from_white_alpha(20), theme::TEXT)
        };
        ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
        ui.painter().galley_with_override_text_color(
            rect.center() - galley.size() / 2.0,
            galley,
            fg,
        );
    }
    response
}

pub fn spinner(ui: &mut Ui) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        spinner_of(ui, 28.0, theme::TEXT_DIM);
    });
}

/// A turning arc, as egui's spinner draws it, but at 60 frames a second
/// rather than the screen's full rate.
pub fn spinner_of(ui: &mut Ui, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let t = ui.input(|i| i.time) as f32;
    let start = t * 5.0;
    let radius = rect.width() / 2.0 - 2.0;
    let points: Vec<Pos2> = (0..=24)
        .map(|i| {
            let a = start + i as f32 / 24.0 * std::f32::consts::PI * 1.4;
            rect.center() + vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    ui.painter().add(egui::Shape::line(points, Stroke::new(2.5, color)));
    theme::next_frame(ui.ctx());
}

pub fn failed(ui: &mut Ui, message: &str, retry: Option<Action>, cx: &mut Cx<'_>) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        Icon::Alert.image(theme::TEXT_DIM, 32.0).paint_at(
            ui,
            Rect::from_center_size(ui.cursor().center_top() + vec2(0.0, 16.0), Vec2::splat(32.0)),
        );
        ui.add_space(40.0);
        ui.label(RichText::new(message).color(theme::TEXT_DIM));
        ui.add_space(12.0);
        if let Some(retry) = retry
            && pill(ui, Some(Icon::Refresh), "Try again", false).clicked()
        {
            cx.push(retry);
        }
    });
}

/// Text cut to `width` with an ellipsis, returning whether it was cut.
pub fn clipped_label(ui: &mut Ui, text: &str, font: egui::FontId, color: Color32, width: f32, sense: Sense) -> Response {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let (rect, response) = ui.allocate_exact_size(galley.size(), sense);
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.min, galley.clone(), color);
    }
    if galley.elided {
        response.on_hover_text(text)
    } else {
        response
    }
}

/// Names that lead somewhere, separated by commas: artists in a row.
pub fn links(ui: &mut Ui, cx: &mut Cx<'_>, items: &[Link], font: egui::FontId, color: Color32, width: f32) {
    if items.is_empty() {
        return;
    }
    let mut job = egui::text::LayoutJob::default();
    let mut ranges = Vec::new();
    for (i, link) in items.iter().enumerate() {
        if i > 0 {
            job.append(", ", 0.0, egui::TextFormat::simple(font.clone(), color));
        }
        // Ranges in characters, as the galley's cursors count.
        let start = job.text.chars().count();
        job.append(&link.name, 0.0, egui::TextFormat::simple(font.clone(), color));
        ranges.push((start..job.text.chars().count(), link));
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let pointer = response.hover_pos();
    let mut hovered_link = None;
    if let Some(pointer) = pointer {
        let cursor = galley.cursor_from_pos(pointer - rect.min);
        let index = cursor.index.0;
        hovered_link = ranges
            .iter()
            .find(|(range, link)| link.id.is_some() && range.contains(&index))
            .map(|(range, link)| (range.clone(), (*link).clone()));
    }
    ui.painter().galley(rect.min, galley.clone(), color);
    if let Some((range, link)) = hovered_link {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        // Underline the hovered name.
        let start = galley.pos_from_cursor(egui::text::CCursor::new(range.start));
        let end = galley.pos_from_cursor(egui::text::CCursor::new(range.end));
        let y = rect.min.y + start.max.y - 1.0;
        ui.painter().line_segment(
            [pos2(rect.min.x + start.min.x, y), pos2(rect.min.x + end.min.x, y)],
            Stroke::new(1.0, color),
        );
        if response.clicked()
            && let Some(id) = link.id
        {
            cx.push(Action::Open(Target::Browse {
                browse_id: id,
                params: None,
            }));
        }
    }
    if galley.elided {
        response.on_hover_text(items.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", "));
    }
}

// ---- Song rows --------------------------------------------------------------

/// Where a row is, which changes its menu.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowPlace<'a> {
    /// A list on a page: double-click plays the list from here.
    List,
    /// An album's list: numbers instead of artwork.
    Album,
    /// One of the account's own playlists: songs can be removed.
    OwnPlaylist(&'a str),
    /// The queue: removing takes it out of the queue.
    Queue(u64),
}

pub struct RowResponse {
    pub play: bool,
    pub response: Response,
}

/// A song: artwork (or its number), title, artists, album, length.
pub fn track_row(
    ui: &mut Ui,
    cx: &mut Cx<'_>,
    track: &Track,
    number: Option<usize>,
    place: RowPlace<'_>,
    show_album: bool,
) -> RowResponse {
    let height = if matches!(place, RowPlace::Queue(_)) { 52.0 } else { theme::ROW_HEIGHT };
    let width = ui.available_width();
    // Queue rows also drag, to reorder.
    let sense = if matches!(place, RowPlace::Queue(_)) { Sense::click_and_drag() } else { Sense::click() };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    let mut play = false;
    if !ui.is_rect_visible(rect) {
        return RowResponse { play, response };
    }
    let current = cx.current == Some(track.video_id.as_str());
    // Where the pointer is, not which widget has it: the artwork and the
    // menu button sit over the row and would take its hover away.
    let hovered = response.contains_pointer() || response.context_menu_opened();
    let shade = ui.ctx().animate_bool_with_time(response.id.with("hover"), hovered, 0.1);
    if shade > 0.0 {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), Color32::from_white_alpha((14.0 * shade) as u8));
    } else if current && matches!(place, RowPlace::Queue(_)) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), Color32::from_white_alpha(10));
    }
    let art = height - 16.0;
    let mut x = rect.left() + 8.0;
    let lead = Rect::from_min_size(pos2(x, rect.center().y - art / 2.0), Vec2::splat(art));
    if place == RowPlace::Album {
        let label = number.map(|n| n.to_string()).unwrap_or_default();
        if hovered {
            Icon::Play
                .image(theme::TEXT, 18.0)
                .paint_at(ui, Rect::from_center_size(lead.center(), Vec2::splat(18.0)));
        } else if current {
            equalizer(ui, lead.center(), cx.playing);
        } else {
            ui.painter().text(
                lead.center(),
                Align2::CENTER_CENTER,
                label,
                theme::regular(14.0),
                theme::TEXT_DIM,
            );
        }
    } else {
        paint_art(ui, cx.images, track.thumbnail.as_deref(), lead, 4.0, false);
        if hovered || current {
            ui.painter()
                .rect_filled(lead, CornerRadius::same(4), Color32::from_black_alpha(140));
            if hovered && !current {
                Icon::Play
                    .image(Color32::WHITE, 18.0)
                    .paint_at(ui, Rect::from_center_size(lead.center(), Vec2::splat(18.0)));
            } else {
                equalizer(ui, lead.center(), cx.playing);
            }
        }
    }
    let lead_response = ui.interact(lead, response.id.with("lead"), Sense::click());
    if lead_response.clicked() {
        play = true;
    }
    x = lead.right() + 14.0;

    let duration_w = 56.0;
    let menu_w = 36.0;
    let right = rect.right() - duration_w - menu_w;
    let album_w = if show_album && track.album.is_some() && width > 640.0 {
        ((right - x) * 0.32).min(280.0)
    } else {
        0.0
    };
    let text_w = (right - x - album_w - 16.0).max(40.0);
    let title_color = if current { theme::ACCENT_TEXT } else { theme::TEXT };
    // The title and the artists as one block, centred in the row. (In a
    // horizontal layout the title line took a button's height, which
    // pushed the artists down against the row's bottom edge.)
    let title_font = theme::medium(14.5);
    let line_font = theme::regular(13.0);
    let (title_h, line_h) = ui.fonts_mut(|f| (f.row_height(&title_font), f.row_height(&line_font)));
    let gap = 3.0;
    let top = (rect.center().y - (title_h + gap + line_h) / 2.0).round();
    let mut title = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_size(pos2(x, top), vec2(text_w, title_h)))
            .layout(EguiLayout::left_to_right(Align::Center)),
    );
    title.spacing_mut().item_spacing.x = 6.0;
    if track.explicit {
        explicit_badge(&mut title);
    }
    let title_w = title.available_width();
    let shown = cx.title(&track.title);
    let label = clipped_label(&mut title, shown, title_font, title_color, title_w, Sense::hover());
    if shown != track.title {
        label.on_hover_text(&track.title);
    }
    let mut line = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_size(pos2(x, top + title_h + gap), vec2(text_w, line_h)))
            .layout(EguiLayout::left_to_right(Align::Center)),
    );
    line.spacing_mut().item_spacing.x = 5.0;
    if let Some(mark) = cx.offline.get(&track.video_id) {
        offline_mark(&mut line, *mark);
    }
    let line_w = line.available_width();
    if track.artists.is_empty() {
        clipped_label(&mut line, track.extra.as_deref().unwrap_or(""), line_font, theme::TEXT_DIM, line_w, Sense::hover());
    } else {
        links(&mut line, cx, &track.artists, line_font, theme::TEXT_DIM, line_w);
    }
    if album_w > 0.0
        && let Some(album) = &track.album
    {
        let album_rect = Rect::from_min_size(
            pos2(right - album_w, rect.center().y - 9.0),
            vec2(album_w, 20.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(album_rect));
        links(&mut child, cx, std::slice::from_ref(album), theme::regular(13.5), theme::TEXT_DIM, album_w);
    }
    if let Some(d) = track.duration {
        ui.painter().text(
            pos2(rect.right() - menu_w - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            format_duration(d),
            theme::regular(13.5),
            theme::TEXT_DIM,
        );
    }
    let menu_rect = Rect::from_center_size(pos2(rect.right() - menu_w / 2.0, rect.center().y), Vec2::splat(32.0));
    let menu_response = ui.interact(menu_rect, response.id.with("menu"), Sense::click());
    if hovered || menu_response.hovered() {
        if menu_response.hovered() {
            ui.painter()
                .circle_filled(menu_rect.center(), 16.0, Color32::from_white_alpha(24));
        }
        Icon::More
            .image(theme::TEXT, 20.0)
            .paint_at(ui, Rect::from_center_size(menu_rect.center(), Vec2::splat(20.0)));
    }
    egui::Popup::menu(&menu_response).show(|ui| track_menu(ui, cx, track, place));
    response.context_menu(|ui| track_menu(ui, cx, track, place));
    if response.double_clicked() {
        play = true;
    }
    RowResponse { play, response }
}

/// "1 song", "12 songs".
pub fn songs(n: usize) -> String {
    if n == 1 { "1 song".to_owned() } else { format!("{n} songs") }
}

/// The mark of a song that plays without the network: a filled circle with
/// a tick once downloaded, an outline while only kept from a recent play,
/// and a ring filling up while it downloads.
pub fn offline_mark(ui: &mut Ui, mark: Mark) {
    let size = 13.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let c = rect.center();
    let r = size / 2.0 - 0.5;
    let painter = ui.painter();
    let tick = |color: Color32| {
        painter.add(egui::Shape::line(
            vec![c + vec2(-3.0, 0.1), c + vec2(-0.9, 2.2), c + vec2(3.0, -2.0)],
            Stroke::new(1.5, color),
        ));
    };
    let tip = match mark {
        Mark::Saved => {
            painter.circle_filled(c, r, theme::TEXT_DIM);
            tick(theme::BG);
            "Downloaded: plays offline"
        }
        Mark::Kept => {
            painter.circle_stroke(c, r - 0.6, Stroke::new(1.2, theme::TEXT_FAINT));
            tick(theme::TEXT_FAINT);
            "On this PC from a recent play: plays offline for now"
        }
        Mark::Saving(done) => {
            painter.circle_stroke(c, r - 0.8, Stroke::new(1.6, Color32::from_white_alpha(36)));
            if done > 0.0 {
                let end = done.clamp(0.0, 1.0) * std::f32::consts::TAU;
                let points: Vec<Pos2> = (0..=24)
                    .map(|i| {
                        let a = -std::f32::consts::FRAC_PI_2 + end * i as f32 / 24.0;
                        c + vec2(a.cos(), a.sin()) * (r - 0.8)
                    })
                    .collect();
                painter.add(egui::Shape::line(points, Stroke::new(1.6, theme::TEXT_DIM)));
            }
            "Downloading"
        }
        Mark::Waiting => {
            painter.circle_stroke(c, r - 0.8, Stroke::new(1.2, Color32::from_white_alpha(36)));
            "Waiting to download"
        }
    };
    response.on_hover_text(tip);
}

fn explicit_badge(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(15.0, 15.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(2), Color32::from_gray(170));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "E",
        theme::bold(10.0),
        Color32::BLACK,
    );
}

/// The three bars of what is playing, still while paused.
fn equalizer(ui: &Ui, center: Pos2, playing: bool) {
    let t = ui.input(|i| i.time) as f32;
    if playing {
        theme::repaint_in(ui.ctx(), std::time::Duration::from_millis(100));
    }
    for (i, phase) in [0.0f32, 1.7, 3.1].iter().enumerate() {
        let h = if playing {
            5.0 + 9.0 * (0.5 + 0.5 * (t * 7.0 + phase).sin())
        } else {
            [8.0, 13.0, 6.0][i]
        };
        let x = center.x - 7.0 + i as f32 * 6.0;
        ui.painter().rect_filled(
            Rect::from_min_max(pos2(x, center.y + 7.0 - h), pos2(x + 4.0, center.y + 7.0)),
            CornerRadius::same(1),
            theme::ACCENT,
        );
    }
}

pub fn track_menu(ui: &mut Ui, cx: &mut Cx<'_>, track: &Track, place: RowPlace<'_>) {
    ui.set_min_width(220.0);
    let item = |ui: &mut Ui, icon: Icon, text: &str| -> bool {
        ui.add(egui::Button::image_and_text(icon.image(theme::TEXT, 18.0), text).frame_when_inactive(false))
            .clicked()
    };
    // A song on this PC: the queue, and its file. YouTube Music's radio,
    // likes, playlists and links are not for it.
    if let Some(file) = &track.file {
        if !matches!(place, RowPlace::Queue(_)) {
            if item(ui, Icon::PlayNext, "Play next") {
                cx.push(Action::PlayNext(vec![track.clone()]));
            }
            if item(ui, Icon::AddToQueue, "Add to queue") {
                cx.push(Action::AddToQueue(vec![track.clone()]));
            }
        }
        if let RowPlace::Queue(uid) = place
            && item(ui, Icon::Close, "Remove from queue")
        {
            cx.push(Action::RemoveFromQueue(uid));
        }
        ui.separator();
        if item(ui, Icon::Folder, "Show in folder") {
            cx.push(Action::ShowInFolder(file.clone()));
        }
        return;
    }
    if item(ui, Icon::Radio, "Start radio") {
        cx.push(Action::StartRadio(track.clone()));
    }
    if !matches!(place, RowPlace::Queue(_)) {
        if item(ui, Icon::PlayNext, "Play next") {
            cx.push(Action::PlayNext(vec![track.clone()]));
        }
        if item(ui, Icon::AddToQueue, "Add to queue") {
            cx.push(Action::AddToQueue(vec![track.clone()]));
        }
    }
    ui.separator();
    match cx.offline.get(&track.video_id) {
        Some(Mark::Saved | Mark::Saving(_) | Mark::Waiting) => {
            if item(ui, Icon::CircleCheck, "Remove download") {
                cx.push(Action::RemoveOffline(vec![track.video_id.clone()]));
            }
        }
        _ => {
            if item(ui, Icon::Download, "Download") {
                cx.push(Action::SaveOffline(vec![track.clone()]));
            }
        }
    }
    if item(ui, Icon::Like, "Like") {
        cx.push(Action::Rate(track.video_id.clone(), Rating::Like));
    }
    if item(ui, Icon::Plus, "Save to playlist") {
        cx.push(Action::PickPlaylist(vec![track.clone()]));
    }
    if let RowPlace::OwnPlaylist(playlist_id) = place
        && track.set_video_id.is_some()
        && item(ui, Icon::Trash, "Remove from playlist")
    {
        cx.push(Action::RemoveFromPlaylist {
            playlist_id: playlist_id.to_owned(),
            track: track.clone(),
        });
    }
    if let RowPlace::Queue(uid) = place
        && item(ui, Icon::Close, "Remove from queue")
    {
        cx.push(Action::RemoveFromQueue(uid));
    }
    ui.separator();
    if let Some(album) = &track.album
        && let Some(id) = &album.id
        && item(ui, Icon::Album, "Go to album")
    {
        cx.push(Action::Open(Target::Browse {
            browse_id: id.clone(),
            params: None,
        }));
    }
    if let Some(artist) = track.artists.iter().find(|a| a.id.is_some())
        && item(ui, Icon::User, "Go to artist")
    {
        cx.push(Action::Open(Target::Browse {
            browse_id: artist.id.clone().unwrap_or_default(),
            params: None,
        }));
    }
    if item(ui, Icon::Link, "Copy link") {
        cx.push(Action::CopyLink(format!(
            "https://music.youtube.com/watch?v={}",
            track.video_id
        )));
    }
}

/// A list of songs that plays as a queue from whichever is chosen.
pub fn track_list(
    ui: &mut Ui,
    cx: &mut Cx<'_>,
    tracks: &[Track],
    place: RowPlace<'_>,
    source: &Source,
    show_album: bool,
) {
    let clip = ui.clip_rect();
    for (i, track) in tracks.iter().enumerate() {
        // Rows outside the view only take their space.
        let top = ui.cursor().top();
        if top > clip.bottom() + 200.0 || top + theme::ROW_HEIGHT < clip.top() - 200.0 {
            ui.allocate_space(vec2(ui.available_width(), theme::ROW_HEIGHT));
            continue;
        }
        let row = track_row(ui, cx, track, Some(i + 1), place, show_album);
        if row.play {
            cx.push(Action::PlayTracks {
                tracks: tracks.to_vec(),
                start: i,
                source: source.clone(),
            });
        }
    }
}

// ---- Cards ------------------------------------------------------------------

/// What happens when an item is chosen.
pub fn open_item(cx: &mut Cx<'_>, item: &Item) {
    if let Some(track) = &item.track {
        match &item.target {
            Some(Target::Watch {
                playlist_id: Some(playlist_id),
                video_id,
                params,
            }) if !playlist_id.starts_with("RDAMVM") => cx.push(Action::PlayWatch {
                video_id: Some(video_id.clone()),
                playlist_id: Some(playlist_id.clone()),
                params: params.clone(),
                title: None,
            }),
            _ => cx.push(Action::PlayTrack(track.clone())),
        }
        return;
    }
    if let Some(target) = &item.target {
        cx.push(Action::Open(target.clone()));
    }
}

fn play_item(cx: &mut Cx<'_>, item: &Item) {
    if item.track.is_some() {
        open_item(cx, item);
    } else if let Some(playlist_id) = &item.play_playlist {
        cx.push(Action::PlayWatch {
            video_id: None,
            playlist_id: Some(playlist_id.clone()),
            params: None,
            title: Some(item.title.clone()),
        });
    } else if let Some(target) = &item.target {
        cx.push(Action::Open(target.clone()));
    }
}

/// A card: artwork with a play button on hover, a title and a subtitle.
pub fn card(ui: &mut Ui, cx: &mut Cx<'_>, item: &Item, width: f32) -> Response {
    let wide = item.kind == Kind::Video
        || item
            .thumbnail
            .as_deref()
            .is_some_and(|u| u.contains("i.ytimg.com") && item.kind != Kind::Song);
    let art_h = if wide { width * 9.0 / 16.0 } else { width };
    let height = art_h + 58.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let art = Rect::from_min_size(rect.min, vec2(width, art_h));
    let round = item.kind.round();
    paint_art(ui, cx.images, item.thumbnail.as_deref(), art, 6.0, round);
    // The play button over the art takes the hover when the pointer is on
    // it; the card stays hovered all the same, or the button would vanish
    // and come back every frame.
    let hovered = response.contains_pointer() || response.context_menu_opened();
    let playable = item.track.is_some() || item.play_playlist.is_some();
    let mut play_clicked = false;
    // The dim and the play button fade in, the button rising a little.
    let shown = ui.ctx().animate_bool_with_time(response.id.with("hover"), hovered, 0.14);
    if shown > 0.0 && !round {
        ui.painter().rect_filled(
            art,
            CornerRadius::same(6),
            Color32::from_black_alpha((90.0 * shown) as u8),
        );
    }
    if shown > 0.0 && playable {
        let c = art.right_bottom() - vec2(28.0, 28.0 - 6.0 * (1.0 - shown));
        let button = Rect::from_center_size(c, Vec2::splat(40.0));
        let play = ui.interact(button, response.id.with("play"), Sense::click());
        let fill = if play.hovered() { Color32::from_black_alpha(230) } else { Color32::from_black_alpha(170) };
        ui.painter().circle_filled(c, 20.0 * (0.85 + 0.15 * shown), fill.gamma_multiply(shown));
        Icon::Play
            .image(Color32::WHITE.gamma_multiply(shown), 18.0)
            .paint_at(ui, Rect::from_center_size(c + vec2(1.0, 0.0), Vec2::splat(18.0)));
        play_clicked = play.clicked() && hovered;
    }
    let text_rect = Rect::from_min_max(pos2(rect.left(), art.bottom() + 8.0), rect.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(if round {
        EguiLayout::top_down(Align::Center)
    } else {
        EguiLayout::top_down(Align::Min)
    }));
    child.spacing_mut().item_spacing.y = 2.0;
    let title = if item.track.is_some() { cx.title(&item.title) } else { &item.title };
    clipped_label(&mut child, title, theme::medium(14.5), theme::TEXT, width, Sense::hover());
    let subtitle = if item.subtitle.is_empty() { item.kind.label().to_owned() } else { item.subtitle.clone() };
    clipped_label(&mut child, &subtitle, theme::regular(13.0), theme::TEXT_DIM, width, Sense::hover());
    if play_clicked {
        play_item(cx, item);
    } else if response.clicked() {
        open_item(cx, item);
    }
    response.context_menu(|ui| item_menu(ui, cx, item));
    response
}

pub fn item_menu(ui: &mut Ui, cx: &mut Cx<'_>, item: &Item) {
    if let Some(track) = &item.track {
        track_menu(ui, cx, track, RowPlace::List);
        return;
    }
    ui.set_min_width(200.0);
    let entry = |ui: &mut Ui, icon: Icon, text: &str| -> bool {
        ui.add(egui::Button::image_and_text(icon.image(theme::TEXT, 18.0), text).frame_when_inactive(false))
            .clicked()
    };
    if let Some(playlist_id) = &item.play_playlist {
        if entry(ui, Icon::Play, "Play") {
            play_item(cx, item);
        }
        if entry(ui, Icon::Shuffle, "Shuffle play") {
            cx.push(Action::PlayPlaylist {
                playlist_id: playlist_id.clone(),
                shuffle: true,
                title: Some(item.title.clone()),
            });
        }
        if entry(ui, Icon::Radio, "Start radio") {
            cx.push(Action::PlayWatch {
                video_id: None,
                playlist_id: Some(format!("RDAMPL{playlist_id}")),
                params: Some("wAEB".into()),
                title: Some(format!("{} radio", item.title)),
            });
        }
        if cx.signed_in && entry(ui, Icon::Save, "Save to library") {
            cx.push(Action::SavePlaylist(playlist_id.clone(), true));
        }
    }
    if let Some(Target::Browse { browse_id, .. }) = &item.target {
        let url = match item.kind {
            Kind::Artist | Kind::Profile => format!("https://music.youtube.com/channel/{browse_id}"),
            Kind::Playlist => format!(
                "https://music.youtube.com/playlist?list={}",
                browse_id.trim_start_matches("VL")
            ),
            _ => format!("https://music.youtube.com/browse/{browse_id}"),
        };
        if entry(ui, Icon::Link, "Copy link") {
            cx.push(Action::CopyLink(url));
        }
    }
}

/// A result row for anything that is not a song (an album, an artist).
pub fn item_row(ui: &mut Ui, cx: &mut Cx<'_>, item: &Item) -> Response {
    if let Some(track) = &item.track {
        let row = track_row(ui, cx, track, None, RowPlace::List, true);
        if row.play {
            open_item(cx, item);
        }
        return row.response;
    }
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, theme::ROW_HEIGHT), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), Color32::from_white_alpha(14));
    }
    let art = theme::ROW_HEIGHT - 16.0;
    let lead = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - art / 2.0), Vec2::splat(art));
    paint_art(ui, cx.images, item.thumbnail.as_deref(), lead, 4.0, item.kind.round());
    let x = lead.right() + 14.0;
    let w = rect.right() - x - 16.0;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(x, rect.top()), rect.max)));
    child.add_space((theme::ROW_HEIGHT - 38.0) / 2.0);
    child.spacing_mut().item_spacing.y = 2.0;
    clipped_label(&mut child, &item.title, theme::medium(14.5), theme::TEXT, w, Sense::hover());
    let subtitle = if item.subtitle.is_empty() {
        item.kind.label().to_owned()
    } else if matches!(item.kind, Kind::Artist | Kind::Playlist | Kind::Profile | Kind::Podcast)
        && !item.subtitle.starts_with(item.kind.label())
    {
        format!("{} \u{2022} {}", item.kind.label(), item.subtitle)
    } else {
        item.subtitle.clone()
    };
    clipped_label(&mut child, &subtitle, theme::regular(13.0), theme::TEXT_DIM, w, Sense::hover());
    if response.clicked() {
        open_item(cx, item);
    }
    response.context_menu(|ui| item_menu(ui, cx, item));
    response
}

// ---- Sections ---------------------------------------------------------------

pub fn section_title(ui: &mut Ui, cx: &mut Cx<'_>, section: &Section) {
    if section.title.is_empty() {
        return;
    }
    if let Some(strapline) = &section.strapline {
        ui.label(RichText::new(strapline.to_uppercase()).font(theme::medium(12.0)).color(theme::TEXT_DIM));
    }
    ui.horizontal(|ui| {
        let title = ui.add(
            egui::Label::new(RichText::new(&section.title).font(theme::bold(22.0)).color(theme::TEXT))
                .sense(if section.more.is_some() { Sense::click() } else { Sense::hover() }),
        );
        if section.more.is_some() {
            let title = title.on_hover_cursor(CursorIcon::PointingHand);
            if title.clicked()
                && let Some(more) = &section.more
            {
                cx.push(Action::Open(more.clone()));
            }
        }
        if let Some(more) = &section.more {
            ui.with_layout(EguiLayout::right_to_left(Align::Center), |ui| {
                if pill(ui, None, "More", false).clicked() {
                    cx.push(Action::Open(more.clone()));
                }
            });
        }
    });
    ui.add_space(10.0);
}

pub fn section(ui: &mut Ui, cx: &mut Cx<'_>, section: &Section, key: &str, index: usize) {
    match section.layout {
        Layout::Text => {
            if !section.title.is_empty() {
                ui.label(RichText::new(&section.title).font(theme::bold(22.0)));
                ui.add_space(8.0);
            }
            if let Some(text) = &section.text {
                let id = Id::new(("text-expanded", key, index));
                let expanded = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
                let shown = if expanded || text.len() < 600 {
                    text.clone()
                } else {
                    format!("{}\u{2026}", text.chars().take(560).collect::<String>())
                };
                ui.label(RichText::new(shown).color(theme::TEXT_DIM).font(theme::regular(14.5)));
                if text.len() >= 600
                    && ui
                        .add(egui::Label::new(RichText::new(if expanded { "Show less" } else { "Show more" }).font(theme::medium(14.0))).sense(Sense::click()))
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                {
                    ui.data_mut(|d| d.insert_temp(id, !expanded));
                }
            }
        }
        Layout::Card => {
            section_title(ui, cx, section);
            if let Some(item) = section.items.first() {
                top_result(ui, cx, item);
            }
        }
        Layout::List => {
            section_title(ui, cx, section);
            let tracks: Vec<Track> = section.items.iter().filter_map(|i| i.track.clone()).collect();
            let all_tracks = tracks.len() == section.items.len() && !tracks.is_empty();
            for item in &section.items {
                if all_tracks && let Some(track) = &item.track {
                    let row = track_row(ui, cx, track, None, RowPlace::List, true);
                    if row.play {
                        open_item(cx, item);
                    }
                } else {
                    item_row(ui, cx, item);
                }
            }
        }
        Layout::Carousel => {
            section_title(ui, cx, section);
            if !section.items.is_empty() && section.items.iter().all(|i| i.kind == Kind::Category) {
                category_grid(ui, cx, &section.items);
            } else {
                carousel(ui, cx, section, key, index);
            }
        }
        Layout::Grid => {
            section_title(ui, cx, section);
            if section.items.iter().all(|i| i.track.is_some()) && !section.items.is_empty() {
                song_grid(ui, cx, &section.items, key, index);
            } else if section.items.iter().all(|i| i.kind == Kind::Category) {
                category_grid(ui, cx, &section.items);
            } else {
                card_grid(ui, cx, &section.items);
            }
        }
    }
}

fn top_result(ui: &mut Ui, cx: &mut Cx<'_>, item: &Item) {
    let width = ui.available_width().min(560.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 124.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let fill = if response.hovered() { theme::SURFACE_HOVER } else { theme::SURFACE };
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    let art = Rect::from_min_size(rect.min + vec2(16.0, 16.0), Vec2::splat(92.0));
    paint_art(ui, cx.images, item.thumbnail.as_deref(), art, 6.0, item.kind.round());
    let x = art.right() + 20.0;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(x, rect.top() + 24.0), rect.max - vec2(16.0, 0.0))));
    child.spacing_mut().item_spacing.y = 4.0;
    clipped_label(&mut child, &item.title, theme::bold(22.0), theme::TEXT, rect.right() - x - 16.0, Sense::hover());
    clipped_label(&mut child, &item.subtitle, theme::regular(14.0), theme::TEXT_DIM, rect.right() - x - 16.0, Sense::hover());
    child.add_space(4.0);
    let mut play_clicked = false;
    child.horizontal(|ui| {
        if (item.track.is_some() || item.play_playlist.is_some() || item.kind == Kind::Artist)
            && item.kind != Kind::Artist
            && pill(ui, Some(Icon::Play), "Play", true).clicked()
        {
            play_clicked = true;
        }
    });
    if play_clicked {
        play_item(cx, item);
    } else if response.clicked() {
        open_item(cx, item);
    }
    response.context_menu(|ui| item_menu(ui, cx, item));
    ui.add_space(8.0);
}

/// Cards in a row that scrolls sideways, with arrows to page through.
fn carousel(ui: &mut Ui, cx: &mut Cx<'_>, section: &Section, key: &str, index: usize) {
    let width = ui.available_width();
    let card_w = card_width(width);
    let id = Id::new(("carousel", key, index));
    let target: Option<f32> = ui.data_mut(|d| d.remove_temp(id.with("go")));
    let mut area = egui::ScrollArea::horizontal()
        .id_salt(id)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
    if let Some(x) = target {
        area = area.horizontal_scroll_offset(x);
    }
    let output = area.show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            for item in &section.items {
                card(ui, cx, item, card_w);
            }
        });
    });
    // Arrows over the ends while hovered.
    let rect = output.inner_rect;
    let offset = output.state.offset.x;
    let max = (output.content_size.x - rect.width()).max(0.0);
    let hovered = ui.rect_contains_pointer(rect);
    if hovered && max > 1.0 {
        let page = (rect.width() - card_w).max(card_w);
        let art_mid = rect.top() + card_w / 2.0;
        for (left, enabled) in [(true, offset > 1.0), (false, offset < max - 1.0)] {
            if !enabled {
                continue;
            }
            let c = if left { pos2(rect.left() + 22.0, art_mid) } else { pos2(rect.right() - 22.0, art_mid) };
            let button = Rect::from_center_size(c, Vec2::splat(40.0));
            let r = ui.interact(button, id.with(left), Sense::click());
            ui.painter().circle_filled(c, 20.0, Color32::from_black_alpha(if r.hovered() { 240 } else { 190 }));
            ui.painter().circle_stroke(c, 20.0, Stroke::new(1.0, Color32::from_white_alpha(30)));
            let icon = if left { Icon::ChevronLeft } else { Icon::ChevronRight };
            icon.image(Color32::WHITE, 22.0).paint_at(ui, Rect::from_center_size(c, Vec2::splat(22.0)));
            if r.clicked() {
                let to = if left { (offset - page).max(0.0) } else { (offset + page).min(max) };
                ui.data_mut(|d| d.insert_temp(id.with("go"), to));
            }
        }
    }
}

fn card_width(available: f32) -> f32 {
    let columns = ((available + 18.0) / (theme::CARD + 18.0)).floor().max(2.0);
    ((available + 18.0) / columns - 18.0).clamp(140.0, 210.0)
}

pub fn card_grid(ui: &mut Ui, cx: &mut Cx<'_>, items: &[Item]) {
    let width = ui.available_width();
    let columns = ((width + 18.0) / (theme::CARD + 18.0)).floor().max(2.0) as usize;
    let card_w = (width + 18.0) / columns as f32 - 18.0;
    for row in items.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            for item in row {
                card(ui, cx, item, card_w);
            }
        });
        ui.add_space(12.0);
    }
}

/// Quick picks: songs in columns of four, paged sideways.
fn song_grid(ui: &mut Ui, cx: &mut Cx<'_>, items: &[Item], key: &str, index: usize) {
    let width = ui.available_width();
    let column_w = (width / ((width / 380.0).floor().max(1.0))).min(460.0) - 12.0;
    egui::ScrollArea::horizontal()
        .id_salt(("song-grid", key, index))
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for column in items.chunks(4) {
                    ui.allocate_ui_with_layout(vec2(column_w, 4.0 * theme::ROW_HEIGHT), EguiLayout::top_down(Align::Min), |ui| {
                        ui.set_width(column_w);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for item in column {
                            if let Some(track) = &item.track {
                                let row = track_row(ui, cx, track, None, RowPlace::List, false);
                                if row.play {
                                    open_item(cx, item);
                                }
                            }
                        }
                    });
                }
            });
        });
    ui.add_space(8.0);
}

/// Moods and genres: coloured buttons.
fn category_grid(ui: &mut Ui, cx: &mut Cx<'_>, items: &[Item]) {
    let width = ui.available_width();
    let columns = ((width + 12.0) / (200.0 + 12.0)).floor().max(2.0) as usize;
    let w = (width + 12.0) / columns as f32 - 12.0;
    for row in items.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            for item in row {
                let (rect, response) = ui.allocate_exact_size(vec2(w, 48.0), Sense::click());
                let response = response.on_hover_cursor(CursorIcon::PointingHand);
                let fill = if response.hovered() { theme::SURFACE_HOVER } else { theme::SURFACE };
                ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
                let stripe = item.color.map_or(theme::ACCENT, |c| {
                    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
                });
                ui.painter().rect_filled(
                    Rect::from_min_size(rect.min, vec2(6.0, rect.height())),
                    CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 },
                    stripe,
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(18.0, 0.0))).layout(EguiLayout::left_to_right(Align::Center)));
                clipped_label(&mut child, &item.title, theme::medium(14.5), theme::TEXT, w - 30.0, Sense::hover());
                if response.clicked() {
                    open_item(cx, item);
                }
            }
        });
        ui.add_space(12.0);
    }
}

/// The tabs atop the queue and lyrics ("UP NEXT", "LYRICS"), with one
/// divider under the whole row, the close button's part too. Returns the
/// tab clicked, and whether the close button was.
pub fn tab_header(ui: &mut Ui, current: crate::model::SideTab, close: bool) -> (Option<crate::model::SideTab>, bool) {
    use crate::model::SideTab;
    let row = ui.available_rect_before_wrap();
    let height = 40.0;
    ui.painter().hline(row.x_range(), row.top() + height - 1.0, Stroke::new(1.0, theme::DIVIDER));
    let mut picked = None;
    let mut closed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let tabs_w = if close { row.width() - 40.0 } else { row.width() };
        for (tab, label) in [(SideTab::UpNext, "UP NEXT"), (SideTab::Lyrics, "LYRICS")] {
            let selected = current == tab;
            let (rect, response) = ui.allocate_exact_size(vec2((tabs_w / 2.0).max(80.0), height), Sense::click());
            let response = response.on_hover_cursor(CursorIcon::PointingHand);
            let color = if selected || response.hovered() { theme::TEXT } else { theme::TEXT_DIM };
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, theme::semibold(13.5), color);
            if selected {
                ui.painter().hline(rect.x_range(), rect.bottom() - 1.0, Stroke::new(2.0, theme::TEXT));
            }
            if response.clicked() && !selected {
                picked = Some(tab);
            }
        }
        if close {
            ui.allocate_ui_with_layout(vec2(40.0, height), EguiLayout::right_to_left(Align::Center), |ui| {
                closed = icon_button(ui, Icon::Close, 18.0, theme::TEXT_DIM, "Close").clicked();
            });
        }
    });
    (picked, closed)
}
