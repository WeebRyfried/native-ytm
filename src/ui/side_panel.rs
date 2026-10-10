//! The right panel: what plays next, or the lyrics.

use egui::{Align, Color32, CornerRadius, CursorIcon, Layout, Rect, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::app::App;
use crate::model::{Action, Loadable, SideTab};
use crate::theme::{self, Icon};
use crate::ui::widgets::{self, RowPlace};

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        for (tab, label) in [(SideTab::UpNext, "UP NEXT"), (SideTab::Lyrics, "LYRICS")] {
            let selected = app.side_tab == tab;
            let width = (ui.available_width() - 40.0) / 2.0;
            let (rect, response) = ui.allocate_exact_size(vec2(width.max(80.0), 40.0), Sense::click());
            let response = response.on_hover_cursor(CursorIcon::PointingHand);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                theme::semibold(13.5),
                if selected { theme::TEXT } else { theme::TEXT_DIM },
            );
            let line = if selected { Stroke::new(2.0, theme::TEXT) } else { Stroke::new(1.0, theme::DIVIDER) };
            ui.painter().hline(rect.x_range(), rect.bottom() - 1.0, line);
            if response.clicked() && !selected {
                app.actions.push(Action::ShowSidePanel(tab));
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(ui, Icon::Close, 18.0, theme::TEXT_DIM, "Close").clicked() {
                app.actions.push(Action::ToggleSidePanel);
            }
        });
    });
    ui.add_space(10.0);
    match app.side_tab {
        SideTab::UpNext => queue(app, ui),
        SideTab::Lyrics => lyrics(app, ui),
    }
}

fn queue(app: &mut App, ui: &mut Ui) {
    if app.queue.entries.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Nothing in the queue").color(theme::TEXT_DIM));
            ui.label(RichText::new("Play something and it shows up here.").color(theme::TEXT_FAINT).font(theme::regular(13.0)));
        });
        return;
    }
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("Playing from").color(theme::TEXT_DIM).font(theme::regular(12.0)));
            let title = app.queue.source.title.clone().unwrap_or_else(|| "Your queue".into());
            widgets::clipped_label(ui, &title, theme::semibold(15.0), theme::TEXT, ui.available_width() - 60.0, Sense::hover());
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(ui, Icon::ClearQueue, 18.0, theme::TEXT_DIM, "Clear what comes next").clicked() {
                app.actions.push(Action::ClearQueue);
            }
        });
    });
    ui.add_space(8.0);
    let current_index = app.queue.current;
    let entries: Vec<_> = app.queue.entries.clone();
    let scroll_to_current = ui.ctx().data_mut(|d| {
        let id = egui::Id::new("queue-scrolled-to");
        let last = d.get_temp::<u64>(id);
        let now = app.queue.current().map_or(0, |e| e.uid);
        d.insert_temp(id, now);
        last != Some(now)
    });
    let drag_from = app.drag_from;
    let mut drop_at: Option<usize> = None;
    let mut started_drag: Option<usize> = None;
    let mut cx = app.cx();
    egui::ScrollArea::vertical()
        .id_salt("queue")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (index, entry) in entries.iter().enumerate() {
                let top = ui.cursor().top();
                let clip = ui.clip_rect();
                let visible = top < clip.bottom() + 120.0 && top + 52.0 > clip.top() - 120.0;
                if !visible && !(scroll_to_current && current_index == Some(index)) {
                    ui.allocate_space(vec2(ui.available_width(), 52.0));
                    continue;
                }
                let row = widgets::track_row(ui, &mut cx, &entry.track, None, RowPlace::Queue(entry.uid), false);
                let response = row.response;
                if current_index == Some(index) && scroll_to_current {
                    response.scroll_to_me(Some(Align::Center));
                }
                if row.play || response.clicked() {
                    cx.push(Action::PlayQueueEntry(entry.uid));
                }
                // Reordering: drag a row onto another.
                if response.drag_started() {
                    started_drag = Some(index);
                }
                if drag_from.is_some()
                    && let Some(pointer) = ui.ctx().pointer_interact_pos()
                    && response.rect.contains(pointer)
                {
                    let y = if pointer.y < response.rect.center().y { response.rect.top() } else { response.rect.bottom() };
                    ui.painter().hline(response.rect.x_range(), y, Stroke::new(2.0, theme::ACCENT));
                    drop_at = Some(if pointer.y < response.rect.center().y { index } else { index + 1 });
                }
                if current_index.is_some_and(|c| index == c) {
                    let bar = Rect::from_min_size(response.rect.left_top(), vec2(3.0, response.rect.height()));
                    ui.painter().rect_filled(bar, CornerRadius::same(1), theme::ACCENT);
                }
            }
        });
    if let Some(index) = started_drag {
        app.drag_from = Some(index);
    }
    if let Some(from) = app.drag_from {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        if ui.input(|i| i.pointer.any_released()) {
            app.drag_from = None;
            if let Some(to) = drop_at {
                let to = if to > from { to - 1 } else { to };
                if to != from {
                    app.actions.push(Action::MoveInQueue(from, to));
                }
            }
        }
    }
    let _ = (Color32::WHITE, Vec2::ZERO, pos2(0.0, 0.0));
}

fn lyrics(app: &mut App, ui: &mut Ui) {
    let Some(track) = app.current().cloned() else {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| ui.label(RichText::new("Play a song to see its lyrics").color(theme::TEXT_DIM)));
        return;
    };
    app.request_lyrics();
    let video_id = track.video_id.clone();
    egui::ScrollArea::vertical()
        .id_salt(("lyrics", video_id.clone()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(8.0);
            match app.lyrics.get(&video_id) {
                Some(Loadable::Ready(Some(lyrics))) => {
                    ui.add(
                        egui::Label::new(RichText::new(&lyrics.text).font(theme::medium(17.0)).color(theme::TEXT))
                            .wrap(),
                    );
                    if let Some(source) = &lyrics.source {
                        ui.add_space(16.0);
                        ui.label(RichText::new(source).color(theme::TEXT_FAINT).font(theme::regular(12.0)));
                    }
                }
                Some(Loadable::Loading) => widgets::spinner(ui),
                Some(Loadable::Failed(_)) | Some(Loadable::Ready(None)) => {
                    ui.add_space(32.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new("Lyrics are not available").color(theme::TEXT_DIM)));
                }
                None => {
                    if app.now.video_id == video_id && app.now.lyrics_id.is_none() && app.now.like.is_some() {
                        ui.add_space(32.0);
                        ui.vertical_centered(|ui| ui.label(RichText::new("Lyrics are not available").color(theme::TEXT_DIM)));
                    } else {
                        widgets::spinner(ui);
                    }
                }
            }
            ui.add_space(24.0);
        });
}
