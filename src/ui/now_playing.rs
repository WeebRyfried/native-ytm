//! The full-window player: the artwork large on the left over a wash of its
//! own colours, the title and artists under it, and what plays next or the
//! lyrics on the right. The player bar stays below.

use egui::{Align, Color32, CornerRadius, CursorIcon, Layout, Mesh, Rect, RichText, Sense, Shape, Ui, UiBuilder, pos2, vec2};

use crate::app::{App, Upgrade, UpgradePhase};
use crate::model::{Action, SideTab};
use crate::theme::{self, Icon};
use crate::ui::{lyrics, side_panel, widgets};

/// `rise`: how far below its place the content still is, opening; `foot`:
/// room kept free at the bottom (the floating bar's, in full screen);
/// `chrome`: how much the buttons along the top show (in full screen they
/// go with the bar while the mouse rests).
pub fn show(app: &mut App, ui: &mut Ui, rect: Rect, rise: f32, foot: f32, chrome: f32) {
    let track = app.current().cloned();
    backdrop(app, ui, rect, track.as_ref().and_then(|t| t.thumbnail.as_deref()));

    if chrome > 0.0 {
        let close = Rect::from_min_size(rect.min + vec2(20.0, 16.0 + rise), vec2(40.0, 40.0));
        let mut bar = ui.new_child(UiBuilder::new().max_rect(close).layout(Layout::left_to_right(Align::Center)));
        bar.multiply_opacity(chrome);
        if widgets::icon_button(&mut bar, Icon::ChevronDown, 26.0, theme::TEXT, "Close the player (Esc)").clicked() {
            app.actions.push(Action::ToggleFullView);
        }
        let corner = Rect::from_min_size(pos2(rect.right() - 60.0, rect.top() + 16.0 + rise), vec2(40.0, 40.0));
        let mut bar = ui.new_child(UiBuilder::new().max_rect(corner).layout(Layout::left_to_right(Align::Center)));
        bar.multiply_opacity(chrome);
        let (icon, tip) = if app.fullscreen {
            (Icon::ExitFullscreen, "Exit full screen (Esc)")
        } else {
            (Icon::Fullscreen, "Full screen (F11)")
        };
        if widgets::icon_button(&mut bar, icon, 22.0, theme::TEXT, tip).clicked() {
            app.actions.push(Action::ToggleFullscreen);
        }
    }

    let Some(track) = track else {
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
        child.add_space(rect.height() * 0.4);
        child.vertical_centered(|ui| {
            ui.label(RichText::new("Nothing is playing").font(theme::semibold(20.0)).color(theme::TEXT_DIM));
        });
        return;
    };

    let content = Rect::from_min_max(rect.min + vec2(56.0, 72.0), rect.max - vec2(48.0, 28.0 + foot)).translate(vec2(0.0, rise));
    let narrow = content.width() < 820.0;
    let right_w = if narrow { content.width() * 0.48 } else { (content.width() * 0.4).clamp(360.0, 560.0) };
    let gap = if narrow { 32.0 } else { (content.width() * 0.07).clamp(56.0, 112.0) };
    // The artwork, as large as fits above its titles.
    let titles_h = 112.0;
    let art_size = (content.width() - right_w - gap).min(content.height() - titles_h).clamp(140.0, 660.0);
    // The artwork and the list as one piece, centred: the same margin on
    // either side, and the list close to the artwork it belongs to.
    let total = art_size + gap + right_w;
    let x0 = content.left() + ((content.width() - total) / 2.0).max(0.0);
    let left = Rect::from_min_max(pos2(x0, content.top()), pos2(x0 + art_size, content.bottom()));
    let right = Rect::from_min_max(pos2(left.right() + gap, content.top()), pos2((left.right() + gap + right_w).min(content.right()), content.bottom()));
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
    // A new song's artwork fades in over the last one's, settling from a
    // touch smaller.
    let art_id = ui.id().with("np-art");
    let (last_url, shown_url): (Option<String>, Option<String>) = ui.data(|d| d.get_temp(art_id)).unwrap_or_default();
    let (last_url, shown_url) = if shown_url != track.thumbnail {
        let pair = (shown_url, track.thumbnail.clone());
        ui.data_mut(|d| d.insert_temp(art_id, pair.clone()));
        pair
    } else {
        (last_url, shown_url)
    };
    let key = egui::Id::new(&shown_url).value();
    let t = theme::appear(ui.ctx(), art_id.with("t"), key, 0.38);
    if t < 1.0 && last_url.is_some() {
        widgets::paint_art_faded(ui, &mut app.images, last_url.as_deref(), art, 12.0, false, 1.0 - t);
    }
    let settle = art.shrink(art.width() * 0.015 * (1.0 - t));
    widgets::paint_art_faded(ui, &mut app.images, shown_url.as_deref(), settle, 12.0, false, if last_url.is_some() { t } else { 1.0 });

    let titles = Rect::from_min_size(pos2(art.left(), art.bottom() + 24.0), vec2(art.width(), titles_h - 24.0));
    let mut text = ui.new_child(UiBuilder::new().max_rect(titles).layout(Layout::top_down(Align::Min)));
    text.spacing_mut().item_spacing.y = 4.0;
    // The titles fade in with the new song.
    let fade = theme::appear(ui.ctx(), ui.id().with("np-titles"), egui::Id::new(&track.video_id).value(), 0.3);
    text.multiply_opacity(fade);
    let width = titles.width();
    let big = art_size > 420.0;
    let title_font = theme::bold(if big { 28.0 } else { 22.0 });
    let shown = app.shown_title(&track).to_owned();
    // The year it came out, small and grey, just after the title.
    let year = track.year.or_else(|| (app.now.video_id == track.video_id).then_some(app.now.year).flatten());
    let year = year.map(|y| text.painter().layout_no_wrap(y.to_string(), theme::medium(if big { 16.0 } else { 14.0 }), theme::TEXT_DIM));
    let year_w = year.as_ref().map_or(0.0, |g| g.size().x + 12.0);
    let response = widgets::clipped_label(&mut text, &shown, title_font, theme::TEXT, width - year_w, Sense::hover());
    if let Some(year) = year {
        // On the title's baseline, four fifths down each line.
        let title = response.rect;
        let at = pos2(title.right() + 12.0, title.top() + title.height() * 0.8 - year.size().y * 0.8);
        text.painter().galley(at, year, theme::TEXT_DIM.gamma_multiply(0.9));
    }
    if shown != track.title {
        response.on_hover_text(&track.title);
    }
    let mut links = track.artists.clone();
    if let Some(album) = &track.album {
        links.push(album.clone());
    }
    let quality = app.stream_quality().map(str::to_owned);
    let upgrade = app.upgrade.clone().filter(|u| u.token == app.current_token());
    let mut cx = app.cx();
    if links.is_empty() {
        widgets::clipped_label(&mut text, track.extra.as_deref().unwrap_or(""), theme::medium(17.0), theme::TEXT_DIM, width, Sense::hover());
    } else {
        widgets::links(&mut text, &mut cx, &links, theme::medium(17.0), theme::TEXT_DIM, width);
    }
    if quality.is_some() || upgrade.is_some() {
        text.add_space(6.0);
        quality_chip(&mut text, quality.as_deref(), upgrade.as_ref());
    }

    // What plays next, or the words.
    let mut side = ui.new_child(UiBuilder::new().max_rect(right).layout(Layout::top_down(Align::Min)));
    // egui fades a list's edges into the panel colour, which is not what
    // shows here: the backdrop is.
    side.spacing_mut().scroll.fade.strength = 0.0;
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

/// The stream playing ("OPUS 257 KB/S"), small and quiet. On a quick
/// start it tells the story of the switch: a shimmer while the better
/// stream is fetched and while it waits its turn in the queue, then the
/// new label fading in with one pulse of the accent and a sparkle that
/// stays.
fn quality_chip(ui: &mut Ui, quality: Option<&str>, upgrade: Option<&Upgrade>) {
    let id = ui.id().with("quality-chip");
    let now = ui.input(|i| i.time);
    let font = theme::semibold(11.0);
    let (label, note, working, done_for) = match upgrade.map(|u| (u.phase, u)) {
        Some((UpgradePhase::Finding, u)) => (
            u.from.as_deref().or(quality).map_or_else(|| "STARTING".to_owned(), str::to_uppercase),
            Some("GETTING 256 KB/S".to_owned()),
            true,
            None,
        ),
        Some((UpgradePhase::Switching(_), u)) => (
            u.from.as_deref().or(quality).unwrap_or_default().to_uppercase(),
            Some(format!("\u{2192} {}", u.to.as_deref().unwrap_or("256 kb/s").to_uppercase())),
            true,
            None,
        ),
        Some((UpgradePhase::Done(at), u)) => (
            u.to.as_deref().or(quality).unwrap_or_default().to_uppercase(),
            None,
            false,
            Some(at.elapsed().as_secs_f32()),
        ),
        _ => (quality.unwrap_or_default().to_uppercase(), None, false, None),
    };
    let premium = upgrade.is_some_and(|u| matches!(u.phase, UpgradePhase::Done(_)) && u.to.is_some());
    let painter = ui.painter().clone();
    let mark = codec_mark(&label);
    let galley = painter.layout_no_wrap(label, font.clone(), theme::TEXT_DIM);
    let note_galley = note.map(|n| painter.layout_no_wrap(n, font.clone(), theme::TEXT));
    let sparkle = if premium { 15.0 } else { 0.0 };
    let mark_h = 11.0;
    let mark_w = mark.map_or(0.0, |(_, _, aspect)| mark_h * aspect + 5.0);
    let note_w = note_galley.as_ref().map_or(0.0, |g| g.size().x + 10.0);
    let target_w = galley.size().x + 14.0 + sparkle + mark_w + note_w;
    // The chip grows and shrinks with what it says.
    let width = ui.ctx().animate_value_with_time(id.with("w"), target_w, 0.28);
    let (rect, response) = ui.allocate_exact_size(vec2(width, galley.size().y + 8.0), Sense::hover());
    let clip = painter.with_clip_rect(rect.intersect(ui.clip_rect()));
    let radius = CornerRadius::same(4);
    clip.rect_filled(rect, radius, Color32::from_white_alpha(18));
    if working {
        // A soft band of light sweeping across while the better stream comes.
        let band = 46.0;
        let x = rect.left() - band + ((now * 0.55).fract() as f32) * (rect.width() + band * 2.0);
        let mut mesh = Mesh::default();
        let clear = Color32::TRANSPARENT;
        let light = Color32::from_white_alpha(26);
        for (dx, color) in [(0.0, clear), (band, light), (band * 2.0, clear)] {
            mesh.colored_vertex(pos2(x + dx, rect.top()), color);
            mesh.colored_vertex(pos2(x + dx, rect.bottom()), color);
        }
        for i in [0u32, 2] {
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i + 1, i + 2, i + 3);
        }
        clip.add(Shape::mesh(mesh));
        theme::repaint_in(ui.ctx(), std::time::Duration::from_millis(33));
    }
    let mut x = rect.left() + 7.0;
    if premium {
        // The sparkle pops in with a little overshoot, then stays.
        let t = done_for.map_or(1.0, |s| (s / 0.45).min(1.0));
        let scale = ease_out_back(t);
        let size = 11.0 * scale;
        Icon::Mix
            .image(theme::ACCENT_TEXT, size.max(1.0))
            .paint_at(ui, Rect::from_center_size(pos2(x + 5.5, rect.center().y), vec2(size, size)));
        x += sparkle;
    }
    // The new label fades in over the old.
    let fade_in = done_for.map_or(1.0, |s| (s / 0.4).min(1.0));
    // The codec's own symbol before its name.
    if let Some((uri, bytes, aspect)) = mark {
        let at = Rect::from_min_size(pos2(x, rect.center().y - mark_h / 2.0), vec2(mark_h * aspect, mark_h));
        if clip.clip_rect().intersects(at) {
            egui::Image::from_bytes(uri, bytes)
                .tint(Color32::WHITE.gamma_multiply(fade_in))
                .paint_at(ui, at);
        }
        x += mark_w;
    }
    let label_color = theme::TEXT_DIM.gamma_multiply(fade_in);
    clip.galley_with_override_text_color(pos2(x, rect.center().y - galley.size().y / 2.0), galley.clone(), label_color);
    if let Some(note) = note_galley {
        let pulse = 0.55 + 0.45 * ((now * 2.6).sin() as f32 * 0.5 + 0.5);
        let at = pos2(x + galley.size().x + 10.0, rect.center().y - note.size().y / 2.0);
        clip.galley_with_override_text_color(at, note, theme::TEXT.gamma_multiply(pulse));
    }
    if let Some(since) = done_for
        && since < 1.6
    {
        // One pulse of the accent around the chip as the switch is heard.
        let t = since / 1.6;
        let alpha = (1.0 - t).powi(2);
        let grow = 1.0 + 5.0 * t;
        painter.rect_stroke(
            rect.expand(grow),
            CornerRadius::same(6),
            egui::Stroke::new(1.5, theme::ACCENT.gamma_multiply(alpha)),
            egui::StrokeKind::Outside,
        );
        theme::next_frame(ui.ctx());
    }
    let tip = match upgrade.map(|u| u.phase) {
        Some(UpgradePhase::Finding) => "Playing at once in standard quality while Premium quality is fetched",
        Some(UpgradePhase::Switching(_)) => "Premium quality is ready and takes over in a moment, without a break",
        Some(UpgradePhase::Done(_)) if premium => "Switched to Premium quality without a break",
        _ => "The stream playing: its codec and bitrate",
    };
    response.on_hover_cursor(CursorIcon::Default).on_hover_text(tip);
}

/// The codec's symbol for a label such as "OPUS 257 KB/S": its image and
/// its width for its height.
fn codec_mark(label: &str) -> Option<(&'static str, &'static [u8], f32)> {
    const MARKS: [(&str, &str, &[u8], f32); 8] = [
        ("OPUS", "bytes://codec-opus.svg", include_bytes!("../../assets/codecs/opus.svg"), 193.2 / 129.2),
        ("AAC", "bytes://codec-aac.svg", include_bytes!("../../assets/codecs/aac.svg"), 908.0 / 783.0),
        ("FLAC", "bytes://codec-flac.svg", include_bytes!("../../assets/codecs/flac.svg"), 24.0 / 20.0),
        ("MP3", "bytes://codec-mp3.svg", include_bytes!("../../assets/codecs/mp3.svg"), 1.0),
        ("WAV", "bytes://codec-wav.svg", include_bytes!("../../assets/codecs/wav.svg"), 31.0 / 20.0),
        ("AIFF", "bytes://codec-aiff.svg", include_bytes!("../../assets/codecs/aiff.svg"), 31.0 / 20.0),
        ("ALAC", "bytes://codec-alac.svg", include_bytes!("../../assets/codecs/alac.svg"), 1.0),
        ("VORBIS", "bytes://codec-vorbis.svg", include_bytes!("../../assets/codecs/vorbis.svg"), 1.0),
    ];
    MARKS
        .iter()
        .find(|(name, ..)| label.split_whitespace().next() == Some(name))
        .map(|&(_, uri, bytes, aspect)| (uri, bytes, aspect))
}

/// Eases to 1 with a small overshoot, for things that pop in.
fn ease_out_back(t: f32) -> f32 {
    let (c1, c3) = (1.70158, 2.70158);
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// The artwork's colours, blurred across the window and darkened so text
/// reads on them, fading from the last song's when the song changes.
/// One textured pass per image: the darkening, and the deepening toward the
/// player bar, are the mesh's vertex colours rather than layers on top.
fn backdrop(app: &mut App, ui: &Ui, rect: Rect, url: Option<&str>) {
    let painter = ui.painter_at(rect);
    let id = ui.id().with("np-backdrop");
    let (mut current, mut previous): (Option<String>, Option<String>) = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if current.as_deref() != url {
        previous = current.take();
        current = url.map(str::to_owned);
        ui.data_mut(|d| d.insert_temp(id, (current.clone(), previous.clone())));
    }
    // How much of the image's brightness shows, at the top and the bottom.
    const TOP: f32 = 0.51;
    const BOTTOM: f32 = 0.22;
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
        let shade = |level: f32| {
            let v = (level * alpha * 255.0).round() as u8;
            Color32::from_rgba_premultiplied(v, v, v, (alpha * 255.0).round() as u8)
        };
        let mut mesh = Mesh::with_texture(texture.id());
        mesh.vertices.extend([
            egui::epaint::Vertex { pos: rect.left_top(), uv: uv.left_top(), color: shade(TOP) },
            egui::epaint::Vertex { pos: rect.right_top(), uv: uv.right_top(), color: shade(TOP) },
            egui::epaint::Vertex { pos: rect.right_bottom(), uv: uv.right_bottom(), color: shade(BOTTOM) },
            egui::epaint::Vertex { pos: rect.left_bottom(), uv: uv.left_bottom(), color: shade(BOTTOM) },
        ]);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(mesh));
    };
    let images = &mut app.images;
    let fresh = current.as_deref().and_then(|u| images.backdrop(u));
    let old = previous.as_deref().and_then(|u| images.backdrop(u));
    match (&fresh, &old) {
        (Some(texture), old) => {
            let t = ui.ctx().animate_bool_with_time(id.with(current.as_deref()), true, 0.8);
            if t < 1.0 {
                match old {
                    Some(old) => paint(old, 1.0),
                    None => {
                        painter.rect_filled(rect, CornerRadius::ZERO, theme::BG);
                    }
                }
            }
            paint(texture, t);
        }
        (None, Some(old)) => paint(old, 1.0),
        (None, None) => {
            painter.rect_filled(rect, CornerRadius::ZERO, theme::BG);
        }
    }
}
