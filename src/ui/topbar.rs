//! The bar above the page: back and forward, the search field with its
//! suggestions, and the account.

use egui::{Align, Color32, CornerRadius, CursorIcon, Key, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::app::App;
use crate::model::{Action, Route};
use crate::theme::{self, Icon};
use crate::ui::widgets;

pub fn show(app: &mut App, ui: &mut Ui) {
    let full = ui.available_rect_before_wrap();
    let rect = Rect::from_min_size(full.min, vec2(full.width(), theme::TOPBAR_HEIGHT));
    ui.allocate_rect(rect, Sense::hover());
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(16.0, 10.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = 4.0;
    if !app.settings.sidebar
        && widgets::icon_button(&mut bar, Icon::Sidebar, 20.0, theme::TEXT, "Show the sidebar").clicked()
    {
        app.actions.push(Action::ToggleSidebar);
    }
    let back_tint = if app.can_go_back() { theme::TEXT } else { theme::TEXT_FAINT };
    if widgets::icon_button(&mut bar, Icon::Back, 20.0, back_tint, "Back (Alt+Left)").clicked() {
        app.actions.push(Action::Back);
    }
    let forward_tint = if app.can_go_forward() { theme::TEXT } else { theme::TEXT_FAINT };
    if widgets::icon_button(&mut bar, Icon::Forward, 20.0, forward_tint, "Forward (Alt+Right)").clicked() {
        app.actions.push(Action::Forward);
    }
    bar.add_space(12.0);
    let search_w = (bar.available_width() - 120.0).clamp(200.0, 560.0);
    search_field(app, &mut bar, search_w);
    bar.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        account(app, ui);
        let settings_on = app.route == Route::Settings;
        if widgets::icon_button(ui, Icon::Settings, 20.0, if settings_on { theme::TEXT } else { theme::TEXT_DIM }, "Settings").clicked() {
            app.actions.push(Action::Navigate(Route::Settings));
        }
        if !app.online() {
            ui.add_space(6.0);
            offline_chip(app, ui);
        }
    });
}

/// "Offline", while there is no connection; it leads to the downloads.
fn offline_chip(app: &mut App, ui: &mut Ui) {
    let galley = ui.painter().layout_no_wrap("Offline".to_owned(), theme::medium(13.0), theme::TEXT);
    let size = vec2(galley.size().x + 44.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("No connection. Downloaded songs still play: see them in Library, Cached.");
    let shown = theme::appear(ui.ctx(), egui::Id::new("offline-chip"), 1, 0.25);
    let fill = if response.hovered() { Color32::from_white_alpha(30) } else { Color32::from_white_alpha(16) };
    ui.painter().rect_filled(rect, CornerRadius::same(15), fill.gamma_multiply(shown));
    Icon::Offline
        .image(theme::TEXT.gamma_multiply(shown), 16.0)
        .paint_at(ui, Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), Vec2::splat(16.0)));
    ui.painter().galley_with_override_text_color(
        pos2(rect.left() + 34.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::TEXT.gamma_multiply(shown),
    );
    if response.clicked() {
        app.actions.push(Action::Navigate(Route::Library(crate::model::LibraryTab::Cached)));
    }
}

fn search_field(app: &mut App, ui: &mut Ui, width: f32) {
    let id = egui::Id::new("search-field");
    let (rect, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    let focused = ui.memory(|m| m.has_focus(id));
    let fill = if focused { Color32::from_rgb(0x21, 0x21, 0x21) } else { Color32::from_white_alpha(16) };
    let stroke = if focused { Stroke::new(1.0, Color32::from_white_alpha(60)) } else { Stroke::NONE };
    ui.painter().rect(rect, CornerRadius::same(8), fill, stroke, StrokeKind::Inside);
    Icon::Search
        .image(theme::TEXT_DIM, 20.0)
        .paint_at(ui, Rect::from_min_size(pos2(rect.left() + 14.0, rect.center().y - 10.0), Vec2::splat(20.0)));
    let text_rect = Rect::from_min_max(pos2(rect.left() + 46.0, rect.top()), pos2(rect.right() - 36.0, rect.bottom()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(Layout::left_to_right(Align::Center)));
    if app.search.focus {
        app.search.focus = false;
        child.memory_mut(|m| m.request_focus(id));
    }
    let edit = egui::TextEdit::singleline(&mut app.search.input)
        .id(id)
        .frame(egui::Frame::NONE)
        .hint_text(RichText::new("Search songs, albums, artists, podcasts").color(theme::TEXT_FAINT))
        .font(theme::regular(15.0))
        .text_color(theme::TEXT)
        .desired_width(text_rect.width())
        .margin(egui::Margin::ZERO);
    let response = child.add(edit);
    if response.changed() {
        app.actions.push(Action::Suggest(app.search.input.clone()));
    }
    let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if submitted {
        app.actions.push(Action::Search(app.search.input.clone()));
    }
    if !app.search.input.is_empty() {
        let clear = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), Vec2::splat(28.0));
        let r = ui.interact(clear, id.with("clear"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        Icon::Close
            .image(if r.hovered() { theme::TEXT } else { theme::TEXT_DIM }, 18.0)
            .paint_at(ui, Rect::from_center_size(clear.center(), Vec2::splat(18.0)));
        if r.clicked() {
            app.search.input.clear();
            app.actions.push(Action::Suggest(String::new()));
            ui.memory_mut(|m| m.request_focus(id));
        }
    }
    // Suggestions below the field while it has focus.
    let has_focus = ui.memory(|m| m.has_focus(id));
    let open_id = id.with("open");
    if has_focus {
        ui.data_mut(|d| d.insert_temp(open_id, true));
    }
    let open = ui.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
    let suggestions = &app.search.suggestions;
    if !open || app.search.input.trim().is_empty() || (suggestions.queries.is_empty() && suggestions.items.is_empty()) {
        if !has_focus {
            ui.data_mut(|d| d.insert_temp(open_id, false));
        }
        return;
    }
    let queries = suggestions.queries.clone();
    let items = suggestions.items.clone();
    let area = egui::Area::new(id.with("suggestions"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(rect.left(), rect.bottom() + 4.0));
    let inner = area.show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style())
            .fill(theme::POPUP)
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(width - 12.0);
                for query in queries.iter().take(7) {
                    let (row, response) = ui.allocate_exact_size(vec2(width - 12.0, 36.0), Sense::click());
                    let response = response.on_hover_cursor(CursorIcon::PointingHand);
                    if response.hovered() {
                        ui.painter().rect_filled(row, CornerRadius::same(6), Color32::from_white_alpha(16));
                    }
                    Icon::Search.image(theme::TEXT_DIM, 18.0).paint_at(ui, Rect::from_min_size(pos2(row.left() + 10.0, row.center().y - 9.0), Vec2::splat(18.0)));
                    ui.painter().text(pos2(row.left() + 40.0, row.center().y), egui::Align2::LEFT_CENTER, query, theme::regular(14.5), theme::TEXT);
                    if response.clicked() {
                        app.search.input = query.clone();
                        app.actions.push(Action::Search(query.clone()));
                        ui.data_mut(|d| d.insert_temp(open_id, false));
                    }
                }
                if !items.is_empty() {
                    ui.separator();
                    let mut cx = app.cx();
                    for item in items.iter().take(4) {
                        let r = widgets::item_row(ui, &mut cx, item);
                        if r.clicked() {
                            ui.data_mut(|d| d.insert_temp(open_id, false));
                        }
                    }
                }
            });
    });
    // A click anywhere else closes it.
    if ui.input(|i| i.pointer.any_pressed())
        && !inner.response.hovered()
        && !ui.rect_contains_pointer(rect)
    {
        ui.data_mut(|d| d.insert_temp(open_id, false));
    }
    if ui.input(|i| i.key_pressed(Key::Escape)) {
        ui.data_mut(|d| d.insert_temp(open_id, false));
        ui.memory_mut(|m| m.surrender_focus(id));
    }
}

fn account(app: &mut App, ui: &mut Ui) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let photo = app.account.as_ref().and_then(|a| a.photo.clone());
    if app.signed_in && photo.is_some() {
        widgets::paint_art(ui, &mut app.images, photo.as_deref(), rect.shrink(2.0), 0.0, true);
    } else {
        if response.hovered() {
            ui.painter().circle_filled(rect.center(), 18.0, Color32::from_white_alpha(22));
        }
        Icon::Account.image(theme::TEXT, 24.0).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(24.0)));
    }
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(220.0);
        if let Some(account) = &app.account {
            ui.label(RichText::new(&account.name).font(theme::semibold(15.0)));
            if let Some(handle) = &account.handle {
                ui.label(RichText::new(handle).color(theme::TEXT_DIM));
            }
            ui.separator();
        }
        let entry = |ui: &mut Ui, icon: Icon, text: &str| {
            ui.add(egui::Button::image_and_text(icon.image(theme::TEXT, 18.0), text).frame_when_inactive(false)).clicked()
        };
        if app.signed_in {
            if entry(ui, Icon::History, "History") {
                app.actions.push(Action::Navigate(Route::Library(crate::model::LibraryTab::History)));
            }
            if entry(ui, Icon::SignOut, "Sign out") {
                app.actions.push(Action::SignOut);
            }
        } else if entry(ui, Icon::SignIn, "Sign in") {
            app.actions.push(Action::OpenSignIn);
        }
        if entry(ui, Icon::Settings, "Settings") {
            app.actions.push(Action::Navigate(Route::Settings));
        }
    });
}
