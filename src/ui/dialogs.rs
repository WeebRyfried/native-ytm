//! Dialogs over the window: signing in, saving to a playlist, and naming
//! a new one.

use egui::{Align, Color32, CornerRadius, CursorIcon, Id, Layout, Margin, Rect, RichText, Sense, Ui, Vec2, pos2, vec2};

use crate::api::model::Target;
use crate::app::App;
use crate::model::Action;
use crate::theme::{self, Icon};
use crate::ui::widgets;

pub fn show(app: &mut App, ctx: &egui::Context) {
    if app.sign_in.is_some() {
        sign_in(app, ctx);
    }
    if app.picker.is_some() {
        picker(app, ctx);
    }
    if app.new_playlist.is_some() {
        new_playlist(app, ctx);
    }
}

fn frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::POPUP)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(24))
        .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
}

fn sign_in(app: &mut App, ctx: &egui::Context) {
    let mut close = false;
    let mut submit = None;
    let response = egui::Modal::new(Id::new("sign-in"))
        .frame(frame())
        .backdrop_color(theme::SCRIM)
        .show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new("Sign in to YouTube Music").font(theme::bold(22.0)));
            ui.add_space(10.0);
            ui.label(RichText::new(
                "This app signs in with your browser's YouTube Music cookie, the way ytmusicapi does. Nothing leaves this computer except to YouTube.",
            ).color(theme::TEXT_DIM));
            ui.add_space(12.0);
            for (n, step) in [
                "Open music.youtube.com in your browser, signed in.",
                "Press F12, open the Network tab, and filter for \u{201c}browse\u{201d}. Click anything on the page if the list is empty.",
                "Select a request to music.youtube.com, and under Request Headers copy the whole value of \u{201c}cookie\u{201d}. (Copy as cURL, or a cookies.txt export, also works.)",
                "Paste it below.",
            ]
            .iter()
            .enumerate()
            {
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new(format!("{}.", n + 1)).font(theme::semibold(14.0)));
                    ui.add(egui::Label::new(RichText::new(*step).font(theme::regular(14.0))).wrap());
                });
            }
            ui.add_space(12.0);
            let dialog = app.sign_in.as_mut().expect("shown");
            egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut dialog.text)
                        .desired_rows(5)
                        .desired_width(f32::INFINITY)
                        .password(false)
                        .hint_text("SAPISID=...; __Secure-3PSID=...; ...")
                        .font(egui::FontId::monospace(12.0)),
                );
            });
            if let Some(error) = &dialog.error {
                ui.add_space(8.0);
                ui.label(RichText::new(error).color(theme::ACCENT_TEXT));
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new("The cookie is kept encrypted with Windows (DPAPI) for your account only. Signing out of YouTube in that browser ends this sign-in too.")
                    .color(theme::TEXT_FAINT)
                    .font(theme::regular(12.5)),
            );
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let busy = dialog.busy;
                if busy {
                    widgets::spinner_of(ui, 20.0, theme::TEXT);
                } else if widgets::pill(ui, None, "Sign in", true).clicked() {
                    submit = Some(dialog.text.clone());
                }
                if widgets::pill(ui, None, "Cancel", false).clicked() {
                    close = true;
                }
                if widgets::pill(ui, Some(Icon::External), "Open YouTube Music", false).clicked() {
                    crate::platform::open_url("https://music.youtube.com");
                }
            });
        });
    if response.should_close() && !app.sign_in.as_ref().is_some_and(|d| d.busy) {
        close = true;
    }
    if let Some(text) = submit {
        app.actions.push(Action::SignIn(text));
    }
    if close {
        if app.sign_in.as_ref().is_some_and(|d| d.busy) {
            // Cancelled mid-check: forget the cookie being checked.
            app.backend.send(crate::backend::Request::SetAuth(None));
            app.signed_in = false;
        }
        app.sign_in = None;
    }
}

fn picker(app: &mut App, ctx: &egui::Context) {
    let tracks = app.picker.clone().unwrap_or_default();
    let mut close = false;
    let playlists = app.playlists.clone();
    let response = egui::Modal::new(Id::new("playlist-picker"))
        .frame(frame())
        .backdrop_color(theme::SCRIM)
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Save to playlist").font(theme::bold(20.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, Icon::Close, 18.0, theme::TEXT_DIM, "Close").clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(8.0);
            if widgets::pill(ui, Some(Icon::Plus), "New playlist", false).clicked() {
                app.new_playlist = Some((String::new(), tracks.clone()));
                close = true;
            }
            ui.add_space(8.0);
            egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                if playlists.is_empty() {
                    widgets::spinner_of(ui, 20.0, theme::TEXT_DIM);
                }
                for item in playlists.iter() {
                    let Some(Target::Browse { browse_id, .. }) = &item.target else {
                        continue;
                    };
                    if browse_id == "VLLM" || browse_id == "VLSE" {
                        continue;
                    }
                    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
                    let response = response.on_hover_cursor(CursorIcon::PointingHand);
                    if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(6), Color32::from_white_alpha(16));
                    }
                    let art = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 20.0), Vec2::splat(40.0));
                    widgets::paint_art(ui, &mut app.images, item.thumbnail.as_deref(), art, 4.0, false);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(art.right() + 12.0, rect.top() + 8.0), rect.max)));
                    child.spacing_mut().item_spacing.y = 1.0;
                    let w = rect.right() - art.right() - 20.0;
                    widgets::clipped_label(&mut child, &item.title, theme::medium(14.5), theme::TEXT, w, Sense::hover());
                    widgets::clipped_label(&mut child, &item.subtitle, theme::regular(12.5), theme::TEXT_DIM, w, Sense::hover());
                    if response.clicked() {
                        app.actions.push(Action::AddToPlaylist {
                            playlist_id: browse_id.clone(),
                            title: item.title.clone(),
                            tracks: tracks.clone(),
                        });
                    }
                }
            });
        });
    if response.should_close() || close {
        app.picker = None;
    }
}

fn new_playlist(app: &mut App, ctx: &egui::Context) {
    let mut close = false;
    let mut create = None;
    let response = egui::Modal::new(Id::new("new-playlist"))
        .frame(frame())
        .backdrop_color(theme::SCRIM)
        .show(ctx, |ui: &mut Ui| {
            ui.set_width(380.0);
            ui.label(RichText::new("New playlist").font(theme::bold(20.0)));
            ui.add_space(12.0);
            let (title, tracks) = app.new_playlist.as_mut().expect("shown");
            let edit = ui.add(
                egui::TextEdit::singleline(title)
                    .hint_text("Title")
                    .desired_width(f32::INFINITY)
                    .font(theme::regular(15.0)),
            );
            if !edit.has_focus() && title.is_empty() {
                edit.request_focus();
            }
            ui.add_space(6.0);
            ui.label(RichText::new("Private: only you can see it.").color(theme::TEXT_FAINT).font(theme::regular(12.5)));
            ui.add_space(16.0);
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if widgets::pill(ui, None, "Create", true).clicked() || enter {
                    create = Some((title.clone(), tracks.clone()));
                }
                if widgets::pill(ui, None, "Cancel", false).clicked() {
                    close = true;
                }
            });
        });
    if let Some((title, tracks)) = create {
        app.actions.push(Action::CreatePlaylist { title, tracks });
    } else if response.should_close() || close {
        app.new_playlist = None;
    }
}
