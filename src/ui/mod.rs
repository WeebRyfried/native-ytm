//! The window: the sidebar, the top bar, the page, the side panel and the
//! player bar, with dialogs and toasts over them.

mod dialogs;
mod keys;
mod lyrics;
mod now_playing;
mod pages;
mod player_bar;
mod settings;
mod side_panel;
mod sidebar;
mod topbar;
pub mod widgets;

use egui::{Align2, Color32, CornerRadius, Frame, Margin, Rect, vec2};

use crate::app::App;
use crate::model::ToastKind;
use crate::theme;

pub use widgets::Cx;

impl App {
    /// The shared view context, alongside the parts of the app pages read.
    pub fn cx(&mut self) -> Cx<'_> {
        let current = self.queue.current_track().map(|t| t.video_id.as_str());
        let playing = matches!(
            self.player.status().state,
            crate::player::State::Playing | crate::player::State::Buffering
        );
        Cx {
            images: &mut self.images,
            actions: &mut self.actions,
            current,
            playing,
            signed_in: self.signed_in,
        }
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    keys::handle(app, &ctx);
    egui::Panel::bottom("player")
        .exact_size(theme::PLAYER_HEIGHT)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().fill(theme::PLAYER))
        .show(ui, |ui| player_bar::show(app, ui));
    // The full view fades in over the window and rises into place, and
    // fades out the same way. Fully open, the window beneath is not drawn.
    let rest = ui.available_rect_before_wrap();
    let open = ctx.animate_bool_with_time(egui::Id::new("full-view-open"), app.full_view, 0.22);
    if open < 1.0 {
        windowed(app, ui);
    }
    if open > 0.0 {
        let eased = theme::ease_out(open);
        egui::Area::new(egui::Id::new("full-view"))
            .order(egui::Order::Middle)
            .fixed_pos(rest.min)
            .show(&ctx, |area| {
                let mut ui = area.new_child(egui::UiBuilder::new().max_rect(rest));
                ui.set_clip_rect(rest);
                ui.multiply_opacity(eased);
                now_playing::show(app, &mut ui, rest, (1.0 - eased) * 28.0);
            });
    }
    dialogs::show(app, &ctx);
    toasts(app, &ctx);
}

/// The window as usual: the sidebar, the side panel and the page.
fn windowed(app: &mut App, ui: &mut egui::Ui) {
    if app.settings.sidebar {
        egui::Panel::left("sidebar")
            .exact_size(theme::SIDEBAR_WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(theme::SIDEBAR).inner_margin(Margin::symmetric(12, 0)))
            .show(ui, |ui| sidebar::show(app, ui));
    }
    if app.settings.side_panel {
        egui::Panel::right("side-panel")
            .default_size(theme::SIDE_PANEL_WIDTH)
            .size_range(300.0..=560.0)
            .resizable(true)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(theme::SIDEBAR)
                    .inner_margin(Margin::symmetric(12, 12)),
            )
            .show(ui, |ui| side_panel::show(app, ui));
    }
    egui::CentralPanel::default()
        .frame(Frame::new().fill(theme::BG))
        .show(ui, |ui| {
            topbar::show(app, ui);
            pages::show(app, ui);
        });
}

fn toasts(app: &App, ctx: &egui::Context) {
    if app.toasts.is_empty() {
        return;
    }
    let screen = ctx.content_rect();
    let mut y = screen.bottom() - theme::PLAYER_HEIGHT - 20.0;
    for toast in app.toasts.iter().rev() {
        let galley = ctx.fonts_mut(|f| {
            f.layout(
                toast.text.clone(),
                theme::regular(14.0),
                theme::TEXT,
                520.0,
            )
        });
        let size = galley.size() + vec2(32.0, 20.0);
        // In: rising and fading in. Out: fading as it sinks a little.
        let now = std::time::Instant::now();
        let shown = theme::ease_out(now.duration_since(toast.born).as_secs_f32() / 0.22);
        let left = toast.until.saturating_duration_since(now).as_secs_f32();
        let leaving = (left / 0.25).clamp(0.0, 1.0);
        let presence = shown.min(leaving);
        if presence < 1.0 {
            theme::next_frame(ctx);
        }
        let rect = Rect::from_center_size(
            egui::pos2(screen.center().x, y - size.y / 2.0 + (1.0 - presence) * 14.0),
            size,
        );
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("toasts")));
        let fill = match toast.kind {
            ToastKind::Info => Color32::from_rgb(0x2b, 0x2b, 0x2b),
            ToastKind::Error => Color32::from_rgb(0x5c, 0x1a, 0x1f),
        };
        painter.rect_filled(rect.translate(vec2(0.0, 3.0)), CornerRadius::same(8), Color32::from_black_alpha(90).gamma_multiply(presence));
        painter.rect_filled(rect, CornerRadius::same(8), fill.gamma_multiply(presence));
        painter.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, theme::TEXT.gamma_multiply(presence));
        y -= size.y + 8.0;
        let _ = Align2::CENTER_CENTER;
    }
    if let Some(next) = app.toasts.iter().map(|t| t.until).min() {
        theme::repaint_in(ctx, next.saturating_duration_since(std::time::Instant::now()));
    }
}
