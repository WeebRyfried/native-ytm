//! Lyrics, the way Apple Music shows them: synced lines follow the song,
//! the line being sung bright and the rest dim, the view gliding to keep
//! it in place, three dots breathing through instrumental breaks. Scrolling
//! by hand pauses the following for a moment; a click on a line plays from
//! there. Lyrics without times are shown as text.

use egui::{Color32, CornerRadius, CursorIcon, Rect, RichText, Sense, Ui, pos2, vec2};

use crate::api::model::{LyricLine, Lyrics};
use crate::app::App;
use crate::model::{Action, Loadable};
use crate::theme;
use crate::ui::widgets;

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
                plain(ui, &lyrics, big);
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

fn plain(ui: &mut Ui, lyrics: &Lyrics, big: bool) {
    egui::ScrollArea::vertical()
        .id_salt("plain-lyrics")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(12.0);
            let size = if big { 24.0 } else { 17.0 };
            ui.spacing_mut().item_spacing.y = size * 0.5;
            ui.add(egui::Label::new(RichText::new(&lyrics.text).font(theme::semibold(size)).color(theme::TEXT)).wrap());
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

    let position = app.position() + LEAD;
    let playing = app.playing();
    let current = lines.iter().rposition(|l| l.time <= position);
    let dots_h = size * 0.9;

    // Where each line sits, from the top of the lyrics.
    // Laid out once per song, size and width, not every frame.
    type Laid = std::sync::Arc<Vec<Option<std::sync::Arc<egui::Galley>>>>;
    let scale = (ui.ctx().pixels_per_point() * 100.0).round() as i32;
    let layout_id = ui.id().with(("lyrics-layout", video_id, width.round() as i32, big, scale));
    let galleys: Laid = ui.data(|d| d.get_temp(layout_id)).unwrap_or_else(|| {
        let laid: Laid = std::sync::Arc::new(
            lines
                .iter()
                .map(|line| {
                    (!line.text.is_empty()).then(|| painter.layout(line.text.clone(), font.clone(), Color32::WHITE, width))
                })
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
            Some(galley) => galley.size().y,
            // A break takes room only while it is the one playing.
            None if current == Some(i) => dots_h,
            None => 0.0,
        };
        y += h + if h > 0.0 { gap } else { 0.0 };
    }
    let total = y;
    let height_of = |i: usize| -> f32 {
        match &galleys[i] {
            Some(galley) => galley.size().y,
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
            Some(galley) => {
                let hovered = pointer.is_some_and(|p| line_rect.expand2(vec2(pad, gap / 2.0)).contains(p));
                if hovered {
                    painter.rect_filled(line_rect.expand2(vec2(pad, gap / 3.0)), CornerRadius::same(8), Color32::from_white_alpha(12));
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    if response.clicked() {
                        clicked_line = Some(line.time);
                    }
                }
                let color = Color32::from_white_alpha((alpha * 255.0) as u8);
                painter.galley_with_override_text_color(line_rect.min, galley.clone(), color);
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
    if let Some(time) = clicked_line {
        app.actions.push(Action::Seek(time.max(0.0)));
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
