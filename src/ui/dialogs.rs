//! Dialogs over the window: signing in, saving to a playlist, naming a new
//! one, renaming one, making sure before deleting one, and what is new.

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
    if app.rename_playlist.is_some() {
        rename_playlist(app, ctx);
    }
    if app.delete_playlist.is_some() {
        delete_playlist(app, ctx);
    }
    if app.whats_new.is_some() {
        whats_new(app, ctx);
    }
    if app.browser_question_shown() {
        browser_question(app, ctx);
    }
}

/// What changed: after an update, in each version since the one before;
/// from Settings, in every version.
fn whats_new(app: &mut App, ctx: &egui::Context) {
    let mut close = false;
    let mut releases_page = false;
    let shown = app.whats_new.as_ref().expect("shown");
    let height = (ctx.content_rect().height() * 0.6).clamp(200.0, 520.0);
    let response = egui::Modal::new(Id::new("whats-new"))
        .frame(frame())
        .backdrop_color(theme::SCRIM)
        .show(ctx, |ui: &mut Ui| {
            ui.set_width(500.0);
            ui.label(RichText::new("What's new").font(theme::bold(22.0)));
            if shown.after_update {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("Native YTM has been updated to {}.", env!("CARGO_PKG_VERSION"))).color(theme::text_dim()));
            }
            ui.add_space(16.0);
            egui::ScrollArea::vertical().max_height(height).auto_shrink([false, true]).show(ui, |ui| {
                for (n, release) in shown.releases.iter().enumerate() {
                    if n > 0 {
                        ui.add_space(14.0);
                        let y = ui.cursor().top();
                        ui.painter().hline(ui.max_rect().x_range(), y, egui::Stroke::new(1.0, theme::divider()));
                        ui.add_space(16.0);
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.label(RichText::new(&release.version).font(theme::semibold(13.5)).color(theme::accent_text()));
                        ui.label(RichText::new(&release.date).font(theme::regular(12.5)).color(theme::text_faint()));
                    });
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(RichText::new(&release.summary).font(theme::semibold(16.0)).color(theme::text())).wrap());
                    ui.add_space(8.0);
                    for change in &release.changes {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            let (dot, _) = ui.allocate_exact_size(vec2(16.0, 20.0), Sense::hover());
                            ui.painter().circle_filled(pos2(dot.left() + 4.0, dot.top() + 10.0), 2.2, theme::text_faint());
                            let mut job = egui::text::LayoutJob::default();
                            for (piece, bold) in crate::changelog::pieces(change) {
                                let (font, color) = if bold { (theme::semibold(14.0), theme::text()) } else { (theme::regular(14.0), theme::text_dim()) };
                                job.append(piece, 0.0, egui::TextFormat { font_id: font, color, line_height: Some(20.0), ..Default::default() });
                            }
                            ui.add(egui::Label::new(job).wrap());
                        });
                        ui.add_space(6.0);
                    }
                }
            });
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if widgets::pill(ui, None, "Done", true).clicked() {
                    close = true;
                }
                if widgets::pill(ui, Some(Icon::External), "Every release", false).clicked() {
                    releases_page = true;
                }
            });
        });
    if releases_page {
        app.actions.push(Action::OpenUrl(format!("https://github.com/{}/releases", crate::update::REPO)));
    }
    if close || response.should_close() {
        app.whats_new = None;
    }
}

/// A playlist of the account's own, renamed.
fn rename_playlist(app: &mut App, ctx: &egui::Context) {
    let mut close = false;
    let mut rename = None;
    let response = egui::Modal::new(Id::new("rename-playlist"))
        .frame(frame())
        .backdrop_color(theme::SCRIM)
        .show(ctx, |ui: &mut Ui| {
            ui.set_width(380.0);
            ui.label(RichText::new("Rename playlist").font(theme::bold(20.0)));
            ui.add_space(12.0);
            let (id, title) = app.rename_playlist.as_mut().expect("shown");
            let edit = ui.add(
                egui::TextEdit::singleline(title)
                    .hint_text("Title")
                    .desired_width(f32::INFINITY)
                    .font(theme::regular(15.0)),
            );
            // Focused, the name chosen, as a rename box does.
            let focus_id = Id::new("rename-playlist-focused");
            if !ui.data(|d| d.get_temp::<bool>(focus_id).unwrap_or(false)) {
                edit.request_focus();
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), edit.id) {
                    let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(title.chars().count()));
                    state.cursor.set_char_range(Some(all));
                    state.store(ui.ctx(), edit.id);
                }
                ui.data_mut(|d| d.insert_temp(focus_id, true));
            }
            ui.add_space(16.0);
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let ready = !title.trim().is_empty();
                if (widgets::pill(ui, None, "Rename", ready).clicked() || enter) && ready {
                    rename = Some((id.clone(), title.clone()));
                }
                if widgets::pill(ui, None, "Cancel", false).clicked() {
                    close = true;
                }
            });
        });
    if let Some((playlist_id, title)) = rename {
        app.actions.push(Action::RenamePlaylist { playlist_id, title });
        ctx.data_mut(|d| d.remove::<bool>(Id::new("rename-playlist-focused")));
    } else if response.should_close() || close {
        app.rename_playlist = None;
        ctx.data_mut(|d| d.remove::<bool>(Id::new("rename-playlist-focused")));
    }
}

/// Deleting a playlist of the account's own, once sure.
fn delete_playlist(app: &mut App, ctx: &egui::Context) {
    let mut close = false;
    let mut delete = false;
    let (id, title) = app.delete_playlist.clone().expect("shown");
    let response = egui::Modal::new(Id::new("delete-playlist"))
        .frame(frame())
        .backdrop_color(theme::SCRIM)
        .show(ctx, |ui: &mut Ui| {
            ui.set_width(400.0);
            ui.label(RichText::new("Delete playlist?").font(theme::bold(20.0)));
            ui.add_space(10.0);
            ui.add(
                egui::Label::new(
                    RichText::new(format!("\u{201c}{title}\u{201d} goes from your YouTube Music account, on every device. This cannot be undone."))
                        .color(theme::text_dim()),
                )
                .wrap(),
            );
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if widgets::pill(ui, Some(Icon::Trash), "Delete", true).clicked() {
                    delete = true;
                }
                if widgets::pill(ui, None, "Cancel", false).clicked() {
                    close = true;
                }
            });
        });
    if delete {
        app.actions.push(Action::DeletePlaylist { playlist_id: id, title });
    } else if response.should_close() || close {
        app.delete_playlist = None;
    }
}

fn frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::popup())
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(24))
        .stroke(egui::Stroke::new(1.0, theme::divider()))
}

fn sign_in(app: &mut App, ctx: &egui::Context) {
    if app.sign_in.as_ref().is_some_and(|d| d.silent) {
        return;
    }
    let mut close = false;
    let mut submit = None;
    let mut from_browser = None;
    let mut install = false;
    let ytdlp = app.backend.ytdlp_found().is_some();
    let installing = app.tools.job == Some(crate::app::Job::Installing);
    let browsers = crate::ytdlp::installed_browsers();
    let response = egui::Modal::new(Id::new("sign-in"))
        .frame(frame())
        .backdrop_color(theme::SCRIM)
        .show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(RichText::new("Sign in to YouTube Music").font(theme::bold(22.0)));
            ui.add_space(8.0);
            ui.label(RichText::new("Sign in at music.youtube.com in your browser, then bring that sign-in here.").color(theme::text_dim()));
            ui.add_space(4.0);
            let open = ui.add(egui::Label::new(RichText::new("Open music.youtube.com").color(theme::accent_text()).font(theme::regular(13.5))).sense(Sense::click()));
            if open.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                crate::platform::open_url("https://music.youtube.com");
            }
            ui.add_space(16.0);
            let dialog = app.sign_in.as_mut().expect("shown");
            if !dialog.paste {
                // Which browser: each tried in turn, or one.
                let mut chosen = app.settings.sign_in_browser.clone();
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    if widgets::chip(ui, "Any browser", chosen.is_empty()).clicked() {
                        chosen.clear();
                    }
                    for id in &browsers {
                        if widgets::chip(ui, crate::ytdlp::browser_name(id), chosen == *id).clicked() {
                            chosen = (*id).to_owned();
                        }
                    }
                });
                if chosen != app.settings.sign_in_browser {
                    let mut settings = app.settings.clone();
                    settings.sign_in_browser = chosen;
                    app.actions.push(Action::ApplySettings(Box::new(settings)));
                }
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Chrome and Edge lock their sign-in away from other apps. Firefox works, or paste a cookie.")
                        .color(theme::text_faint())
                        .font(theme::regular(12.5)),
                );
            } else {
                ui.add(
                    egui::Label::new(
                        RichText::new("In the browser's developer tools (F12, Network), copy the cookie header of a music.youtube.com request and paste it here.")
                            .color(theme::text_dim())
                            .font(theme::regular(13.0)),
                    )
                    .wrap(),
                );
                ui.add_space(8.0);
                egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut dialog.text)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY)
                            .hint_text("SAPISID=...; __Secure-3PSID=...; ...")
                            .font(egui::FontId::monospace(12.0)),
                    );
                });
            }
            if let Some(error) = &dialog.error {
                ui.add_space(10.0);
                ui.add(egui::Label::new(RichText::new(error).color(theme::accent_text())).wrap());
            }
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                let toggle = if dialog.paste { "Get it from a browser instead" } else { "Paste a cookie instead" };
                let link = ui.add(egui::Label::new(RichText::new(toggle).color(theme::text_dim()).font(theme::regular(13.0))).sense(Sense::click()));
                if link.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    dialog.paste = !dialog.paste;
                    dialog.error = None;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if dialog.busy || installing {
                        widgets::spinner_of(ui, 20.0, theme::text());
                    } else if dialog.paste {
                        if widgets::pill(ui, None, "Sign in", true).clicked() {
                            submit = Some(dialog.text.clone());
                        }
                    } else if !ytdlp {
                        if widgets::pill(ui, Some(Icon::Download), "Install yt-dlp, sign in", true).clicked() {
                            install = true;
                        }
                    } else if widgets::pill(ui, None, "Sign in", true).clicked() {
                        from_browser = Some(app.settings.sign_in_browser.clone());
                    }
                    if widgets::pill(ui, None, "Cancel", false).clicked() {
                        close = true;
                    }
                });
            });
        });
    if response.should_close() && !app.sign_in.as_ref().is_some_and(|d| d.busy) {
        close = true;
    }
    if let Some(text) = submit {
        app.actions.push(Action::SignIn(text));
    }
    if let Some(browser) = from_browser {
        app.actions.push(Action::SignInFromBrowser { browser: (!browser.is_empty()).then_some(browser), auto: false });
    }
    if install {
        app.actions.push(Action::SignInFromBrowser { browser: None, auto: false });
    }
    if close {
        if app.sign_in.as_ref().is_some_and(|d| d.busy) {
            // Cancelled mid-check: forget the cookie being checked, and the
            // browser's answer should it still come.
            app.backend.send(crate::backend::Request::SetAuth(None));
            app.signed_in = false;
            app.tools.cancel_sign_in();
        }
        app.sign_in = None;
    }
}

/// On the first start: whether to sign in from the browser by itself.
fn browser_question(app: &mut App, ctx: &egui::Context) {
    let mut answer = None;
    egui::Modal::new(Id::new("browser-question"))
        .frame(frame())
        .backdrop_color(theme::SCRIM)
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("Sign in from your browser?").font(theme::bold(20.0)));
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    RichText::new("Get your YouTube Music sign-in from your browser automatically, now and whenever you're signed out.")
                        .color(theme::text_dim()),
                )
                .wrap(),
            );
            if app.backend.ytdlp_found().is_none() {
                ui.add_space(6.0);
                ui.label(RichText::new("This installs yt-dlp (about 24 MB).").color(theme::text_faint()).font(theme::regular(12.5)));
            }
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if widgets::pill(ui, None, "Yes", true).clicked() {
                    answer = Some(true);
                }
                if widgets::pill(ui, None, "No thanks", false).clicked() {
                    answer = Some(false);
                }
            });
        });
    if let Some(yes) = answer {
        app.actions.push(Action::AnswerBrowserSignIn(yes));
    }
}

/// The offer to install yt-dlp, small in the corner over the player bar,
/// until it is installed or put away.
pub fn ytdlp_prompt(app: &mut App, ctx: &egui::Context) {
    if !app.ytdlp_prompt_shown() {
        return;
    }
    let installing = app.tools.job == Some(crate::app::Job::Installing);
    let screen = ctx.content_rect();
    let width = 300.0;
    let shown = theme::appear(ctx, Id::new("ytdlp-prompt"), 1, 0.3);
    let mut action = None;
    egui::Area::new(Id::new("ytdlp-prompt-area"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -(theme::PLAYER_HEIGHT + 16.0) + (1.0 - shown) * 12.0))
        .show(ctx, |ui| {
            ui.multiply_opacity(shown);
            egui::Frame::new()
                .fill(theme::popup())
                .corner_radius(CornerRadius::same(10))
                .stroke(egui::Stroke::new(1.0, theme::divider()))
                .inner_margin(Margin::symmetric(14, 12))
                .shadow(egui::Shadow { offset: [0, 4], blur: 18, spread: 0, color: Color32::from_black_alpha(110) })
                .show(ui, |ui| {
                    ui.set_width(width - 28.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Get yt-dlp").font(theme::semibold(14.0)));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::icon_button(ui, Icon::Close, 14.0, theme::text_dim(), "Don't show this again").clicked() {
                                action = Some(Action::HideYtDlpPrompt);
                            }
                        });
                    });
                    ui.label(RichText::new("For browser sign-in and Premium quality.").color(theme::text_dim()).font(theme::regular(12.5)));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if installing {
                            widgets::spinner_of(ui, 16.0, theme::text_dim());
                            ui.label(RichText::new("Installing\u{2026}").color(theme::text_dim()).font(theme::regular(12.5)));
                        } else if widgets::pill(ui, Some(Icon::Download), "Install", true).clicked() {
                            action = Some(Action::InstallYtDlp);
                        }
                    });
                });
        });
    let _ = screen;
    if let Some(action) = action {
        app.actions.push(action);
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
                    if widgets::icon_button(ui, Icon::Close, 18.0, theme::text_dim(), "Close").clicked() {
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
                    widgets::spinner_of(ui, 20.0, theme::text_dim());
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
                        widgets::hover_glow(ui, rect, 6.0, 1.0);
                    }
                    let art = Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 20.0), Vec2::splat(40.0));
                    widgets::paint_art(ui, &mut app.images, item.thumbnail.as_deref(), art, 4.0, false);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(art.right() + 12.0, rect.top() + 8.0), rect.max)));
                    child.spacing_mut().item_spacing.y = 1.0;
                    let w = rect.right() - art.right() - 20.0;
                    widgets::clipped_label(&mut child, &item.title, theme::medium(14.5), theme::text(), w, Sense::hover());
                    widgets::clipped_label(&mut child, &item.subtitle, theme::regular(12.5), theme::text_dim(), w, Sense::hover());
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
            ui.label(RichText::new("Private: only you can see it.").color(theme::text_faint()).font(theme::regular(12.5)));
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
