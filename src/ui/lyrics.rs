//! Lyrics, the way Apple Music shows them: synced lines follow the song,
//! the line being sung bright and the rest dim, the view gliding to keep
//! it in place, three dots breathing through instrumental breaks. Scrolling
//! by hand pauses the following for a moment; a click on a line plays from
//! there. Lyrics without times are shown as text.

use std::sync::Arc;

use egui::{Color32, CornerRadius, CursorIcon, Galley, Pos2, Rect, RichText, Sense, Ui, pos2, vec2};

use crate::api::model::{LyricLine, Lyrics};
use crate::app::App;
use crate::model::{Action, Loadable};
use crate::readings::{self, Dictionary, Style};
use crate::theme::{self, Icon};
use crate::ui::widgets;

/// A line laid out, with the readings over it: each one's centre, from
/// the line's top left, and its text.
type LaidLine = (Arc<Galley>, Vec<(Pos2, Arc<Galley>)>);

/// How long after a scroll by hand the view takes over again.
const HAND_SCROLL: f64 = 3.0;
/// Lines light up this much before their time, as a line is read just
/// before it is sung.
const LEAD: f64 = 0.2;

/// How long the view glides to a new line, and a line takes to light.
const GLIDE: f32 = 0.45;
const LIGHT: f32 = 0.35;

#[derive(Clone, Copy, Default)]
struct View {
    /// Where the view is, and the glide it is on: from, to, since when.
    offset: f32,
    from: f32,
    to: f32,
    glide_start: f64,
    /// The line lit now, the one lit before it, and when that changed.
    line: Option<usize>,
    previous: Option<usize>,
    changed_at: f64,
    hand_until: f64,
    by_hand: bool,
    placed: bool,
}

pub fn show(app: &mut App, ui: &mut Ui, big: bool) {
    let Some(track) = app.current().cloned() else {
        message(ui, "Play a song to see its lyrics");
        return;
    };
    app.request_lyrics();
    let video_id = track.video_id.clone();
    match app.lyrics.get(&video_id) {
        Some(Loadable::Ready(Some(lyrics))) => {
            let lyrics = lyrics.clone();
            if lyrics.lines.is_empty() {
                plain(app, ui, &video_id, &lyrics, big);
            } else {
                synced(app, ui, &video_id, &lyrics, big);
            }
        }
        Some(Loadable::Failed(_) | Loadable::Ready(None)) => message(ui, "Lyrics are not available for this song"),
        Some(Loadable::Loading) | None => widgets::spinner(ui),
    }
}

fn message(ui: &mut Ui, text: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| ui.label(RichText::new(text).color(theme::TEXT_DIM)));
}

/// Readings for these lyrics, when they are wanted and Japanese: the
/// dictionary (once loaded) and the style.
fn readings_for(app: &App, ui: &Ui, big: bool, texts: &[&str]) -> Option<(&'static Dictionary, Style)> {
    if !big || !app.settings.lyric_readings || !readings::is_japanese(texts) {
        return None;
    }
    let ctx = ui.ctx().clone();
    readings::dictionary(move || ctx.request_repaint()).map(|dictionary| (dictionary, app.settings.reading_style))
}

/// A line of lyrics laid out to `width`, with room above each row for the
/// readings of its words, and the readings placed.
fn lay_line(
    painter: &egui::Painter,
    text: &str,
    font: &egui::FontId,
    width: f32,
    readings: Option<(&Dictionary, Style)>,
) -> LaidLine {
    let base = painter.layout(text.to_owned(), font.clone(), Color32::WHITE, width);
    let Some((dictionary, style)) = readings else {
        return (base, Vec::new());
    };
    let rubies = readings::readings(dictionary, text, style);
    if rubies.is_empty() {
        return (base, Vec::new());
    }
    let size = font.size * if style == Style::Kana { 0.44 } else { 0.4 };
    let ppp = painter.ctx().pixels_per_point();
    let lift = (size * 1.3 * ppp).round() / ppp;
    // Each row moved down to make room over it.
    let mut galley = (*base).clone();
    let rows = galley.rows.len() as f32;
    for (i, row) in galley.rows.iter_mut().enumerate() {
        row.pos.y += lift * (i + 1) as f32;
    }
    galley.rect.max.y += lift * rows;
    galley.mesh_bounds.max.y += lift * rows;
    let galley = Arc::new(galley);
    let mut placed = Vec::with_capacity(rubies.len());
    for ruby in rubies {
        let a = galley.pos_from_cursor(egui::text::CCursor::new(ruby.start));
        let b = galley.pos_from_cursor(egui::text::CCursor::new(ruby.end));
        // A word broken over two rows has its reading over its first part.
        let (left, right) = if (a.min.y - b.min.y).abs() < 1.0 { (a.min.x, b.min.x) } else { (a.min.x, a.min.x + font.size) };
        let room = (right - left) + font.size * 0.35;
        let mut reading = painter.layout_no_wrap(ruby.text.clone(), theme::medium(size), Color32::WHITE);
        if reading.size().x > room {
            // Too long for its word: a little smaller, so neighbours stay apart.
            let fit = (size * room / reading.size().x).max(size * 0.72);
            reading = painter.layout_no_wrap(ruby.text, theme::medium(fit), Color32::WHITE);
        }
        placed.push((pos2((left + right) / 2.0, a.min.y - lift / 2.0), reading));
    }
    (galley, placed)
}

/// Paints a laid line at `at`, its readings a little fainter than it.
fn paint_line(painter: &egui::Painter, at: Pos2, line: &LaidLine, color: Color32) {
    painter.galley_with_override_text_color(at, line.0.clone(), color);
    let faint = color.gamma_multiply(0.72);
    for (center, reading) in &line.1 {
        painter.galley_with_override_text_color(at + center.to_vec2() - reading.size() / 2.0, reading.clone(), faint);
    }
}

fn plain(app: &App, ui: &mut Ui, video_id: &str, lyrics: &Lyrics, big: bool) {
    let texts: Vec<&str> = lyrics.text.lines().collect();
    let readings = readings_for(app, ui, big, &texts);
    egui::ScrollArea::vertical()
        .id_salt("plain-lyrics")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(12.0);
            let size = if big { 24.0 } else { 17.0 };
            ui.spacing_mut().item_spacing.y = size * 0.5;
            if let Some(readings) = readings {
                // Line by line, each with its readings; laid out once.
                let width = ui.available_width();
                let font = theme::semibold(size);
                let painter = ui.painter().clone();
                let id = ui.id().with(("plain-readings", video_id, width.round() as i32, readings.1));
                let laid: Arc<Vec<LaidLine>> = ui.data(|d| d.get_temp(id)).unwrap_or_else(|| {
                    let laid = Arc::new(texts.iter().map(|t| lay_line(&painter, t, &font, width, Some(readings))).collect());
                    ui.data_mut(|d| d.insert_temp(id, Arc::clone(&laid)));
                    laid
                });
                ui.spacing_mut().item_spacing.y = size * 0.3;
                for line in laid.iter() {
                    let (rect, _) = ui.allocate_exact_size(vec2(width, line.0.size().y.max(size * 0.6)), Sense::hover());
                    if ui.is_rect_visible(rect) {
                        paint_line(ui.painter(), rect.min, line, theme::TEXT);
                    }
                }
            } else {
                ui.add(egui::Label::new(RichText::new(&lyrics.text).font(theme::semibold(size)).color(theme::TEXT)).wrap());
            }
            if let Some(source) = &lyrics.source {
                ui.add_space(20.0);
                ui.label(RichText::new(source).color(theme::TEXT_FAINT).font(theme::regular(12.5)));
            }
            ui.add_space(32.0);
        });
}

fn synced(app: &mut App, ui: &mut Ui, video_id: &str, lyrics: &Lyrics, big: bool) {
    let area = ui.available_rect_before_wrap();
    let (rect, response) = ui.allocate_exact_size(area.size(), Sense::click());
    let painter = ui.painter_at(rect);
    let size = if big { 32.0 } else { 22.0 };
    let font = theme::bold(size);
    let gap = if big { 26.0 } else { 16.0 };
    let pad = if big { 10.0 } else { 6.0 };
    let width = (rect.width() - pad * 2.0).max(60.0);

    // A quiet start becomes a break of its own, with its dots.
    let mut lines: Vec<LyricLine> = Vec::with_capacity(lyrics.lines.len() + 1);
    if lyrics.lines.first().is_some_and(|l| l.time > 3.0) {
        lines.push(LyricLine { time: 0.0, text: String::new() });
    }
    lines.extend(lyrics.lines.iter().cloned());

    // Lyrics a little early or late for this recording: moved, per song.
    let offset = app.personal.lyric_offset(video_id);
    let position = app.position() + LEAD - offset;
    let playing = app.playing();
    let current = lines.iter().rposition(|l| l.time <= position);
    let dots_h = size * 0.9;

    // Where each line sits, from the top of the lyrics.
    // Laid out once per song, size, width and readings, not every frame.
    type Laid = Arc<Vec<Option<LaidLine>>>;
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    let readings = readings_for(app, ui, big, &texts);
    let scale = (ui.ctx().pixels_per_point() * 100.0).round() as i32;
    let layout_id = ui.id().with(("lyrics-layout", video_id, width.round() as i32, big, scale, readings.map(|r| r.1)));
    let galleys: Laid = ui.data(|d| d.get_temp(layout_id)).unwrap_or_else(|| {
        let laid: Laid = Arc::new(
            lines
                .iter()
                .map(|line| (!line.text.is_empty()).then(|| lay_line(&painter, &line.text, &font, width, readings)))
                .collect(),
        );
        ui.data_mut(|d| d.insert_temp(layout_id, laid.clone()));
        laid
    });
    let mut tops = Vec::with_capacity(lines.len());
    let mut y = 0.0f32;
    for (i, galley) in galleys.iter().enumerate() {
        tops.push(y);
        let h = match galley {
            Some((galley, _)) => galley.size().y,
            // A break takes room only while it is the one playing.
            None if current == Some(i) => dots_h,
            None => 0.0,
        };
        y += h + if h > 0.0 { gap } else { 0.0 };
    }
    let total = y;
    let height_of = |i: usize| -> f32 {
        match &galleys[i] {
            Some((galley, _)) => galley.size().y,
            None if current == Some(i) => dots_h,
            None => 0.0,
        }
    };

    // Follow the line being sung, unless scrolled by hand a moment ago.
    let id = ui.id().with(("lyrics-view", video_id));
    let mut view: View = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let now = ui.input(|i| i.time);
    let anchor = rect.height() * if big { 0.36 } else { 0.3 };
    let focus = current.unwrap_or(0);
    let target = tops.get(focus).copied().unwrap_or(0.0) + height_of(focus) / 2.0 - anchor;
    if !view.placed {
        view = View {
            offset: target,
            from: target,
            to: target,
            line: current,
            changed_at: now - f64::from(LIGHT),
            placed: true,
            ..View::default()
        };
    }
    if current != view.line {
        view.previous = view.line;
        view.line = current;
        view.changed_at = now;
    }
    if response.contains_pointer() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            view.offset = (view.offset - scroll).clamp(-anchor, (total - anchor).max(-anchor));
            view.hand_until = now + HAND_SCROLL;
            view.by_hand = true;
        }
    }
    if view.by_hand && now > view.hand_until {
        // Back from where the hand left it.
        view.by_hand = false;
        view.to = f32::NAN;
    }
    if !view.by_hand {
        // A new place to be (a new line, or a resize): glide there, timed
        // and eased, at no more than 60 frames a second.
        if (target - view.to).abs() > 0.5 || view.to.is_nan() {
            view.from = view.offset;
            view.to = target;
            view.glide_start = now;
        }
        let g = ((now - view.glide_start) as f32 / GLIDE).clamp(0.0, 1.0);
        view.offset = view.from + (view.to - view.from) * theme::ease_out(g);
        if g < 1.0 {
            theme::next_frame(ui.ctx());
        }
    }
    let lit = theme::ease_out((now - view.changed_at) as f32 / LIGHT);
    if lit < 1.0 {
        theme::next_frame(ui.ctx());
    }
    ui.data_mut(|d| d.insert_temp(id, view));

    let pointer = ui.ctx().pointer_hover_pos().filter(|p| rect.contains(*p) && response.contains_pointer());
    let fade = if big { 72.0 } else { 40.0 };
    let mut clicked_line = None;
    for (i, line) in lines.iter().enumerate() {
        let h = height_of(i);
        if h == 0.0 {
            continue;
        }
        let top = rect.top() + tops[i] - view.offset;
        if top > rect.bottom() || top + h < rect.top() {
            continue;
        }
        let line_rect = Rect::from_min_size(pos2(rect.left() + pad, top), vec2(width, h));
        let is_current = current == Some(i);
        // The line coming in brightens as the one before it dims.
        let alpha = if is_current {
            0.32 + 0.68 * lit
        } else if view.previous == Some(i) {
            1.0 - 0.68 * lit
        } else {
            0.32
        };
        // Lines fade out toward the edges of the view.
        let edge = ((top - rect.top()) / fade).min((rect.bottom() - top - h) / fade).clamp(0.0, 1.0);
        let alpha = alpha * (0.25 + 0.75 * edge);
        match &galleys[i] {
            Some(laid) => {
                let hovered = pointer.is_some_and(|p| line_rect.expand2(vec2(pad, gap / 2.0)).contains(p));
                if hovered {
                    painter.rect_filled(line_rect.expand2(vec2(pad, gap / 3.0)), CornerRadius::same(8), Color32::from_white_alpha(12));
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    if response.clicked() {
                        clicked_line = Some(line.time);
                    }
                }
                let color = Color32::from_white_alpha((alpha * 255.0) as u8);
                paint_line(&painter, line_rect.min, laid, color);
            }
            None => {
                let next = lines.get(i + 1).map_or(line.time + 4.0, |l| l.time);
                dots(ui, &painter, line_rect, size, (position - line.time) / (next - line.time).max(0.5), playing);
            }
        }
    }
    if let Some(source) = &lyrics.source {
        let top = rect.top() + total - view.offset + gap;
        if top < rect.bottom() && top > rect.top() - 20.0 {
            painter.text(
                pos2(rect.left() + pad, top),
                egui::Align2::LEFT_TOP,
                source,
                theme::regular(12.5),
                theme::TEXT_FAINT,
            );
        }
    }
    if response.contains_pointer() || offset != 0.0 {
        nudge(app, ui, rect, video_id, offset, response.contains_pointer());
    }
    if let Some(time) = clicked_line {
        app.actions.push(Action::Seek((time + offset).max(0.0)));
        // Back to following, from the line clicked.
        view.hand_until = 0.0;
        view.by_hand = false;
        view.to = f32::NAN;
        ui.data_mut(|d| d.insert_temp(id, view));
    }
    // Wake for the next line rather than every frame.
    if playing
        && let Some(next) = lines.iter().find(|l| l.time > position)
    {
        let wait = (next.time - position).max(0.016);
        theme::repaint_in(ui.ctx(), std::time::Duration::from_secs_f64(wait));
    }
}

/// In the lyrics' corner: lines earlier or later by half a second, for a
/// song whose lyrics run ahead of it or behind it; a click on the amount
/// puts them back. Faint while the pointer is elsewhere, and only there
/// at all then when the song's lyrics are moved.
fn nudge(app: &mut App, ui: &mut Ui, area: Rect, video_id: &str, offset: f64, pointed: bool) {
    let id = ui.id().with(("lyrics-nudge", video_id));
    let shown = ui.ctx().animate_bool_with_time(id, pointed, 0.2).max(if offset != 0.0 { 0.55 } else { 0.0 });
    if shown <= 0.01 {
        return;
    }
    let label = if offset > 0.0 {
        format!("{offset:.1} s later")
    } else if offset < 0.0 {
        format!("{:.1} s earlier", -offset)
    } else {
        "In time".to_owned()
    };
    let painter = ui.painter().clone();
    let galley = painter.layout_no_wrap(label, theme::medium(12.5), theme::TEXT);
    let (h, button) = (30.0, 32.0);
    let w = button * 2.0 + galley.size().x + 16.0;
    let bar = Rect::from_min_size(pos2(area.right() - w - 10.0, area.bottom() - h - 10.0), vec2(w, h));
    painter.rect_filled(bar, CornerRadius::same(15), theme::POPUP.gamma_multiply(shown));
    painter.rect_stroke(bar, CornerRadius::same(15), egui::Stroke::new(1.0, theme::DIVIDER.gamma_multiply(shown)), egui::StrokeKind::Inside);
    let earlier = Rect::from_min_size(bar.min, vec2(button, h));
    let later = Rect::from_min_size(pos2(bar.right() - button, bar.top()), vec2(button, h));
    let middle = Rect::from_min_max(pos2(earlier.right(), bar.top()), pos2(later.left(), bar.bottom()));
    let earlier_r = ui.interact(earlier, id.with("earlier"), Sense::click()).on_hover_text("Lines come half a second earlier");
    let later_r = ui.interact(later, id.with("later"), Sense::click()).on_hover_text("Lines come half a second later");
    let middle_r = ui.interact(middle, id.with("reset"), Sense::click());
    let middle_r = if offset != 0.0 { middle_r.on_hover_text("Back to the lyrics' own timing") } else { middle_r.on_hover_text("Lyrics early or late? Move them here; it is kept for this song") };
    for (r, rect, icon) in [(&earlier_r, earlier, Icon::Minus), (&later_r, later, Icon::Plus)] {
        let tint = if r.hovered() { theme::TEXT } else { theme::TEXT_DIM };
        if r.hovered() {
            painter.circle_filled(rect.center(), 12.0, Color32::from_white_alpha((24.0 * shown) as u8));
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        icon.image(tint.gamma_multiply(shown), 16.0).paint_at(ui, Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0)));
    }
    let text_color = if offset != 0.0 { theme::ACCENT_TEXT } else { theme::TEXT_DIM };
    painter.galley_with_override_text_color(middle.center() - galley.size() / 2.0, galley, text_color.gamma_multiply(shown));
    if offset != 0.0 && middle_r.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if earlier_r.clicked() {
        app.actions.push(Action::NudgeLyrics { video_id: video_id.to_owned(), by: -0.5 });
    } else if later_r.clicked() {
        app.actions.push(Action::NudgeLyrics { video_id: video_id.to_owned(), by: 0.5 });
    } else if middle_r.clicked() && offset != 0.0 {
        app.actions.push(Action::ResetLyrics(video_id.to_owned()));
    }
    if shown < 1.0 && pointed {
        theme::next_frame(ui.ctx());
    }
}

/// Three dots that fill in turn over a break, breathing.
fn dots(ui: &Ui, painter: &egui::Painter, rect: Rect, size: f32, progress: f64, playing: bool) {
    let t = ui.input(|i| i.time);
    let breath = 1.0 + 0.12 * ((t * 2.4).sin() as f32);
    let r = size * 0.13 * breath;
    let spacing = size * 0.48;
    let y = rect.center().y;
    for k in 0..3 {
        let lit = ((progress * 3.0 - f64::from(k)).clamp(0.0, 1.0)) as f32;
        let alpha = 0.28 + 0.72 * lit;
        painter.circle_filled(
            pos2(rect.left() + r + f32::from(k as u8) * spacing, y),
            r,
            Color32::from_white_alpha((alpha * 255.0) as u8),
        );
    }
    // The breathing is slow: twenty frames a second draw it as smoothly.
    if playing {
        theme::repaint_in(ui.ctx(), std::time::Duration::from_millis(50));
    }
}
