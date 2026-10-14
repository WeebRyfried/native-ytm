//! The left column: the app's name and version, the three destinations,
//! and the account's playlists.

use egui::{Align, Color32, CornerRadius, CursorIcon, Layout, Rect, RichText, Sense, Ui, Vec2, pos2, vec2};

use crate::api::model::{Kind, Target};
use crate::app::App;
use crate::model::{Action, LibraryTab, Route};
use crate::theme::{self, Icon};
use crate::ui::widgets;

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
        let texture = ui.ctx().data_mut(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("app-icon")));
        let texture = texture.unwrap_or_else(|| {
            let size = 64;
            let image = egui::ColorImage::from_rgba_unmultiplied([size, size], &theme::app_icon_rgba(size));
            let texture = ui.ctx().load_texture("app-icon", image, egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("app-icon"), texture.clone()));
            texture
        });
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        ui.add_space(2.0);
        // The name, and the version small and grey on its baseline.
        let name = ui.painter().layout_no_wrap("YTMusic".to_owned(), theme::bold(19.0), theme::TEXT);
        let version = ui.painter().layout_no_wrap(concat!("v", env!("CARGO_PKG_VERSION")).to_owned(), theme::medium(10.5), theme::TEXT_FAINT);
        let size = vec2(name.size().x + 5.0 + version.size().x, name.size().y);
        let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
        let top = rect.center().y - name.size().y / 2.0;
        // Inter's baseline sits about four fifths down its line.
        let baseline = top + name.size().y * 0.8;
        let name_w = name.size().x;
        ui.painter().galley(pos2(rect.left(), top), name, theme::TEXT);
        ui.painter().galley(pos2(rect.left() + name_w + 5.0, baseline - version.size().y * 0.8), version, theme::TEXT_FAINT);
        response.on_hover_text(concat!("Native YTM ", env!("CARGO_PKG_VERSION")));
    });
    ui.add_space(16.0);
    let route = app.route.clone();
    if nav(ui, Icon::Home, "Home", route == Route::Home) {
        app.actions.push(Action::Navigate(Route::Home));
    }
    if nav(ui, Icon::Explore, "Explore", route == Route::Explore) {
        app.actions.push(Action::Navigate(Route::Explore));
    }
    if nav(ui, Icon::Library, "Library", matches!(route, Route::Library(_))) {
        app.actions.push(Action::Navigate(Route::Library(LibraryTab::Playlists)));
    }
    ui.add_space(10.0);
    let divider = ui.available_rect_before_wrap();
    ui.painter().hline(
        divider.left() + 8.0..=divider.right() - 8.0,
        divider.top(),
        egui::Stroke::new(1.0, theme::DIVIDER),
    );
    ui.add_space(12.0);
    if !app.signed_in {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.set_width(theme::SIDEBAR_WIDTH - 40.0);
                ui.label(
                    RichText::new("Sign in to like songs, save music to your library and see your playlists.")
                        .color(theme::TEXT_DIM)
                        .font(theme::regular(13.0)),
                );
                ui.add_space(10.0);
                if widgets::pill(ui, Some(Icon::SignIn), "Sign in", false).clicked() {
                    app.actions.push(Action::OpenSignIn);
                }
            });
        });
        return;
    }
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        if widgets::pill(ui, Some(Icon::Plus), "New playlist", false).clicked() {
            app.new_playlist = Some((String::new(), Vec::new()));
        }
    });
    ui.add_space(10.0);
    let liked = Route::Browse {
        browse_id: "VLLM".into(),
        params: None,
    };
    let current_key = route.key();
    egui::ScrollArea::vertical()
        .id_salt("sidebar-playlists")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if playlist_row(ui, app, "Liked music", "Auto playlist", None, current_key == liked.key(), true) {
                app.actions.push(Action::Navigate(liked.clone()));
            }
            let playlists = app.playlists.clone();
            for item in playlists.iter().filter(|i| i.kind == Kind::Playlist || matches!(&i.target, Some(Target::Browse { browse_id, .. }) if browse_id.starts_with("VL"))) {
                let Some(Target::Browse { browse_id, params }) = &item.target else {
                    continue;
                };
                if browse_id == "VLLM" {
                    continue;
                }
                let route = Route::Browse {
                    browse_id: browse_id.clone(),
                    params: params.clone(),
                };
                let selected = current_key == route.key();
                if playlist_row(ui, app, &item.title, &item.subtitle, item.thumbnail.clone(), selected, false) {
                    app.actions.push(Action::Navigate(route));
                }
            }
            ui.add_space(12.0);
        });
}

fn nav(ui: &mut Ui, icon: Icon, label: &str, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let fill = if selected {
        Color32::from_white_alpha(24)
    } else if response.hovered() {
        Color32::from_white_alpha(14)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    icon.image(theme::TEXT, 22.0)
        .paint_at(ui, Rect::from_min_size(pos2(rect.left() + 14.0, rect.center().y - 11.0), Vec2::splat(22.0)));
    ui.painter().text(
        pos2(rect.left() + 52.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        if selected { theme::semibold(15.0) } else { theme::medium(15.0) },
        theme::TEXT,
    );
    response.clicked()
}

fn playlist_row(
    ui: &mut Ui,
    app: &mut App,
    title: &str,
    subtitle: &str,
    thumbnail: Option<String>,
    selected: bool,
    liked: bool,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return false;
    }
    let fill = if selected {
        Color32::from_white_alpha(24)
    } else if response.hovered() {
        Color32::from_white_alpha(14)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    let art = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 18.0), Vec2::splat(36.0));
    if liked {
        ui.painter().rect_filled(art, CornerRadius::same(4), Color32::from_rgb(0x6a, 0x3d, 0xe8));
        Icon::Liked.image(Color32::WHITE, 18.0).paint_at(ui, Rect::from_center_size(art.center(), Vec2::splat(18.0)));
    } else {
        widgets::paint_art(ui, &mut app.images, thumbnail.as_deref(), art, 4.0, false);
    }
    let x = art.right() + 10.0;
    let w = rect.right() - x - 6.0;
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(x, rect.top() + 7.0), rect.max))
            .layout(Layout::top_down(Align::Min)),
    );
    child.spacing_mut().item_spacing.y = 1.0;
    widgets::clipped_label(&mut child, title, theme::medium(14.0), theme::TEXT, w, Sense::hover());
    widgets::clipped_label(&mut child, subtitle, theme::regular(12.5), theme::TEXT_DIM, w, Sense::hover());
    response.clicked()
}
