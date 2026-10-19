//! The visualiser: the music's spectrum rising from the top edge of the
//! player bar, in the theme's colours, the accent turning into its glow
//! from one side to the other. It fades in as a song plays, and away when
//! it stops; while nothing moves, nothing is drawn.

use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, Ui, epaint::PathStroke, pos2};

use crate::app::App;
use crate::theme;
use crate::visualiser::Mode;

/// How far above the bar it reaches at the loudest.
const HEIGHT: f32 = 58.0;

/// Paints over the page just above `bar`, before the seek bar's line.
pub fn paint(app: &mut App, ui: &Ui, painter: &Painter, bar: Rect) {
    let ctx = ui.ctx();
    let on = app.settings.visualiser;
    let live = on && app.playing();
    app.player.scope().on.store(on, std::sync::atomic::Ordering::Relaxed);
    let shown = ctx.animate_bool_with_time(egui::Id::new("visualiser-shown"), live, if live { 0.35 } else { 0.6 });
    if !on && shown == 0.0 {
        return;
    }
    let mode = app.settings.visualiser_mode;
    let width = bar.width();
    let bands = match mode {
        Mode::Bars => (width / 15.0).round().clamp(24.0, 112.0),
        Mode::Wave => (width / 16.0).round().clamp(24.0, 96.0),
        Mode::Mirror => (width / 32.0).round().clamp(14.0, 56.0),
        Mode::Aurora => (width / 34.0).round().clamp(14.0, 48.0),
    } as usize;
    let dt = ctx.input(|i| i.stable_dt);
    let moving = app.analyser.update(app.player.scope(), bands, live, dt, app.settings.visualiser_focus);
    // Thirty frames a second: smooth enough for motion this soft, and each
    // frame redraws the whole window, so half the work of sixty.
    if shown > 0.0 || moving {
        theme::repaint_in(ctx, std::time::Duration::from_micros(33_333));
    }
    if shown <= 0.0 || !moving {
        return;
    }
    let area = Rect::from_min_max(pos2(bar.left(), bar.top() - HEIGHT), pos2(bar.right(), bar.top()));
    let colours = Colours { from: theme::accent(), to: theme::glow(), alpha: shown, mirrored: false };
    shade(painter, area, shown);
    let (levels, peaks) = (app.analyser.levels(), app.analyser.peaks());
    match mode {
        Mode::Bars => bars(painter, area, levels, peaks, &colours),
        Mode::Wave => {
            let echo = heights(area, peaks);
            let main = heights(area, levels);
            wave(painter, area, &echo, &colours, 0.10, None);
            wave(painter, area, &main, &colours, 0.34, Some(1.6));
        }
        Mode::Mirror => {
            let mirrored = |values: &[f32]| values.iter().rev().chain(values.iter().skip(1)).copied().collect::<Vec<f32>>();
            let colours = Colours { mirrored: true, ..colours };
            wave(painter, area, &heights(area, &mirrored(peaks)), &colours, 0.10, None);
            wave(painter, area, &heights(area, &mirrored(levels)), &colours, 0.34, Some(1.6));
        }
        Mode::Aurora => aurora(painter, area, levels, &colours),
    }
}

/// The two colours across, and how much shows.
#[derive(Clone, Copy)]
struct Colours {
    from: Color32,
    to: Color32,
    alpha: f32,
    /// From the middle out, rather than left to right.
    mirrored: bool,
}

impl Colours {
    /// The colour at `t` across (0 to 1), at `opacity`.
    fn at(&self, t: f32, opacity: f32) -> Color32 {
        let t = if self.mirrored { (t - 0.5).abs() * 2.0 } else { t }.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        let (a, b) = (self.from, self.to);
        let alpha = (opacity * self.alpha).clamp(0.0, 1.0);
        Color32::from_rgba_unmultiplied(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()), (alpha * 255.0).round() as u8)
    }
}

/// A soft dark rising from the bar, so the colours sit on the bar's own
/// rather than across whatever the page shows there.
fn shade(painter: &Painter, area: Rect, shown: f32) {
    let base = theme::player();
    let clear = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 0);
    let deep = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), (150.0 * shown) as u8);
    let mut mesh = Mesh::default();
    mesh.colored_vertex(area.left_top(), clear);
    mesh.colored_vertex(area.right_top(), clear);
    mesh.colored_vertex(area.right_bottom(), deep);
    mesh.colored_vertex(area.left_bottom(), deep);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
}

/// Slim bars with round tops, brighter at the foot, and a cap above each
/// that falls behind it.
fn bars(painter: &Painter, area: Rect, levels: &[f32], peaks: &[f32], colours: &Colours) {
    let n = levels.len();
    let step = area.width() / n as f32;
    let w = (step * 0.5).clamp(2.5, 9.0);
    let r = w / 2.0;
    let tall = area.height() - 6.0;
    let base = area.bottom();
    let mut mesh = Mesh::default();
    for (i, (level, peak)) in levels.iter().zip(peaks).enumerate() {
        let t = (i as f32 + 0.5) / n as f32;
        let x = area.left() + step * (i as f32 + 0.5);
        let h = level * tall;
        if h >= 1.0 {
            let (foot, head) = (colours.at(t, 0.9), colours.at(t, 0.5));
            let top = base - h.max(w);
            // The body, from the foot to where the round top begins.
            let i0 = mesh.vertices.len() as u32;
            mesh.colored_vertex(pos2(x - r, base), foot);
            mesh.colored_vertex(pos2(x + r, base), foot);
            mesh.colored_vertex(pos2(x + r, top + r), head);
            mesh.colored_vertex(pos2(x - r, top + r), head);
            mesh.add_triangle(i0, i0 + 1, i0 + 2);
            mesh.add_triangle(i0, i0 + 2, i0 + 3);
            fan(&mut mesh, pos2(x, top + r), r, head);
        }
        // The cap, a short rounded dash, while it is above the bar.
        let cap = base - peak * tall - 4.0;
        if peak * tall > h + 3.0 {
            let c = colours.at(t, 0.65);
            let i0 = mesh.vertices.len() as u32;
            mesh.colored_vertex(pos2(x - r, cap - 1.0), c);
            mesh.colored_vertex(pos2(x + r, cap - 1.0), c);
            mesh.colored_vertex(pos2(x + r, cap + 1.0), c);
            mesh.colored_vertex(pos2(x - r, cap + 1.0), c);
            mesh.add_triangle(i0, i0 + 1, i0 + 2);
            mesh.add_triangle(i0, i0 + 2, i0 + 3);
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// The upper half of a disc, as a fan of triangles.
fn fan(mesh: &mut Mesh, centre: Pos2, r: f32, colour: Color32) {
    const STEPS: usize = 6;
    let c = mesh.vertices.len() as u32;
    mesh.colored_vertex(centre, colour);
    for s in 0..=STEPS {
        let a = std::f32::consts::PI * (1.0 + s as f32 / STEPS as f32);
        mesh.colored_vertex(pos2(centre.x + r * a.cos(), centre.y + r * a.sin()), colour);
    }
    for s in 0..STEPS as u32 {
        mesh.add_triangle(c, c + 1 + s, c + 2 + s);
    }
}

/// The points of a smooth curve through `values` across `area`, meeting
/// the bar at both ends.
fn heights(area: Rect, values: &[f32]) -> Vec<Pos2> {
    let n = values.len();
    let tall = area.height() - 4.0;
    // The ends rest on the bar, half a step out.
    let step = area.width() / n as f32;
    let mut knots = Vec::with_capacity(n + 2);
    knots.push(pos2(area.left(), area.bottom()));
    for (i, v) in values.iter().enumerate() {
        knots.push(pos2(area.left() + step * (i as f32 + 0.5), area.bottom() - v * tall));
    }
    knots.push(pos2(area.right(), area.bottom()));
    // Catmull-Rom between each pair of knots.
    const SUB: usize = 6;
    let mut points = Vec::with_capacity((knots.len() - 1) * SUB + 1);
    for k in 0..knots.len() - 1 {
        let p0 = knots[k.saturating_sub(1)];
        let (p1, p2) = (knots[k], knots[k + 1]);
        let p3 = knots[(k + 2).min(knots.len() - 1)];
        for s in 0..SUB {
            let t = s as f32 / SUB as f32;
            let (t2, t3) = (t * t, t * t * t);
            let blend = |a: f32, b: f32, c: f32, d: f32| {
                0.5 * (2.0 * b + (c - a) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (3.0 * b - a - 3.0 * c + d) * t3)
            };
            let y = blend(p0.y, p1.y, p2.y, p3.y).min(area.bottom());
            points.push(pos2(blend(p0.x, p1.x, p2.x, p3.x), y));
        }
    }
    points.push(*knots.last().unwrap_or(&area.right_bottom()));
    points
}

/// The area under `points` filled, `fill` strong under the line and clear
/// at the bar; with `line`, the line itself that wide.
fn wave(painter: &Painter, area: Rect, points: &[Pos2], colours: &Colours, fill: f32, line: Option<f32>) {
    let across = |p: Pos2| (p.x - area.left()) / area.width();
    let mut mesh = Mesh::default();
    for (i, p) in points.iter().enumerate() {
        let t = across(*p);
        let lift = ((area.bottom() - p.y) / area.height()).clamp(0.0, 1.0);
        mesh.colored_vertex(*p, colours.at(t, fill * (0.4 + 0.6 * lift)));
        mesh.colored_vertex(pos2(p.x, area.bottom()), colours.at(t, fill * 0.15));
        if i > 0 {
            let i = (i * 2) as u32;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i + 1, i);
        }
    }
    painter.add(Shape::mesh(mesh));
    if let Some(width) = line {
        let colours = *colours;
        let (left, w) = (area.left(), area.width());
        let stroke = PathStroke::new_uv(width, move |_, p| colours.at((p.x - left) / w, 0.95));
        painter.add(Shape::line(points.to_vec(), stroke));
    }
}

/// Soft columns of light, each fading to the sides and upward, wide enough
/// to run into their neighbours, with a thin glow along the bar.
fn aurora(painter: &Painter, area: Rect, levels: &[f32], colours: &Colours) {
    let n = levels.len();
    let step = area.width() / n as f32;
    let half = step * 1.4;
    let base = area.bottom();
    let mut mesh = Mesh::default();
    for (i, level) in levels.iter().enumerate() {
        if *level < 0.01 {
            continue;
        }
        let t = (i as f32 + 0.5) / n as f32;
        let x = area.left() + step * (i as f32 + 0.5);
        let h = ((0.15 + level * 1.5) * area.height()).min(area.height());
        let strong = colours.at(t, 0.3 + 0.45 * level);
        let middle = colours.at(t, (0.3 + 0.45 * level) * 0.45);
        let clear = colours.at(t, 0.0);
        // Three columns (edge, centre, edge) by three rows (foot, middle,
        // top): only the centre's foot and middle carry colour.
        let rows = [(base, strong), (base - h * 0.55, middle), (base - h, clear)];
        let i0 = mesh.vertices.len() as u32;
        for (y, centre) in rows {
            mesh.colored_vertex(pos2(x - half, y), clear);
            mesh.colored_vertex(pos2(x, y), centre);
            mesh.colored_vertex(pos2(x + half, y), clear);
        }
        for row in 0..2u32 {
            for col in 0..2u32 {
                let a = i0 + row * 3 + col;
                mesh.add_triangle(a, a + 1, a + 4);
                mesh.add_triangle(a, a + 4, a + 3);
            }
        }
    }
    // The glow along the bar, as loud as the music is overall.
    let overall = levels.iter().sum::<f32>() / n.max(1) as f32;
    let foot = Rect::from_min_max(pos2(area.left(), base - 3.0), area.right_bottom());
    let i0 = mesh.vertices.len() as u32;
    for (k, x) in [foot.left(), foot.center().x, foot.right()].into_iter().enumerate() {
        let t = k as f32 / 2.0;
        mesh.colored_vertex(pos2(x, foot.top()), colours.at(t, 0.0));
        mesh.colored_vertex(pos2(x, foot.bottom()), colours.at(t, 0.25 + 0.6 * overall));
    }
    for k in 0..2u32 {
        let a = i0 + k * 2;
        mesh.add_triangle(a, a + 2, a + 3);
        mesh.add_triangle(a, a + 3, a + 1);
    }
    painter.add(Shape::mesh(mesh));
}
