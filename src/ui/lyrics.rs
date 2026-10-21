//! Lyrics, the way Apple Music shows them: synced lines follow the song,
//! the lines being sung bright (more than one when singers overlap) and
//! the rest dim, the view gliding to keep them in place, three dots
//! breathing through instrumental breaks. Lyrics
//! timed by the word light up as each word is sung, a soft edge sweeping
//! through it; backing vocals sit small under their line, and a translation
//! under it all when there is one. When more than one singer is named, the
//! first sings on the left, the others on the right and all of them
//! together in the middle, each in a colour of their own.
//! Scrolling by hand pauses the following for a moment; a click on a line
//! plays from there. Lyrics without times are shown as text. A song's
//! lyrics from another source are a click away, in the corner.

use std::sync::Arc;

use egui::{Color32, CornerRadius, CursorIcon, Galley, Pos2, Rect, RichText, Sense, Ui, pos2, vec2};

use crate::api::model::{LyricLine, Lyrics, Voice, Word};
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
/// translation under them, each placed from the line's top left, the
/// words' spans, and who sings it (for its colour).
#[derive(Clone)]
struct Laid {
    voice: Voice,
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
/// Once sung, a line (or its backing vocals) dims for as long as this many
/// of its words took, on average: a quick line goes out quickly, a slow
/// one more slowly. No quicker than the first, no slower than the second.
const FADE_WORDS: f64 = 1.25;
const FADE_MIN: f32 = 0.2;
const FADE_MAX: f32 = 1.0;
/// How long a line takes to dim (or brighten) toward the view's edges as
/// the view moves on.
const EDGE_EASE: f32 = 0.5;
/// How wide the soft edge of the singing is, in points.
const SOFT: f32 = 12.5;

#[derive(Clone, Copy, Default)]
struct View {
    /// Where the view is, and the glide it is on: from, to, since when.
    offset: f32,
    from: f32,
    to: f32,
    glide_start: f64,
    hand_until: f64,
    by_hand: bool,
    placed: bool,
}

/// When each line is over: once it has ended and the next has started
/// (the last never is). A line is sung from its time until then, so lines
/// sung over one another are sung, and lit, together.
fn overs(lines: &[LyricLine]) -> Vec<f64> {
    (0..lines.len())
        .map(|i| match lines.get(i + 1) {
            Some(next) => lines[i].end.map_or(next.time, |end| end.max(next.time)),
            None => f64::INFINITY,
        })
        .collect()
}

/// How far a line is into the view's edges' fade (0 at the very edge, 1
/// clear of it), eased from one place to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Edge {
    from: f32,
    to: f32,
    since: f64,
}

impl Edge {
    fn at(&self, now: f64) -> f32 {
        let t = ((now - self.since) / f64::from(EDGE_EASE)) as f32;
        self.from + (self.to - self.from) * theme::ease_even(t)
    }

    /// Where a line is in the edges' fade now, heading for `target`: from
    /// wherever it was, over [`EDGE_EASE`] at an even pace, so the view
    /// moving on never dims a line all at once; at once when `at_once` (the
    /// view scrolled by hand, which the fade follows as the hand moves).
    /// Also whether it is still on its way.
    fn ease(slot: &mut Option<Edge>, target: f32, now: f64, at_once: bool) -> (f32, bool) {
        let edge = slot.get_or_insert(Edge { from: target, to: target, since: now });
        if at_once {
            *edge = Edge { from: target, to: target, since: now };
        } else if (edge.to - target).abs() > 1e-3 {
            *edge = Edge { from: edge.at(now), to: target, since: now };
        }
        let value = edge.at(now);
        (value, (value - edge.to).abs() > 1e-4)
    }
}

/// A song's lines as the view goes through them: with a break of their own
/// where it is quiet a while (see [`timeline`]), when each is over, and
/// whether any is Japanese (for readings). Made once per song and source.
struct Timeline {
    lines: Vec<LyricLine>,
    over: Vec<f64>,
    japanese: bool,
}

/// `lyrics` as a [`Timeline`]: a quiet start becomes a break with its dots,
/// and so does a long silence between lines that say when they end.
fn timeline(ui: &Ui, video_id: &str, lyrics: &Lyrics) -> Arc<Timeline> {
    use std::hash::{Hash as _, Hasher as _};
    let mut lines_now = std::collections::hash_map::DefaultHasher::new();
    for line in &lyrics.lines {
        (line.time.to_bits(), line.text.len(), line.words.len(), line.backing.len()).hash(&mut lines_now);
    }
    let key = egui::Id::new(("lyrics-timeline", video_id, lines_now.finish()));
    cached(ui, ui.id().with("lyrics-timeline"), key, || {
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
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        let japanese = readings::is_japanese(&texts);
        let over = overs(&lines);
        Arc::new(Timeline { lines, over, japanese })
    })
}

/// What `make` makes for `key`, kept in `slot` (one a view) until the key
/// changes: so only the song showing is kept, not every song played (nor
/// the lyrics at every width the panel was dragged through).
fn cached<T: Clone + Send + Sync + 'static>(ui: &Ui, slot: egui::Id, key: egui::Id, make: impl FnOnce() -> T) -> T {
    if let Some((kept, value)) = ui.data(|d| d.get_temp::<(egui::Id, T)>(slot))
        && kept == key
    {
        return value;
    }
    let value = make();
    ui.data_mut(|d| d.insert_temp(slot, (key, value.clone())));
    value
}

/// How lit a line (or its backing vocals) is at `position` (0 to 1):
/// coming in over [`LIGHT`] from its time, and once `over`, going out at an
/// even pace, eased at its ends, over `out` seconds. By the song's own time,
/// so it holds still while the song is paused.
fn light(time: f64, over: f64, position: f64, out: f32) -> f32 {
    let on = if position >= time { theme::ease_out(((position - time) / f64::from(LIGHT)) as f32) } else { 0.0 };
    let off = if position >= over { theme::ease_even(((position - over) / f64::from(out.max(0.01))) as f32) } else { 0.0 };
    (on * (1.0 - off)).clamp(0.0, 1.0)
}

/// How long something sung takes to dim once over, by how fast it was
/// sung: [`FADE_WORDS`] of its words' average length. Words timed one by
/// one say it; without them, the line's length over its words (its
/// characters, for Japanese, Chinese and Korean written without spaces);
/// without an end either, [`LIGHT`].
fn fade_out(words: &[Word], span: Option<(f64, f64, &str)>) -> f32 {
    let (sum, count) = words
        .iter()
        .filter(|w| w.end > w.start && !w.text.trim().is_empty())
        .fold((0.0, 0usize), |(sum, count), w| (sum + (w.end - w.start), count + 1));
    let pace = if count == 0 {
        let Some((start, end, text)) = span.filter(|(start, end, _)| end > start) else {
            return LIGHT;
        };
        let spaced = text.split_whitespace().count();
        let units = if spaced <= 1 { text.chars().filter(|c| !c.is_whitespace()).count() } else { spaced };
        if units == 0 {
            return LIGHT;
        }
        (end - start) / units as f64
    } else {
        sum / count as f64
    };
    ((pace * FADE_WORDS) as f32).clamp(FADE_MIN, FADE_MAX)
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
fn readings_for(app: &App, ui: &Ui, big: bool, japanese: impl FnOnce() -> bool) -> Option<(&'static Dictionary, Style)> {
    if !big || !app.settings.lyric_readings || !japanese() {
        return None;
    }
    let ctx = ui.ctx().clone();
    readings::dictionary(move || ctx.request_repaint()).map(|dictionary| (dictionary, app.settings.reading_style))
}

/// Each row of a galley moved across `width`: none of the way for the
/// left (0), half for the middle, all of it for the right (1).
fn place(galley: &mut Galley, width: f32, across: f32) {
    for row in &mut galley.rows {
        row.pos.x += ((width - row.rect().width()) * across).max(0.0);
    }
    galley.rect = Rect::from_min_max(pos2(0.0, galley.rect.min.y), pos2(width.max(galley.rect.max.x), galley.rect.max.y));
    galley.mesh_bounds = galley.rect;
}

/// Text laid out to `width`, its rows `across` it (see [`place`]).
fn lay_text(painter: &egui::Painter, text: &str, font: egui::FontId, width: f32, across: f32) -> Arc<Galley> {
    let galley = painter.layout(text.to_owned(), font, Color32::WHITE, width);
    if across <= 0.0 {
        return galley;
    }
    let mut galley = (*galley).clone();
    place(&mut galley, width, across);
    Arc::new(galley)
}

/// Where a singer's lines sit across the lyrics, in a song with more than
/// one: the first on the left, the others on the right, all of them
/// together in the middle.
fn across(voice: Voice) -> f32 {
    match voice {
        Voice::First => 0.0,
        Voice::Second | Voice::Third => 1.0,
        Voice::Together => 0.5,
    }
}

/// More than one singer named, so each is set apart.
fn duet(lines: &[LyricLine]) -> bool {
    let mut voices = lines.iter().filter(|l| !l.text.trim().is_empty() || !l.backing.is_empty()).map(LyricLine::singer);
    voices.next().is_some_and(|first| voices.any(|v| v != first))
}

/// The colour a singer's lines are drawn in, in a song with more than one:
/// the first in white, the second in the theme's colour and a third in its
/// glow (both made light enough to read), all of them together in between
/// the first two.
fn voice_colour(voice: Voice) -> Color32 {
    let toward = |from: Color32, to: Color32, t: f32| {
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Color32::from_rgb(mix(from.r(), to.r()), mix(from.g(), to.g()), mix(from.b(), to.b()))
    };
    let second = toward(theme::accent(), Color32::WHITE, 0.4);
    match voice {
        Voice::First => Color32::WHITE,
        Voice::Second => second,
        Voice::Third => toward(theme::glow(), Color32::WHITE, 0.4),
        Voice::Together => toward(Color32::WHITE, second, 0.5),
    }
}

/// A line of lyrics laid out to `width`, with room above each row for the
/// readings of its words, and the readings placed; its rows `across` the
/// width (see [`place`]).
fn lay_line(painter: &egui::Painter, text: &str, font: &egui::FontId, width: f32, readings: Option<(&Dictionary, Style)>, across: f32) -> LaidLine {
    let base = painter.layout(text.to_owned(), font.clone(), Color32::WHITE, width);
    let rubies = readings.map(|(dictionary, style)| readings::readings(dictionary, text, style)).unwrap_or_default();
    if rubies.is_empty() {
        let galley = if across > 0.0 {
            let mut galley = (*base).clone();
            place(&mut galley, width, across);
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
    if across > 0.0 {
        place(&mut galley, width, across);
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
/// index, see [`sung`]), the edge between them soft. In one go: each glyph
/// takes its colour from where it is against the edge, one the edge
/// crosses cut where its colour changes, so the line is one mesh, each part
/// of it painted once (see-through colours painted over one another would
/// add up, and pieces clipped apart cost a draw each).
fn paint_sung(painter: &egui::Painter, at: Pos2, galley: &Arc<Galley>, upto: f32, dim: Color32, bright: Color32) {
    if upto <= 0.0 {
        painter.galley_with_override_text_color(at, galley.clone(), dim);
        return;
    }
    let total = galley.job.text.chars().count();
    let whole = upto.floor() as usize;
    if whole >= total {
        painter.galley_with_override_text_color(at, galley.clone(), bright);
        return;
    }
    let a = galley.pos_from_cursor(egui::text::CCursor::new(whole));
    let edge_row = galley.rows.iter().position(|r| a.center().y >= r.rect().min.y - 0.5 && a.center().y <= r.rect().max.y + 0.5).unwrap_or(0);
    let b = galley.pos_from_cursor(egui::text::CCursor::new(whole + 1));
    let next_x = if (a.min.y - b.min.y).abs() < 1.0 { b.min.x } else { galley.rows[edge_row].rect().max.x };
    let edge = a.min.x + (next_x - a.min.x) * upto.fract();
    let mut swept = (**galley).clone();
    let (mut vertices, mut indices) = (0, 0);
    for (i, placed) in swept.rows.iter_mut().enumerate() {
        let x = placed.pos.x;
        let visuals = &mut Arc::make_mut(&mut placed.row).visuals;
        match i.cmp(&edge_row) {
            std::cmp::Ordering::Less => colour_glyphs(visuals, bright),
            std::cmp::Ordering::Greater => colour_glyphs(visuals, dim),
            std::cmp::Ordering::Equal => cut_at_edge(visuals, edge - x, dim, bright),
        }
        vertices += visuals.mesh.vertices.len();
        indices += visuals.mesh.indices.len();
    }
    swept.num_vertices = vertices;
    swept.num_indices = indices;
    painter.add(egui::epaint::TextShape::new(at, Arc::new(swept), dim));
}

/// A row's glyphs all in `colour`.
fn colour_glyphs(visuals: &mut egui::epaint::text::RowVisuals, colour: Color32) {
    for vertex in &mut visuals.mesh.vertices[visuals.glyph_vertex_range.clone()] {
        vertex.color = colour;
    }
}

/// The glyphs of the row being sung, `edge` across it: bright up to there,
/// dim from [`SOFT`] after it, going from one to the other between; a glyph
/// quad (as egui lays them out, four corners each) cut where that begins
/// and ends, so its colours follow it exactly.
fn cut_at_edge(visuals: &mut egui::epaint::text::RowVisuals, edge: f32, dim: Color32, bright: Color32) {
    use egui::epaint::Vertex;
    let colour = |x: f32| mix(bright, dim, (x - edge) / SOFT);
    let mesh = &mut visuals.mesh;
    let quads = mesh.vertices.len() / 4;
    if visuals.glyph_vertex_range != (0..mesh.vertices.len()) || mesh.vertices.len() % 4 != 0 || mesh.indices.len() != quads * 6 {
        // More than glyphs (a background, an underline): coloured by
        // where each corner is, uncut.
        for vertex in &mut mesh.vertices[visuals.glyph_vertex_range.clone()] {
            vertex.color = colour(vertex.pos.x);
        }
        return;
    }
    let corners = std::mem::take(&mut mesh.vertices);
    mesh.indices.clear();
    let between = |a: Vertex, b: Vertex, t: f32| {
        let pos = a.pos + (b.pos - a.pos) * t;
        Vertex { pos, uv: a.uv + (b.uv - a.uv) * t, color: colour(pos.x) }
    };
    for quad in corners.chunks_exact(4) {
        let (lt, rt, lb, rb) = (quad[0], quad[1], quad[2], quad[3]);
        // Where along the glyph it is cut: where the soft edge begins and
        // ends, if it does within it.
        let wide = rt.pos.x - lt.pos.x;
        let mut cuts = [0.0f32, 1.0, 1.0, 1.0];
        let mut n = 1;
        if wide > 0.0 {
            for x in [edge, edge + SOFT] {
                let t = (x - lt.pos.x) / wide;
                if t > 0.0 && t < 1.0 {
                    cuts[n] = t;
                    n += 1;
                }
            }
        }
        cuts[n] = 1.0;
        for piece in cuts[..=n].windows(2) {
            let first = mesh.vertices.len() as u32;
            mesh.vertices.extend([between(lt, rt, piece[0]), between(lt, rt, piece[1]), between(lb, rb, piece[0]), between(lb, rb, piece[1])]);
            mesh.indices.extend([first, first + 1, first + 2, first + 2, first + 1, first + 3]);
        }
    }
    visuals.glyph_vertex_range = 0..mesh.vertices.len();
}

/// When a sweep through `spans` next needs a frame, at `time`, in seconds
/// from then: while a word is sung, once its edge has moved about a pixel
/// (`per_char` points a character), however slow; between words, when the
/// next begins; never, once all are sung.
fn sweep_wait(spans: &[Span], time: f64, per_char: f32) -> f64 {
    for &(start, end, from, to) in spans {
        if time < start {
            return start - time;
        }
        if time < end {
            let speed = (to - from) as f64 / (end - start).max(0.01) * f64::from(per_char);
            return (1.0 / speed.max(1.0)).min(0.25);
        }
    }
    f64::INFINITY
}

/// How wide a character of `galley` is, on average, in points.
fn per_char(galley: &Galley) -> f32 {
    let chars = galley.job.text.chars().count().max(1);
    galley.rows.iter().map(|r| r.rect().width()).sum::<f32>() / chars as f32
}

/// `a` going toward `b` by `t`, premultiplied as egui's colours are.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let (a, b) = (a.to_array(), b.to_array());
    let c = |k: usize| (f32::from(a[k]) + (f32::from(b[k]) - f32::from(a[k])) * t.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_premultiplied(c(0), c(1), c(2), c(3))
}

fn plain(app: &App, ui: &mut Ui, video_id: &str, lyrics: &Lyrics, big: bool) {
    let texts: Vec<&str> = lyrics.text.lines().collect();
    let readings = readings_for(app, ui, big, || readings::is_japanese(&texts));
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
                let key = ui.id().with(("plain-readings", video_id, &source, width.round() as i32, readings.1));
                let laid: Arc<Vec<LaidLine>> = cached(ui, ui.id().with("plain-readings"), key, || {
                    Arc::new(texts.iter().map(|t| lay_line(&painter, t, &font, width, Some(readings), 0.0)).collect())
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

    let timeline = timeline(ui, video_id, lyrics);
    let lines = &timeline.lines;

    // Lyrics a little early or late for this recording: moved, per song.
    let offset = app.personal.lyric_offset(video_id);
    let heard = app.position() - offset;
    let position = heard + LEAD;
    let playing = app.playing();
    // The lines being sung: one, or more when singers overlap.
    let over = &timeline.over;
    let active: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].time <= position && position < over[i]).collect();
    let dots_h = size * 0.9;

    // Where each line sits, from the top of the lyrics.
    // Laid out once per song, source, size, width, readings and
    // translations, not every frame.
    let readings = readings_for(app, ui, big, || timeline.japanese);
    let scale = (ui.ctx().pixels_per_point() * 100.0).round() as i32;
    let machine = &all[0].translated;
    let source = (lyrics.choice(), lines.len(), lyrics.text.len(), machine.lines.len());
    let layout_key = ui.id().with(("lyrics-layout", video_id, source, width.round() as i32, big, scale, readings.map(|r| r.1), translations));
    let layout: Arc<Layout> = cached(ui, ui.id().with("lyrics-layout"), layout_key, || {
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
        // One singer: every line on the left, as ever.
        let apart = duet(&lines);
        let mut untranslated = Vec::new();
        let mut borrowed_from: Vec<String> = Vec::new();
        let mut by_machine = false;
        let laid: Vec<Option<Laid>> = lines
            .iter()
            .map(|line| {
                if line.text.is_empty() && line.backing.is_empty() {
                    return None;
                }
                let voice = if apart { line.singer() } else { Voice::First };
                let at = across(voice);
                let main = lay_line(&painter, &line.text, &font, width, readings, at);
                let timed = spans(&line.words, &line.text);
                let mut height = if line.text.is_empty() { 0.0 } else { main.galley.size().y };
                let backing = (!line.backing.is_empty()).then(|| {
                    let text: String = line.backing.iter().map(|w| w.text.as_str()).collect();
                    let galley = lay_text(&painter, &text, backing_font.clone(), width, at);
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
                    let galley = lay_text(&painter, &text, translation_font.clone(), width, at);
                    let y = height + size * 0.18;
                    height = y + galley.size().y;
                    (galley, y)
                });
                Some(Laid { voice, main, timed, backing, translation, height })
            })
            .collect();
        // Only lyrics that may be in another language, and not already
        // found to be in English.
        untranslated.sort();
        untranslated.dedup();
        if machine.from.as_deref() == Some(crate::api::translate::INTO) || !crate::api::translate::worth_asking(&untranslated) {
            untranslated.clear();
        }
        Arc::new(Layout { lines: laid, untranslated, borrowed_from, by_machine })
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
            None if active.contains(&i) => dots_h,
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

    // Follow the lines being sung (the middle of them, those still being
    // sung, as Better Lyrics does), unless scrolled by hand a moment ago.
    let id = ui.id().with(("lyrics-view", video_id));
    let mut view: View = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let now = ui.input(|i| i.time);
    let anchor = rect.height() * if big { 0.36 } else { 0.3 };
    let singing: Vec<usize> = active
        .iter()
        .enumerate()
        .filter(|&(k, &i)| k + 1 == active.len() || lines[i].end.is_none_or(|end| heard < end))
        .map(|(_, &i)| i)
        .collect();
    let focus: Vec<usize> = if singing.is_empty() { vec![0] } else { singing };
    let middle = focus.iter().map(|&i| tops.get(i).copied().unwrap_or(0.0) + height_of(i) / 2.0).sum::<f32>() / focus.len() as f32;
    let target = middle - anchor;
    if !view.placed {
        view = View {
            offset: target,
            from: target,
            to: target,
            placed: true,
            ..View::default()
        };
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
        if g < 1.0 && (view.to - view.offset).abs() > 0.25 {
            theme::next_frame(ui.ctx());
        } else {
            // Its last stretch is less than a pixel: there already.
            view.offset = view.to;
        }
    }
    ui.data_mut(|d| d.insert_temp(id, view));

    let pointer = ui.ctx().pointer_hover_pos().filter(|p| rect.contains(*p) && response.contains_pointer());
    let fade = if big { 72.0 } else { 40.0 };
    let mut clicked_line = None;
    // When the sweep next moves, in seconds.
    let mut sweep = f64::INFINITY;
    let mut lighting = false;
    let mut edging = false;
    let edges_id = ui.id().with(("lyrics-edges", video_id));
    let mut edges: Vec<Option<Edge>> = ui.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Vec<Option<Edge>>>(edges_id)));
    edges.resize(lines.len(), None);
    for (i, line) in lines.iter().enumerate() {
        let h = height_of(i);
        if h == 0.0 {
            continue;
        }
        let top = rect.top() + tops[i] - view.offset;
        if top > rect.bottom() || top + h < rect.top() {
            // Out of view: placed afresh when it comes back.
            edges[i] = None;
            continue;
        }
        let line_rect = Rect::from_min_size(pos2(rect.left() + pad, top), vec2(width, h));
        let is_active = active.contains(&i);
        // A line coming in brightens, one that is over dims.
        let lit = light(line.time, over[i], position, fade_out(&line.words, line.end.map(|end| (line.time, end, line.text.as_str()))));
        lighting |= lit > 0.0 && lit < 1.0;
        // Lines fade out toward the edges of the view, by where the view is
        // going rather than where its glide has them: the glide is quick,
        // and would dim a line pushed into the edge all at once. The fade
        // moves on at its own pace instead ([`Edge::ease`]).
        let bound = if view.by_hand { top } else { rect.top() + tops[i] - view.to };
        let target = ((bound - rect.top()) / fade).min((rect.bottom() - bound - h) / fade).clamp(0.0, 1.0);
        let (edge, easing) = Edge::ease(&mut edges[i], target, now, view.by_hand);
        edging |= easing;
        // How bright a part of a line is goes straight from its rest (with
        // the edges' fade) to its brightest as it lights: not one still lit
        // is dimmed by the edge, so it dims at the pace of its own fade.
        let at_edge = 0.25 + 0.75 * edge;
        let tone = |rest: f32, full: f32, lit: f32| rest * at_edge + (full - rest * at_edge) * lit;
        match &galleys[i] {
            Some(laid) => {
                // Drawn in its singer's colour (white, with one singer).
                let base = voice_colour(laid.voice);
                let ink = |alpha: f32| base.gamma_multiply(alpha.clamp(0.0, 1.0));
                let hovered = pointer.is_some_and(|p| line_rect.expand2(vec2(pad, gap / 2.0)).contains(p));
                if hovered {
                    painter.rect_filled(line_rect.expand2(vec2(pad, gap / 3.0)), CornerRadius::same(8), Color32::from_white_alpha(12));
                    crate::ui::widgets::hover_glow(ui, line_rect.expand2(vec2(pad, gap / 3.0)), 8.0, 1.0);
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    if response.clicked() {
                        clicked_line = Some(line.time);
                    }
                }
                if !laid.timed.is_empty() && lit > 0.0 && !line.text.is_empty() {
                    // Being sung (or just over), timed by the word: lit as far
                    // as the singing has come, the rest a little brighter
                    // than the lines around it.
                    let upto = sung(&laid.timed, heard);
                    let dim = ink(tone(0.32, 0.46, lit));
                    let bright = ink(tone(0.32, 1.0, lit));
                    paint_sung(&painter, line_rect.min, &laid.main.galley, upto, dim, bright);
                    for ruby in &laid.main.readings {
                        let color = if upto > ruby.at as f32 { bright } else { dim };
                        painter.galley_with_override_text_color(
                            line_rect.min + ruby.center.to_vec2() - ruby.text.size() / 2.0,
                            ruby.text.clone(),
                            color.gamma_multiply(0.72),
                        );
                    }
                    if is_active {
                        sweep = sweep.min(sweep_wait(&laid.timed, heard, per_char(&laid.main.galley)));
                    }
                } else if !line.text.is_empty() {
                    paint_line(&painter, line_rect.min, &laid.main, ink(tone(0.32, 1.0, lit)));
                }
                if let Some((galley, y, timed)) = &laid.backing {
                    let at = line_rect.min + vec2(0.0, *y);
                    // Lit while their line is, and for as long as they are
                    // sung after it; then dimmed by how fast they were sung,
                    // from however bright they were to the line's rest.
                    let end = line.backing.last().map_or(line.time, |w| w.end);
                    let glow = light(line.time, over[i].max(end + LEAD), position, fade_out(&line.backing, None));
                    lighting |= glow > 0.0 && glow < 1.0;
                    let level = |full: f32| ink(tone(0.32 * 0.7, full, glow));
                    // Swept as sung only when the line is timed by the word.
                    if !timed.is_empty() && !laid.timed.is_empty() {
                        paint_sung(&painter, at, galley, sung(timed, heard), level(0.26), level(0.78));
                        if glow > 0.0 {
                            sweep = sweep.min(sweep_wait(timed, heard, per_char(galley)));
                        }
                    } else {
                        painter.galley_with_override_text_color(at, galley.clone(), level(0.7));
                    }
                }
                if let Some((galley, y)) = &laid.translation {
                    painter.galley_with_override_text_color(line_rect.min + vec2(0.0, *y), galley.clone(), ink(tone(0.32 * 0.62, 0.62, lit)));
                }
            }
            None => {
                let next = lines.get(i + 1).map_or(line.time + 4.0, |l| l.time);
                dots(ui, &painter, line_rect, size, (position - line.time) / (next - line.time).max(0.5), playing);
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(edges_id, edges));
    if edging {
        // Paused or not: the view moves either way.
        theme::next_frame(ui.ctx());
    }
    let footer_top = rect.top() + total - view.offset + gap;
    if footer_top < rect.bottom() && footer_top > rect.top() - 40.0
        && let Some(mut said) = footer(lyrics)
    {
        if !layout.borrowed_from.is_empty() {
            said.push_str(&format!("\nTranslations from {}", layout.borrowed_from.join(", ")));
        }
        if layout.by_machine {
            said.push_str("\nTranslations from Unison, Better Lyrics' community (made by machine)");
        }
        let galley = painter.layout(said, theme::regular(12.5), theme::text_faint(), width);
        painter.galley(pos2(rect.left() + pad, footer_top), galley, theme::text_faint());
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
    if playing && lighting {
        // A line lighting or going out: every frame, at no more than 60 a
        // second.
        theme::next_frame(ui.ctx());
    } else if playing {
        // Otherwise only as the sweep moves (a frame a pixel, at no more
        // than 60 a second), and for the next line.
        let next = lines.iter().find(|l| l.time > position).map_or(f64::INFINITY, |l| l.time - position);
        let wait = sweep.min(next);
        if wait.is_finite() {
            theme::repaint_in(ui.ctx(), std::time::Duration::from_secs_f64(wait.max(1.0 / 60.0)));
        }
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
    fn singers_are_set_apart_only_when_there_is_more_than_one() {
        let line = |voice: Voice, text: &str| LyricLine { time: 1.0, text: text.into(), voice, ..LyricLine::default() };
        assert!(!duet(&[line(Voice::First, "a"), line(Voice::First, "b")]));
        // One singer named, whoever: all on the left.
        assert!(!duet(&[line(Voice::Second, "a"), line(Voice::Second, "b")]));
        // A break says nothing of who sings.
        assert!(!duet(&[line(Voice::First, "a"), line(Voice::Second, "")]));
        assert!(duet(&[line(Voice::First, "a"), line(Voice::Second, "b")]));
        assert!(duet(&[line(Voice::First, "a"), line(Voice::Together, "b")]));
        // Lyrics kept by 0.3.5 said only whether a line was the second voice's.
        let old = LyricLine { time: 2.0, text: "c".into(), second_voice: true, ..LyricLine::default() };
        assert_eq!(old.singer(), Voice::Second);
        assert!(duet(&[line(Voice::First, "a"), old]));
        assert_eq!((across(Voice::First), across(Voice::Second), across(Voice::Third), across(Voice::Together)), (0.0, 1.0, 1.0, 0.5));
    }

    #[test]
    fn lines_sung_over_one_another_are_sung_together() {
        let line = |time: f64, end: Option<f64>| LyricLine { time, end, text: "x".into(), ..LyricLine::default() };
        // 打上花火's round: each voice comes in before the other is done.
        let round = [line(158.42, Some(160.55)), line(159.49, Some(161.52)), line(160.86, Some(162.66)), line(170.0, Some(172.0))];
        let over = overs(&round);
        assert_eq!(over[..3], [160.55, 161.52, 170.0]);
        assert_eq!(over[3], f64::INFINITY);
        let active = |at: f64| (0..round.len()).filter(|&i| round[i].time <= at && at < over[i]).collect::<Vec<_>>();
        assert_eq!(active(158.5), [0]);
        assert_eq!(active(160.0), [0, 1]);
        assert_eq!(active(161.0), [1, 2]);
        // An ended line stays lit until the next comes in.
        assert_eq!(active(165.0), [2]);
        // Timed by the line alone: one at a time, as ever.
        let plain = [line(1.0, None), line(5.0, None), line(9.0, None)];
        let over = overs(&plain);
        assert_eq!(over[..2], [5.0, 9.0]);
        // Lit coming in, dimmed once over, by the song's own time.
        assert_eq!(light(5.0, 9.0, 4.0, LIGHT), 0.0);
        assert!(light(5.0, 9.0, 5.1, LIGHT) > 0.0 && light(5.0, 9.0, 5.1, LIGHT) < 1.0);
        assert_eq!(light(5.0, 9.0, 7.0, LIGHT), 1.0);
        assert!(light(5.0, 9.0, 9.1, LIGHT) < 1.0);
        assert_eq!(light(5.0, 9.0, 10.0, LIGHT), 0.0);
    }

    #[test]
    fn what_was_sung_slowly_fades_slowly() {
        let words = |each: f64, n: usize| (0..n).map(|k| word(k as f64 * each, (k + 1) as f64 * each, "la ")).collect::<Vec<_>>();
        // Quick words go out quickly, slow ones ebb away, within bounds.
        let quick = fade_out(&words(0.2, 6), None);
        let slow = fade_out(&words(0.6, 4), None);
        assert!((quick - 0.25).abs() < 1e-4);
        assert!((slow - 0.75).abs() < 1e-4);
        assert_eq!(fade_out(&words(0.05, 8), None), FADE_MIN);
        assert_eq!(fade_out(&words(3.0, 2), None), FADE_MAX);
        // Timed by the line: its length over its words, or its characters.
        assert!((fade_out(&[], Some((10.0, 12.0, "one two three four"))) - 0.625).abs() < 1e-4);
        assert!((fade_out(&[], Some((10.0, 12.0, "夜に咲いた"))) - 0.5).abs() < 1e-4);
        assert_eq!(fade_out(&[], None), LIGHT);
        // Backing vocals sung on after their line, until 15: lit while sung,
        // then dimmed gently, never more than a little in a frame.
        let over = 15.0;
        assert_eq!(light(10.0, over, 14.5, slow), 1.0);
        let step = 1.0 / 60.0;
        assert!(light(10.0, over, over + step, slow) > 0.99);
        let (mut last, mut at) = (1.0, over);
        while at < over + f64::from(slow) {
            let now = light(10.0, over, at, slow);
            assert!(last - now < 0.03, "never more than a little in a frame");
            last = now;
            at += step;
        }
        assert!((light(10.0, over, over + f64::from(slow) / 2.0, slow) - 0.5).abs() < 1e-3);
        assert_eq!(light(10.0, over, over + f64::from(slow), slow), 0.0);
        assert!(light(10.0, over, over + 0.3, slow) > light(10.0, over, over + 0.3, quick));
        // At an even pace through the middle: no part of it rushes.
        let speed = |at: f64| (light(10.0, over, at, slow) - light(10.0, over, at + step, slow)) as f64 / step;
        let even = 1.0 / (0.8 * f64::from(slow));
        for k in 0..6 {
            let at = over + f64::from(slow) * (0.25 + 0.1 * f64::from(k));
            assert!((speed(at) - even).abs() < 0.02 * even, "even at {at}");
        }
    }

    #[test]
    fn the_view_moving_on_dims_a_line_at_its_own_pace() {
        let mut slot = None;
        assert_eq!(Edge::ease(&mut slot, 1.0, 10.0, false), (1.0, false));
        // The view moves on, the line now heading into the top's fade: it
        // starts where it was and gets there evenly, never at once.
        let (start, easing) = Edge::ease(&mut slot, 0.2, 11.0, false);
        assert!((start - 1.0).abs() < 1e-4 && easing);
        let step = 1.0 / 60.0;
        let mut last = start;
        let mut at = 11.0;
        while at < 11.0 + f64::from(EDGE_EASE) {
            at += step;
            let (now, _) = Edge::ease(&mut slot, 0.2, at, false);
            assert!(last - now < 0.8 * 1.25 / EDGE_EASE / 60.0 + 1e-3, "a little a frame");
            last = now;
        }
        let (end, easing) = Edge::ease(&mut slot, 0.2, at, false);
        assert!((end - 0.2).abs() < 1e-5 && !easing);
        // Turned back halfway: from where it had got to.
        let (_, _) = Edge::ease(&mut slot, 1.0, 20.0, false);
        let (half, _) = Edge::ease(&mut slot, 1.0, 20.0 + f64::from(EDGE_EASE) / 2.0, false);
        let (back, _) = Edge::ease(&mut slot, 0.2, 20.0 + f64::from(EDGE_EASE) / 2.0, false);
        assert!((half - 0.6).abs() < 1e-3 && (back - half).abs() < 1e-4);
        // Scrolled by hand: where the hand puts it.
        assert_eq!(Edge::ease(&mut slot, 0.9, 21.0, true), (0.9, false));
    }

    #[test]
    fn a_swept_line_is_one_mesh_cut_at_its_edge() {
        use egui::epaint::text::RowVisuals;
        let (dim, bright) = (Color32::WHITE.gamma_multiply(0.32), Color32::WHITE);
        // The soft edge's colours go between the two, never past either.
        assert_eq!(mix(dim, bright, 0.0), dim);
        assert_eq!(mix(dim, bright, 1.0), bright);
        assert!(mix(dim, bright, 0.5).a() > dim.a() && mix(dim, bright, 0.5).a() < bright.a());
        // Two glyphs, 0-20 and 30-40 across, the edge at 5: the first cut
        // where the soft edge begins and ends, the second (past it) dim.
        let mut mesh = egui::Mesh::default();
        mesh.add_rect_with_uv(Rect::from_min_max(pos2(0.0, 0.0), pos2(20.0, 10.0)), Rect::from_min_max(pos2(100.0, 0.0), pos2(120.0, 10.0)), Color32::WHITE);
        mesh.add_rect_with_uv(Rect::from_min_max(pos2(30.0, 0.0), pos2(40.0, 10.0)), Rect::from_min_max(pos2(200.0, 0.0), pos2(210.0, 10.0)), Color32::WHITE);
        let mut visuals = RowVisuals { glyph_vertex_range: 0..8, mesh, ..RowVisuals::default() };
        cut_at_edge(&mut visuals, 5.0, dim, bright);
        let mesh = &visuals.mesh;
        assert_eq!((mesh.vertices.len(), mesh.indices.len()), (16, 24));
        assert_eq!(visuals.glyph_vertex_range, 0..16);
        let xs: Vec<f32> = mesh.vertices.iter().step_by(4).map(|v| v.pos.x).collect();
        assert_eq!(xs, [0.0, 5.0, 17.5, 30.0]);
        // Bright to the edge, dim past the soft edge, the texture following.
        assert_eq!((mesh.vertices[0].color, mesh.vertices[1].color), (bright, bright));
        assert_eq!(mesh.vertices[1].uv.x, 105.0);
        assert_eq!((mesh.vertices[9].color, mesh.vertices[13].color), (dim, dim));
        assert!(mesh.indices.chunks(6).enumerate().all(|(q, i)| i == [0, 1, 2, 2, 1, 3].map(|k| k + 4 * q as u32)));
    }

    #[test]
    fn frames_only_while_the_singing_moves() {
        // "ab " sung from 1 to 2, "cd" from 3 to 5: a pause between.
        let spans = spans(&[word(1.0, 2.0, "ab "), word(3.0, 5.0, "cd")], "ab cd");
        // Before and between words, nothing until the next begins.
        assert_eq!(sweep_wait(&spans, 0.5, 10.0), 0.5);
        assert!((sweep_wait(&spans, 2.4, 10.0) - 0.6).abs() < 1e-9);
        // While one is sung, about a pixel's travel: 3 characters a second
        // at 10 points is 30 points a second; slower, longer.
        assert!((sweep_wait(&spans, 1.5, 10.0) - 1.0 / 30.0).abs() < 1e-9);
        assert!((sweep_wait(&spans, 4.0, 10.0) - 1.0 / 10.0).abs() < 1e-9);
        // All sung: no more frames for it.
        assert_eq!(sweep_wait(&spans, 6.0, 10.0), f64::INFINITY);
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
