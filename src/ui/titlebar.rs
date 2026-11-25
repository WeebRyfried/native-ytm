//! The window's own title bar, in place of Windows': the logo and the name
//! on a strip of the sidebar's colour lit by the theme's accent, the
//! window's three buttons on the right. It moves the window when dragged,
//! and a double-click maximises it. Also the window's edges, which resize
//! it, as a window without Windows' frame has none of its own.

use egui::{Color32, CornerRadius, CursorIcon, Rect, Sense, Stroke, Ui, ViewportCommand, pos2, vec2};

use crate::app::App;
use crate::theme;
use crate::ui::widgets;

pub const HEIGHT: f32 = 34.0;
const BUTTON_W: f32 = 46.0;

/// The bar's colour: the sidebar's, so the two read as one.
pub fn fill() -> Color32 {
    theme::panel(theme::sidebar())
}

pub fn show(_app: &mut App, ui: &mut Ui) {
    let rect = ui.max_rect();
    let ctx = ui.ctx().clone();
    let (maximized, focused) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().focused.unwrap_or(true)));
    let painter = ui.painter().clone();
    // A glow of the accent along the top edge, gone by the bar's foot, so
    // the bar runs into the window beneath without a line.
    let mut mesh = egui::Mesh::default();
    let strength = if focused { 0.16 } else { 0.08 };
    for (y, alpha) in [(rect.top(), strength), (rect.bottom(), 0.0)] {
        let colour = theme::accent().gamma_multiply(alpha);
        mesh.colored_vertex(pos2(rect.left(), y), colour);
        mesh.colored_vertex(pos2(rect.right(), y), colour);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(egui::Shape::mesh(mesh));

    // Dragging anywhere not a button moves the window.
    let drag = ui.interact(rect, ui.id().with("titlebar-drag"), Sense::click_and_drag());
    if drag.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }

    // In the middle: the logo, the name, and the version small beside it.
    let text = if focused { theme::text_dim() } else { theme::text_faint() };
    let logo_size = 18.0;
    let name = painter.layout_no_wrap("Native YTM".to_owned(), theme::medium(12.5), text);
    let version = painter.layout_no_wrap(concat!("v", env!("CARGO_PKG_VERSION")).to_owned(), theme::medium(11.0), theme::text_faint());
    let width = logo_size + 8.0 + name.size().x + 6.0 + version.size().x;
    let left = (rect.center().x - width / 2.0).round();
    let logo = Rect::from_center_size(pos2(left + logo_size / 2.0, rect.center().y), vec2(logo_size, logo_size));
    let texture = widgets::app_logo(ui, logo_size);
    let tint = if focused { Color32::WHITE } else { Color32::from_white_alpha(170) };
    painter.image(texture.id(), logo, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
    let name_at = pos2(logo.right() + 8.0, rect.center().y - name.size().y / 2.0);
    let name_w = name.size().x;
    let name_h = name.size().y;
    painter.galley(name_at, name, text);
    // On the name's baseline (Inter's sits about four fifths down a line).
    let baseline = name_at.y + name_h * 0.8;
    painter.galley(pos2(name_at.x + name_w + 6.0, baseline - version.size().y * 0.8), version, theme::text_faint());

    // Minimise, maximise or restore, close.
    let mut x = rect.right() - BUTTON_W * 3.0;
    for which in 0..3 {
        let button = Rect::from_min_size(pos2(x, rect.top()), vec2(BUTTON_W, rect.height()));
        x += BUTTON_W;
        let response = ui.interact(button, ui.id().with(("titlebar-button", which)), Sense::click());
        let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.1);
        let close = which == 2;
        if hover > 0.0 {
            let fill = if close { Color32::from_rgb(0xc4, 0x2b, 0x1c) } else { Color32::from_white_alpha(22) };
            painter.rect_filled(button, CornerRadius::ZERO, fill.gamma_multiply(hover));
        }
        let colour = if close && hover > 0.5 { Color32::WHITE } else if response.hovered() { theme::text() } else { text };
        let stroke = Stroke::new(1.0, colour);
        let c = button.center();
        let s = 5.0;
        match which {
            0 => {
                painter.hline(c.x - s..=c.x + s, c.y.round() + 0.5, stroke);
            }
            1 if maximized => {
                // Two windows, one behind the other.
                let front = Rect::from_center_size(c + vec2(-1.0, 1.0), vec2(8.0, 8.0));
                painter.rect_stroke(front, CornerRadius::same(1), stroke, egui::StrokeKind::Middle);
                painter.line_segment([pos2(front.left() + 2.0, front.top() - 2.0), pos2(front.right() + 2.0, front.top() - 2.0)], stroke);
                painter.line_segment([pos2(front.right() + 2.0, front.top() - 2.0), pos2(front.right() + 2.0, front.bottom() - 2.0)], stroke);
            }
            1 => {
                painter.rect_stroke(Rect::from_center_size(c, vec2(10.0, 10.0)), CornerRadius::same(1), stroke, egui::StrokeKind::Middle);
            }
            _ => {
                painter.line_segment([c + vec2(-s, -s), c + vec2(s, s)], stroke);
                painter.line_segment([c + vec2(-s, s), c + vec2(s, -s)], stroke);
            }
        }
        let tip = match which {
            0 => "Minimise",
            1 if maximized => "Restore",
            1 => "Maximise",
            _ => "Close",
        };
        if response.on_hover_text(tip).clicked() {
            match which {
                0 => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                1 => ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
                _ => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
    }
}

/// The window's edges and corners resize it, while it is not maximised.
pub fn resize_edges(ctx: &egui::Context) {
    const EDGE: f32 = 5.0;
    let (maximized, pos, pressed) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.pointer.hover_pos(), i.pointer.primary_pressed()));
    let Some(pos) = pos else { return };
    if maximized || ctx.dragged_id().is_some() {
        return;
    }
    let screen = ctx.content_rect();
    let left = pos.x - screen.left() < EDGE;
    let right = screen.right() - pos.x < EDGE;
    let top = pos.y - screen.top() < EDGE;
    let bottom = screen.bottom() - pos.y < EDGE;
    use egui::viewport::ResizeDirection as D;
    let (direction, cursor) = match (left, right, top, bottom) {
        (true, _, true, _) => (D::NorthWest, CursorIcon::ResizeNorthWest),
        (_, true, true, _) => (D::NorthEast, CursorIcon::ResizeNorthEast),
        (true, _, _, true) => (D::SouthWest, CursorIcon::ResizeSouthWest),
        (_, true, _, true) => (D::SouthEast, CursorIcon::ResizeSouthEast),
        (true, ..) => (D::West, CursorIcon::ResizeWest),
        (_, true, ..) => (D::East, CursorIcon::ResizeEast),
        (_, _, true, _) => (D::North, CursorIcon::ResizeNorth),
        (.., true) => (D::South, CursorIcon::ResizeSouth),
        _ => return,
    };
    ctx.set_cursor_icon(cursor);
    if pressed {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}
