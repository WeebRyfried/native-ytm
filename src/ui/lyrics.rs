//! Lyrics, the way Apple Music shows them: synced lines follow the song,
//! the line being sung bright and the rest dim, the view gliding to keep
//! it in place, three dots breathing through instrumental breaks. Lyrics
//! timed by the word light up as each word is sung, a soft edge sweeping
//! through it; backing vocals sit small under their line, the duet's other
//! voice on the right, and a translation under it all when there is one.
//! Scrolling by hand pauses the following for a moment; a click on a line
//! plays from there. Lyrics without times are shown as text. A song's
//! lyrics from another source are a click away, in the corner.

use std::sync::Arc;

use egui::{Color32, CornerRadius, CursorIcon, Galley, Pos2, Rect, RichText, Sense, Ui, pos2, vec2};

use crate::api::model::{LyricLine, Lyrics, Word};
use crate::app::App;
use crate::model::{Action, Loadable};
use crate::readings::{self, Dictionary, Style};
use crate::theme::{self, Icon};
use crate::ui::widgets;

/// A line laid out, with the readings over it.
#[derive(Clone)]
struct LaidLine {
    galley: Arc<Galley>,
    readings: Vec<Ruby>,
}

/// A reading: its centre, from the line's top left, its text, and the
/// character its word starts at.
#[derive(Clone)]
struct Ruby {
    center: Pos2,
    text: Arc<Galley>,
    at: usize,
}

/// A word's times and the characters it covers in its line's text.
type Span = (f64, f64, usize, usize);

/// A synced line laid out: the words, the backing vocals and the
/// translation under them, each placed from the line's top left, and the
/// words' spans.
#[derive(Clone)]
struct Laid {
    main: LaidLine,
    timed: Vec<Span>,
    backing: Option<(Arc<Galley>, f32, Vec<Span>)>,
    translation: Option<(Arc<Galley>, f32)>,
    height: f32,
}

/// Synced lyrics laid out: each line (none for a break), the lines still
/// wanting a translation, and the other sources whose translations show
/// (and whether any shown was made by machine), to credit.
struct Layout {
    lines: Vec<Option<Laid>>,
    untranslated: Vec<String>,
    borrowed_from: Vec<String>,
    by_machine: bool,
}

/// Where a line's translation comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Said<'a> {
    Own,
    /// Another source's lyrics, by that source's name.
    Borrowed(&'a str),
    Machine,
}

/// How often lines still wanting a translation are sent for one, at most.
const TRANSLATE_EVERY: f64 = 15.0;

/// How long after a scroll by hand the view takes over again.
const HAND_SCROLL: f64 = 3.0;
/// Lines light up this much before their time, as a line is read just
/// before it is sung.
const LEAD: f64 = 0.2;
/// A silence this long between lines timed to their ends is a break, with
/// its dots.
const BREAK_AFTER: f64 = 6.0;

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
            let lyrics = Arc::clone(lyrics);
            // The source chosen for the song, or the first found.
            let all: Vec<&Lyrics> = std::iter::once(&*lyrics).chain(lyrics.others.iter()).collect();
            let chosen = app.personal.lyric_source(&video_id).and_then(|c| all.iter().position(|l| l.choice() == c)).unwrap_or(0);
            let area = ui.available_rect_before_wrap();
            let shown = all[chosen];
            if shown.lines.is_empty() {
                plain(app, ui, &video_id, shown, big);
            } else {
                synced(app, ui, &video_id, shown, &all, big);
            }
            if all.len() > 1 {
                let pointed = ui.ctx().pointer_hover_pos().is_some_and(|p| area.contains(p));
                sources(app, ui, area, &video_id, &all, chosen, pointed);
            }
        }
        Some(Loadable::Failed(_)) => message(ui, "The lyrics could not be looked up just now. Trying again in a minute"),
        Some(Loadable::Ready(None)) => message(ui, "Lyrics are not available for this song"),
        Some(Loadable::Loading) | None => widgets::spinner(ui),
    }
}

fn message(ui: &mut Ui, text: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| ui.label(RichText::new(text).color(theme::text_dim())));
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

/// Each row of a galley moved over to end at `width`.
fn set_right(galley: &mut Galley, width: f32) {
    for row in &mut galley.rows {
        row.pos.x += (width - row.rect().width()).max(0.0);
    }
    galley.rect = Rect::from_min_max(pos2(0.0, galley.rect.min.y), pos2(width.max(galley.rect.max.x), galley.rect.max.y));
    galley.mesh_bounds = galley.rect;
}

/// Text laid out to `width`, against the right edge when `right`.
fn lay_text(painter: &egui::Painter, text: &str, font: egui::FontId, width: f32, right: bool) -> Arc<Galley> {
    let galley = painter.layout(text.to_owned(), font, Color32::WHITE, width);
    if !right {
        return galley;
    }
    let mut galley = (*galley).clone();
    set_right(&mut galley, width);
    Arc::new(galley)
}

/// A line of lyrics laid out to `width`, with room above each row for the
/// readings of its words, and the readings placed; against the right edge
/// for a duet's other voice.
fn lay_line(painter: &egui::Painter, text: &str, font: &egui::FontId, width: f32, readings: Option<(&Dictionary, Style)>, right: bool) -> LaidLine {
    let base = painter.layout(text.to_owned(), font.clone(), Color32::WHITE, width);
    let rubies = readings.map(|(dictionary, style)| readings::readings(dictionary, text, style)).unwrap_or_default();
    if rubies.is_empty() {
        let galley = if right {
            let mut galley = (*base).clone();
            set_right(&mut galley, width);
            Arc::new(galley)
        } else {
            base
        };
        return LaidLine { galley, readings: Vec::new() };
    }
    let style = readings.map_or(Style::Romaji, |r| r.1);
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
    if right {
        set_right(&mut galley, width);
    }
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
        placed.push(Ruby { center: pos2((left + right) / 2.0, a.min.y - lift / 2.0), text: reading, at: ruby.start });
    }
    LaidLine { galley, readings: placed }
}

/// Paints a laid line at `at`, its readings a little fainter than it.
fn paint_line(painter: &egui::Painter, at: Pos2, line: &LaidLine, color: Color32) {
    painter.galley_with_override_text_color(at, line.galley.clone(), color);
    let faint = color.gamma_multiply(0.72);
    for ruby in &line.readings {
        painter.galley_with_override_text_color(at + ruby.center.to_vec2() - ruby.text.size() / 2.0, ruby.text.clone(), faint);
    }
}

/// The words' times and the characters each covers in their text; none
/// when they do not make up the text.
fn spans(words: &[Word], text: &str) -> Vec<Span> {
    let mut out = Vec::with_capacity(words.len());
    let mut at = 0;
    let mut joined = String::with_capacity(text.len());
    for word in words {
        let chars = word.text.chars().count();
        out.push((word.start, word.end, at, at + chars));
        at += chars;
        joined.push_str(&word.text);
    }
    if joined.trim_end() == text.trim_end() { out } else { Vec::new() }
}

/// How far through its text the singing is at `time`: a character index,
/// with how far through that character.
fn sung(spans: &[Span], time: f64) -> f32 {
    let mut upto = 0.0;
    for &(start, end, from, to) in spans {
        if time >= end {
            upto = to as f32;
        } else {
            if time > start {
                let t = ((time - start) / (end - start).max(0.01)) as f32;
                upto = from as f32 + (to - from) as f32 * t;
            }
            break;
        }
    }
    upto
}

/// Paints `galley` at `at` dim, and bright as far as `upto` (a character
/// index, see [`sung`]), the edge between them soft.
fn paint_sung(painter: &egui::Painter, at: Pos2, galley: &Arc<Galley>, upto: f32, dim: Color32, bright: Color32) {
    painter.galley_with_override_text_color(at, galley.clone(), dim);
    if upto <= 0.0 {
        return;
    }
    let total = galley.job.text.chars().count();
    let whole = upto.floor() as usize;
    let (edge_row, edge_x) = if whole >= total {
        (usize::MAX, 0.0)
    } else {
        let a = galley.pos_from_cursor(egui::text::CCursor::new(whole));
        let row = galley.rows.iter().position(|r| a.center().y >= r.rect().min.y - 0.5 && a.center().y <= r.rect().max.y + 0.5).unwrap_or(0);
        let b = galley.pos_from_cursor(egui::text::CCursor::new(whole + 1));
        let next_x = if (a.min.y - b.min.y).abs() < 1.0 { b.min.x } else { galley.rows[row].rect().max.x };
        (row, a.min.x + (next_x - a.min.x) * upto.fract())
    };
    let clip = painter.clip_rect();
    for (i, row) in galley.rows.iter().enumerate() {
        if i > edge_row {
            break;
        }
        let rect = row.rect().translate(at.to_vec2()).expand2(vec2(4.0, 2.0));
        if i < edge_row {
            painter.with_clip_rect(rect.intersect(clip)).galley_with_override_text_color(at, galley.clone(), bright);
            continue;
        }
        let x = at.x + edge_x;
        let filled = Rect::from_min_max(rect.min, pos2(x, rect.max.y));
        painter.with_clip_rect(filled.intersect(clip)).galley_with_override_text_color(at, galley.clone(), bright);
        // The soft edge: slivers fading from bright to dim.
        const SLIVERS: usize = 5;
        const WIDE: f32 = 2.5;
        for k in 0..SLIVERS {
            let t = 1.0 - (k as f32 + 1.0) / (SLIVERS as f32 + 1.0);
            let sliver = Rect::from_min_max(pos2(x + WIDE * k as f32, rect.min.y), pos2(x + WIDE * (k as f32 + 1.0), rect.max.y));
            painter.with_clip_rect(sliver.intersect(clip)).galley_with_override_text_color(at, galley.clone(), bright.gamma_multiply(t));
        }
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
                let source = lyrics.choice();
                let id = ui.id().with(("plain-readings", video_id, &source, width.round() as i32, readings.1));
                let laid: Arc<Vec<LaidLine>> = ui.data(|d| d.get_temp(id)).unwrap_or_else(|| {
                    let laid = Arc::new(texts.iter().map(|t| lay_line(&painter, t, &font, width, Some(readings), false)).collect());
                    ui.data_mut(|d| d.insert_temp(id, Arc::clone(&laid)));
                    laid
                });
                ui.spacing_mut().item_spacing.y = size * 0.3;
                for line in laid.iter() {
                    let (rect, _) = ui.allocate_exact_size(vec2(width, line.galley.size().y.max(size * 0.6)), Sense::hover());
                    if ui.is_rect_visible(rect) {
                        paint_line(ui.painter(), rect.min, line, theme::text());
                    }
                }
            } else {
                ui.add(egui::Label::new(RichText::new(&lyrics.text).font(theme::semibold(size)).color(theme::text())).wrap());
            }
            if let Some(source) = footer(lyrics) {
                ui.add_space(20.0);
                ui.add(egui::Label::new(RichText::new(source).color(theme::text_faint()).font(theme::regular(12.5))).wrap());
            }
            ui.add_space(48.0);
        });
}

/// Where the lyrics are from, and who wrote the song.
fn footer(lyrics: &Lyrics) -> Option<String> {
    let mut said = lyrics.source.clone().unwrap_or_default();
    if !lyrics.songwriters.is_empty() {
        if !said.is_empty() {
            said.push('\n');
        }
        said.push_str(&format!("Written by {}", lyrics.songwriters.join(", ")));
    }
    (!said.is_empty()).then_some(said)
}

/// A line's translation: its own; else another source's line sung at the
/// same moment; else one made by machine. `others` are the other sources'
/// translated lines: when each is sung, its translation, and the source.
fn translation_of<'a>(line: &LyricLine, others: &[(f64, &str, &'a str)], machine: &std::collections::BTreeMap<String, String>) -> Option<(String, Said<'a>)> {
    if let Some(own) = line.translation.as_ref().filter(|t| !t.is_empty()) {
        return Some((own.clone(), Said::Own));
    }
    let near = others
        .iter()
        .filter(|(time, ..)| (time - line.time).abs() <= 1.0)
        .min_by(|a, b| (a.0 - line.time).abs().total_cmp(&(b.0 - line.time).abs()));
    if let Some((_, said, source)) = near {
        return Some(((*said).to_owned(), Said::Borrowed(source)));
    }
    machine.get(line.text.trim()).filter(|t| !t.is_empty()).map(|t| (t.clone(), Said::Machine))
}

/// `all`: the song's lyrics from every source (the first the one found
/// first, which keeps the translations made by machine).
fn synced(app: &mut App, ui: &mut Ui, video_id: &str, lyrics: &Lyrics, all: &[&Lyrics], big: bool) {
    let area = ui.available_rect_before_wrap();
    let (rect, response) = ui.allocate_exact_size(area.size(), Sense::click());
    let painter = ui.painter_at(rect);
    let size = if big { 32.0 } else { 22.0 };
    let font = theme::bold(size);
    let gap = if big { 26.0 } else { 16.0 };
    let pad = if big { 10.0 } else { 6.0 };
    let width = (rect.width() - pad * 2.0).max(60.0);
    let translations = app.settings.lyric_translations;

    // A quiet start becomes a break of its own, with its dots; and so does
    // a long silence between lines that say when they end.
    let mut lines: Vec<LyricLine> = Vec::with_capacity(lyrics.lines.len() + 4);
    if lyrics.lines.first().is_some_and(|l| l.time > 3.0) {
        lines.push(LyricLine::default());
    }
    for (i, line) in lyrics.lines.iter().enumerate() {
        lines.push(line.clone());
        if let (Some(end), Some(next)) = (line.end, lyrics.lines.get(i + 1))
            && next.time - end > BREAK_AFTER
            && !line.text.is_empty()
        {
            lines.push(LyricLine { time: end, ..LyricLine::default() });
        }
    }

    // Lyrics a little early or late for this recording: moved, per song.
    let offset = app.personal.lyric_offset(video_id);
    let heard = app.position() - offset;
    let position = heard + LEAD;
    let playing = app.playing();
    let current = lines.iter().rposition(|l| l.time <= position);
    let dots_h = size * 0.9;

    // Where each line sits, from the top of the lyrics.
    // Laid out once per song, source, size, width, readings and
    // translations, not every frame.
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    let readings = readings_for(app, ui, big, &texts);
    let scale = (ui.ctx().pixels_per_point() * 100.0).round() as i32;
    let machine = &all[0].translated;
    let source = (lyrics.choice(), lines.len(), lyrics.text.len(), machine.lines.len());
    let layout_id = ui.id().with(("lyrics-layout", video_id, source, width.round() as i32, big, scale, readings.map(|r| r.1), translations));
    let layout: Arc<Layout> = ui.data(|d| d.get_temp(layout_id)).unwrap_or_else(|| {
        let backing_font = theme::semibold(size * 0.62);
        let translation_font = theme::medium(size * 0.5);
        // The other sources' translations, to borrow by the moment sung.
        let others: Vec<(f64, &str, &str)> = if translations {
            all.iter()
                .filter(|l| !std::ptr::eq(**l, lyrics))
                .flat_map(|l| {
                    let source = l.provider.as_deref().unwrap_or("another source");
                    l.lines.iter().filter_map(move |line| line.translation.as_deref().filter(|t| !t.is_empty()).map(|t| (line.time, t, source)))
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut untranslated = Vec::new();
        let mut borrowed_from: Vec<String> = Vec::new();
        let mut by_machine = false;
        let laid: Vec<Option<Laid>> = lines
            .iter()
            .map(|line| {
                if line.text.is_empty() && line.backing.is_empty() {
                    return None;
                }
                let right = line.second_voice;
                let main = lay_line(&painter, &line.text, &font, width, readings, right);
                let timed = spans(&line.words, &line.text);
                let mut height = if line.text.is_empty() { 0.0 } else { main.galley.size().y };
                let backing = (!line.backing.is_empty()).then(|| {
                    let text: String = line.backing.iter().map(|w| w.text.as_str()).collect();
                    let galley = lay_text(&painter, &text, backing_font.clone(), width, right);
                    let y = height + if height > 0.0 { size * 0.12 } else { 0.0 };
                    height = y + galley.size().y;
                    let timed = spans(&line.backing, &text);
                    (galley, y, timed)
                });
                let said = if translations { translation_of(line, &others, &machine.lines) } else { None };
                if translations && said.is_none() && !line.text.trim().is_empty() && !machine.lines.contains_key(line.text.trim()) {
                    untranslated.push(line.text.trim().to_owned());
                }
                let translation = said.map(|(text, from)| {
                    match from {
                        Said::Own => {}
                        Said::Borrowed(source) => {
                            if !borrowed_from.iter().any(|s| s == source) {
                                borrowed_from.push(source.to_owned());
                            }
                        }
                        Said::Machine => by_machine = true,
                    }
                    let galley = lay_text(&painter, &text, translation_font.clone(), width, right);
                    let y = height + size * 0.18;
                    height = y + galley.size().y;
                    (galley, y)
                });
                Some(Laid { main, timed, backing, translation, height })
            })
            .collect();
        // Only lyrics that may be in another language, and not already
        // found to be in English.
        untranslated.sort();
        untranslated.dedup();
        if machine.from.as_deref() == Some(crate::api::translate::INTO) || !crate::api::translate::worth_asking(&untranslated) {
            untranslated.clear();
        }
        let layout = Arc::new(Layout { lines: laid, untranslated, borrowed_from, by_machine });
        ui.data_mut(|d| d.insert_temp(layout_id, layout.clone()));
        layout
    });
    if !layout.untranslated.is_empty() {
        let id = ui.id().with(("lyrics-translate", video_id));
        let now = ui.input(|i| i.time);
        let last: f64 = ui.data(|d| d.get_temp(id)).unwrap_or(f64::NEG_INFINITY);
        if now - last > TRANSLATE_EVERY {
            ui.data_mut(|d| d.insert_temp(id, now));
            app.actions.push(Action::TranslateLyrics { video_id: video_id.to_owned(), lines: layout.untranslated.clone() });
        }
    }
    let galleys = &layout.lines;
    let height_of = |i: usize| -> f32 {
        match &galleys[i] {
            Some(laid) => laid.height,
            // A break takes room only while it is the one playing.
            None if current == Some(i) => dots_h,
            None => 0.0,
        }
    };
    let mut tops = Vec::with_capacity(lines.len());
    let mut y = 0.0f32;
    for i in 0..lines.len() {
        tops.push(y);
        let h = height_of(i);
        y += h + if h > 0.0 { gap } else { 0.0 };
    }
    let total = y;

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
    let white = |alpha: f32| Color32::from_white_alpha((alpha.clamp(0.0, 1.0) * 255.0) as u8);
    let mut clicked_line = None;
    let mut sweeping = false;
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
        let reach = 0.25 + 0.75 * edge;
        match &galleys[i] {
            Some(laid) => {
                let hovered = pointer.is_some_and(|p| line_rect.expand2(vec2(pad, gap / 2.0)).contains(p));
                if hovered {
                    painter.rect_filled(line_rect.expand2(vec2(pad, gap / 3.0)), CornerRadius::same(8), Color32::from_white_alpha(12));
                    crate::ui::widgets::hover_glow(ui, line_rect.expand2(vec2(pad, gap / 3.0)), 8.0, 1.0);
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    if response.clicked() {
                        clicked_line = Some(line.time);
                    }
                }
                if !laid.timed.is_empty() && is_current && !line.text.is_empty() {
                    // Being sung, timed by the word: lit as far as the singing
                    // has come, the rest a little brighter than the lines
                    // around it.
                    let upto = sung(&laid.timed, heard);
                    let dim = white((0.32 + 0.14 * lit) * reach);
                    let bright = white(reach);
                    paint_sung(&painter, line_rect.min, &laid.main.galley, upto, dim, bright);
                    for ruby in &laid.main.readings {
                        let color = if upto > ruby.at as f32 { bright } else { dim };
                        painter.galley_with_override_text_color(
                            line_rect.min + ruby.center.to_vec2() - ruby.text.size() / 2.0,
                            ruby.text.clone(),
                            color.gamma_multiply(0.72),
                        );
                    }
                    let end = line.end.unwrap_or_else(|| line.words.last().map_or(line.time, |w| w.end));
                    sweeping |= heard < end + 0.1;
                } else if !line.text.is_empty() {
                    paint_line(&painter, line_rect.min, &laid.main, white(alpha * reach));
                }
                if let Some((galley, y, timed)) = &laid.backing {
                    let at = line_rect.min + vec2(0.0, *y);
                    // Swept as sung only when the line is timed by the word.
                    if is_current && !timed.is_empty() && !laid.timed.is_empty() {
                        let upto = sung(timed, heard);
                        paint_sung(&painter, at, galley, upto, white(0.26 * reach), white(0.78 * reach));
                        sweeping |= timed.last().is_some_and(|t| heard < t.1 + 0.1);
                    } else {
                        painter.galley_with_override_text_color(at, galley.clone(), white(alpha * 0.7 * reach));
                    }
                }
                if let Some((galley, y)) = &laid.translation {
                    painter.galley_with_override_text_color(line_rect.min + vec2(0.0, *y), galley.clone(), white(alpha * 0.62 * reach));
                }
            }
            None => {
                let next = lines.get(i + 1).map_or(line.time + 4.0, |l| l.time);
                dots(ui, &painter, line_rect, size, (position - line.time) / (next - line.time).max(0.5), playing);
            }
        }
    }
    let footer = footer(lyrics).map(|mut said| {
        if !layout.borrowed_from.is_empty() {
            said.push_str(&format!("\nTranslations from {}", layout.borrowed_from.join(", ")));
        }
        if layout.by_machine {
            said.push_str("\nTranslations from Unison, Better Lyrics' community (made by machine)");
        }
        said
    });
    if let Some(source) = footer {
        let top = rect.top() + total - view.offset + gap;
        if top < rect.bottom() && top > rect.top() - 40.0 {
            let galley = painter.layout(source, theme::regular(12.5), theme::text_faint(), width);
            painter.galley(pos2(rect.left() + pad, top), galley, theme::text_faint());
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
    if playing && sweeping {
        // A word being sung: every frame, at no more than 60 a second.
        theme::next_frame(ui.ctx());
    } else if playing
        && let Some(next) = lines.iter().find(|l| l.time > position)
    {
        // Wake for the next line rather than every frame.
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
    let galley = painter.layout_no_wrap(label, theme::medium(12.5), theme::text());
    let (h, button) = (30.0, 32.0);
    let w = button * 2.0 + galley.size().x + 16.0;
    let bar = Rect::from_min_size(pos2(area.right() - w - 10.0, area.bottom() - h - 10.0), vec2(w, h));
    painter.rect_filled(bar, CornerRadius::same(15), theme::popup().gamma_multiply(shown));
    painter.rect_stroke(bar, CornerRadius::same(15), egui::Stroke::new(1.0, theme::divider().gamma_multiply(shown)), egui::StrokeKind::Inside);
    let earlier = Rect::from_min_size(bar.min, vec2(button, h));
    let later = Rect::from_min_size(pos2(bar.right() - button, bar.top()), vec2(button, h));
    let middle = Rect::from_min_max(pos2(earlier.right(), bar.top()), pos2(later.left(), bar.bottom()));
    let earlier_r = ui.interact(earlier, id.with("earlier"), Sense::click()).on_hover_text("Lines come half a second earlier");
    let later_r = ui.interact(later, id.with("later"), Sense::click()).on_hover_text("Lines come half a second later");
    let middle_r = ui.interact(middle, id.with("reset"), Sense::click());
    let middle_r = if offset != 0.0 { middle_r.on_hover_text("Back to the lyrics' own timing") } else { middle_r.on_hover_text("Lyrics early or late? Move them here; it is kept for this song") };
    for (r, rect, icon) in [(&earlier_r, earlier, Icon::Minus), (&later_r, later, Icon::Plus)] {
        let tint = if r.hovered() { theme::text() } else { theme::text_dim() };
        if r.hovered() {
            painter.circle_filled(rect.center(), 12.0, Color32::from_white_alpha((24.0 * shown) as u8));
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        icon.image(tint.gamma_multiply(shown), 16.0).paint_at(ui, Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0)));
    }
    let text_color = if offset != 0.0 { theme::accent_text() } else { theme::text_dim() };
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

/// In the other corner: whose lyrics these are, and a menu of the other
/// sources that had the song, to switch to (kept for the song).
fn sources(app: &mut App, ui: &mut Ui, area: Rect, video_id: &str, all: &[&Lyrics], chosen: usize, pointed: bool) {
    let id = ui.id().with(("lyrics-sources", video_id));
    let painter = ui.painter().clone();
    let label = all[chosen].provider.clone().unwrap_or_else(|| "Lyrics".into());
    let galley = painter.layout_no_wrap(label, theme::medium(12.5), theme::text());
    let h = 30.0;
    let w = 14.0 + galley.size().x + 8.0 + 14.0 + 12.0;
    let pill = Rect::from_min_size(pos2(area.left() + 10.0, area.bottom() - h - 10.0), vec2(w, h));
    let response = ui.interact(pill, id, Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    let shown = ui.ctx().animate_bool_with_time(id.with("shown"), pointed || open, 0.2);
    if shown > 0.01 {
        let fill = if response.hovered() { theme::surface_hover() } else { theme::popup() };
        painter.rect_filled(pill, CornerRadius::same(15), fill.gamma_multiply(shown));
        painter.rect_stroke(pill, CornerRadius::same(15), egui::Stroke::new(1.0, theme::divider().gamma_multiply(shown)), egui::StrokeKind::Inside);
        painter.galley_with_override_text_color(pos2(pill.left() + 14.0, pill.center().y - galley.size().y / 2.0), galley, theme::text_dim().gamma_multiply(shown));
        Icon::ChevronDown
            .image(theme::text_dim().gamma_multiply(shown), 14.0)
            .paint_at(ui, Rect::from_center_size(pos2(pill.right() - 19.0, pill.center().y), egui::Vec2::splat(14.0)));
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if shown < 1.0 {
            theme::next_frame(ui.ctx());
        }
    }
    let response = response.on_hover_text("Lyrics from another source");
    egui::Popup::menu(&response).align(egui::RectAlign::TOP_START).show(|ui| {
        ui.set_min_width(220.0);
        for (i, lyrics) in all.iter().enumerate() {
            let name = lyrics.provider.as_deref().unwrap_or("Lyrics");
            let text = format!("{name}  ·  {}", lyrics.timing());
            if ui.add(egui::Button::new(text).selected(i == chosen)).clicked() {
                let choice = (i != 0).then(|| lyrics.choice());
                app.actions.push(Action::ChooseLyrics { video_id: video_id.to_owned(), choice });
                ui.close();
            }
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start: f64, end: f64, text: &str) -> Word {
        Word { start, end, text: text.into() }
    }

    #[test]
    fn a_translation_from_the_line_another_source_or_a_machine() {
        let line = |time: f64, text: &str, own: Option<&str>| LyricLine { time, text: text.into(), translation: own.map(str::to_owned), ..LyricLine::default() };
        let mut machine = std::collections::BTreeMap::new();
        machine.insert("はい".to_owned(), "Yes".to_owned());
        machine.insert("いいえ".to_owned(), String::new());
        let others = [(10.4, "Hello there", "BiniLyrics"), (30.0, "Far off", "BiniLyrics")];
        // Its own first.
        assert_eq!(translation_of(&line(10.0, "こんにちは", Some("Hi")), &others, &machine), Some(("Hi".into(), Said::Own)));
        // Else another source's line sung at the same moment.
        assert_eq!(translation_of(&line(10.0, "こんにちは", None), &others, &machine), Some(("Hello there".into(), Said::Borrowed("BiniLyrics"))));
        // Else the machine's, by the line's text; an empty one is none.
        assert_eq!(translation_of(&line(20.0, "はい", None), &others, &machine), Some(("Yes".into(), Said::Machine)));
        assert_eq!(translation_of(&line(20.0, "いいえ", None), &others, &machine), None);
        assert_eq!(translation_of(&line(50.0, "さようなら", None), &others, &machine), None);
    }

    #[test]
    fn singing_moves_through_the_words() {
        let words = [word(1.0, 2.0, "ab "), word(2.0, 4.0, "cd")];
        let spans = spans(&words, "ab cd");
        assert_eq!(spans, [(1.0, 2.0, 0, 3), (2.0, 4.0, 3, 5)]);
        assert_eq!(sung(&spans, 0.5), 0.0);
        assert!((sung(&spans, 1.5) - 1.5).abs() < 1e-5);
        assert!((sung(&spans, 3.0) - 4.0).abs() < 1e-5);
        assert_eq!(sung(&spans, 9.0), 5.0);
        // Words that do not make up the text are not followed.
        assert!(super::spans(&words, "something else").is_empty());
    }
}
