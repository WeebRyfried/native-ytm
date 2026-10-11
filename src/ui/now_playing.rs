//! The full-window player: the artwork large on the left over a wash of its
//! own colours, the title and artists under it, and what plays next or the
//! lyrics on the right. The player bar stays below.

use egui::{Align, Color32, CornerRadius, CursorIcon, Layout, Mesh, Rect, RichText, Sense, Shape, Ui, UiBuilder, pos2, vec2};

use crate::app::App;
use crate::model::{Action, SideTab};
use crate::theme::{self, Icon};
use crate::ui::{lyrics, side_panel, widgets};

pub fn show(app: &mut App, ui: &mut Ui) {
    let rect = ui.max_rect();
    let track = app.current().cloned();
    backdrop(app, ui, rect, track.as_ref().and_then(|t| t.thumbnail.as_deref()));

    let close = Rect::from_min_size(rect.min + vec2(20.0, 16.0), vec2(40.0, 40.0));
    let mut bar = ui.new_child(UiBuilder::new().max_rect(close).layout(Layout::left_to_right(Align::Center)));
    if widgets::icon_button(&mut bar, Icon::ChevronDown, 26.0, theme::TEXT, "Close the player (Esc)").clicked() {
        app.actions.push(Action::ToggleFullView);
    }

    let Some(track) = track else {
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
        child.add_space(rect.height() * 0.4);
        child.vertical_centered(|ui| {
            ui.label(RichText::new("Nothing is playing").font(theme::semibold(20.0)).color(theme::TEXT_DIM));
        });
        return;
    };

    let content = Rect::from_min_max(rect.min + vec2(56.0, 72.0), rect.max - vec2(48.0, 28.0));
    let narrow = content.width() < 820.0;
    let right_w = if narrow { content.width() * 0.48 } else { (content.width() * 0.4).clamp(360.0, 560.0) };
    let gap = if narrow { 32.0 } else { 64.0 };
    let left = Rect::from_min_max(content.min, pos2(content.right() - right_w - gap, content.bottom()));
    let right = Rect::from_min_max(pos2(content.right() - right_w, content.top()), content.max);

    // The artwork, as large as fits above its titles.
    let titles_h = 112.0;
    let art_size = left.width().min(left.height() - titles_h).clamp(140.0, 660.0);
    let block_h = art_size + titles_h;
    let art_top = left.top() + ((left.height() - block_h) / 2.0).max(0.0);
    let art = Rect::from_min_size(pos2(left.center().x - art_size / 2.0, art_top), vec2(art_size, art_size));
    let shadow = egui::Shadow {
        offset: [0, 18],
        blur: 60,
        spread: 0,
        color: Color32::from_black_alpha(150),
    };
    ui.painter().add(shadow.as_shape(art, CornerRadius::same(12)));
    widgets::paint_art(ui, &mut app.images, track.thumbnail.as_deref(), art, 12.0, false);

    let titles = Rect::from_min_size(pos2(art.left(), art.bottom() + 24.0), vec2(art.width(), titles_h - 24.0));
    let mut text = ui.new_child(UiBuilder::new().max_rect(titles).layout(Layout::top_down(Align::Min)));
    text.spacing_mut().item_spacing.y = 4.0;
    let width = titles.width();
    let title_font = theme::bold(if art_size > 420.0 { 28.0 } else { 22.0 });
    widgets::clipped_label(&mut text, &track.title, title_font, theme::TEXT, width, Sense::hover());
    let mut links = track.artists.clone();
    if let Some(album) = &track.album {
        links.push(album.clone());
    }
    let quality = app.stream_quality().map(str::to_owned);
    let mut cx = app.cx();
    if links.is_empty() {
        widgets::clipped_label(&mut text, track.extra.as_deref().unwrap_or(""), theme::medium(17.0), theme::TEXT_DIM, width, Sense::hover());
    } else {
        widgets::links(&mut text, &mut cx, &links, theme::medium(17.0), theme::TEXT_DIM, width);
    }
    if let Some(quality) = quality {
        text.add_space(6.0);
        quality_chip(&mut text, &quality);
    }

    // What plays next, or the words.
    let mut side = ui.new_child(UiBuilder::new().max_rect(right).layout(Layout::top_down(Align::Min)));
    let (picked, _) = widgets::tab_header(&mut side, app.full_tab, false);
    if let Some(tab) = picked {
        app.actions.push(Action::ShowSidePanel(tab));
    }
    side.add_space(12.0);
    match app.full_tab {
        SideTab::UpNext => side_panel::queue(app, &mut side),
        SideTab::Lyrics => lyrics::show(app, &mut side, true),
    }
}

/// "OPUS 257 KB/S", small and quiet, so the stream in use can be checked.
fn quality_chip(ui: &mut Ui, quality: &str) {
    let text = quality.to_uppercase();
    let galley = ui.painter().layout_no_wrap(text, theme::semibold(11.0), theme::TEXT_DIM);
    let size = galley.size() + vec2(14.0, 8.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(4), Color32::from_white_alpha(18));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, theme::TEXT_DIM);
    response
        .on_hover_cursor(CursorIcon::Default)
        .on_hover_text("The stream playing: its codec and bitrate");
}

/// The artwork's colours, blurred across the window and darkened so text
/// reads on them, fading from the last song's when the song changes.
fn backdrop(app: &mut App, ui: &Ui, rect: Rect, url: Option<&str>) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, theme::BG);
    let id = ui.id().with("np-backdrop");
    let (mut current, mut previous): (Option<String>, Option<String>) = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if current.as_deref() != url {
        previous = current.take();
        current = url.map(str::to_owned);
        ui.data_mut(|d| d.insert_temp(id, (current.clone(), previous.clone())));
    }
    let images = &mut app.images;
    let paint = |texture: &egui::TextureHandle, alpha: f32| {
        // Cover the window, keeping the square's proportions.
        let aspect = rect.width() / rect.height().max(1.0);
        let uv = if aspect > 1.0 {
            let h = 1.0 / aspect;
            Rect::from_min_max(pos2(0.0, 0.5 - h / 2.0), pos2(1.0, 0.5 + h / 2.0))
        } else {
            let w = aspect;
            Rect::from_min_max(pos2(0.5 - w / 2.0, 0.0), pos2(0.5 + w / 2.0, 1.0))
        };
        let tint = Color32::from_white_alpha((alpha * 255.0) as u8);
        painter.image(texture.id(), rect, uv, tint);
    };
    let fresh = current.as_deref().and_then(|u| images.backdrop(u));
    let shown = match &fresh {
        Some(texture) => {
            let t = ui.ctx().animate_bool_with_time(id.with(current.as_deref()), true, 0.8);
            if t < 1.0
                && let Some(old) = previous.as_deref().and_then(|u| images.backdrop(u))
            {
                paint(&old, 1.0);
            }
            paint(texture, t);
            true
        }
        None => match previous.as_deref().and_then(|u| images.backdrop(u)) {
            Some(old) => {
                paint(&old, 1.0);
                true
            }
            None => false,
        },
    };
    if shown {
        // Dark enough for white text, darker toward the bottom where the
        // player bar meets it.
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(120));
        let mut mesh = Mesh::default();
        let top = Color32::from_black_alpha(10);
        let bottom = Color32::from_black_alpha(150);
        mesh.colored_vertex(rect.left_top(), top);
        mesh.colored_vertex(rect.right_top(), top);
        mesh.colored_vertex(rect.right_bottom(), bottom);
        mesh.colored_vertex(rect.left_bottom(), bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(mesh));
    }
}
