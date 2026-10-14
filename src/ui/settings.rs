//! Settings, in plain words: cards of switches and choices, with the
//! technical ones (yt-dlp, application ids, API keys, folders) kept under
//! Advanced.

use egui::{Align, Color32, CornerRadius, CursorIcon, Layout, Margin, Rect, Response, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::app::App;
use crate::model::{Action, LibraryTab, Route};
use crate::settings::Settings;
use crate::theme::{self, Icon};
use crate::ui::widgets;

/// The room a row keeps on its right for its switch, slider or button.
const TOGGLE_W: f32 = 48.0;

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.add_space(8.0);
    ui.label(RichText::new("Settings").font(theme::bold(30.0)));
    let mut draft = app.settings_draft.take().unwrap_or_else(|| app.settings.clone());
    // A column of readable width, however wide the window.
    let width = ui.available_width().min(860.0);
    ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        account(app, ui);
        playback(ui, &mut draft);
        quality(app, ui, &mut draft);
        your_music(app, ui, &mut draft);
        storage(app, ui, &mut draft);
        connections(app, ui, &mut draft);
        appearance(ui, &mut draft);
        updates(app, ui, &mut draft);
        keyboard(ui);
        advanced(app, ui, &mut draft);
        about(ui);
    });

    if draft != app.settings {
        app.actions.push(Action::ApplySettings(Box::new(draft)));
    } else {
        app.settings_draft = None;
    }
}

fn account(app: &mut App, ui: &mut Ui) {
    card(ui, "Account", |ui| {
        if app.signed_in {
            let name = app.account.as_ref().map_or("Your YouTube Music account".to_owned(), |a| match &a.handle {
                Some(handle) => format!("{} \u{00b7} {handle}", a.name),
                None => a.name.clone(),
            });
            let mut sign_out = false;
            row(ui, &name, "Signed in. Your library, likes and playlists are here; the sign-in stays on this PC, encrypted.", 120.0, |ui| {
                sign_out = widgets::pill(ui, Some(Icon::SignOut), "Sign out", false).clicked();
            });
            if sign_out {
                app.actions.push(Action::SignOut);
            }
        } else {
            let mut sign_in = false;
            row(ui, "Not signed in", "Everything plays without an account. Sign in for your library, likes, playlists, history and Premium quality.", 110.0, |ui| {
                sign_in = widgets::pill(ui, Some(Icon::SignIn), "Sign in", true).clicked();
            });
            if sign_in {
                app.actions.push(Action::OpenSignIn);
            }
        }
    });
}

fn playback(ui: &mut Ui, draft: &mut Settings) {
    card(ui, "Playback", |ui| {
        toggle_row(ui, "Keep the music going", "When the queue ends, play similar songs.", &mut draft.autoplay);
        divider(ui);
        toggle_row(ui, "Even out loudness", "Loud songs are turned down to match the rest, as YouTube Music does.", &mut draft.normalize);
        divider(ui);
        toggle_row(ui, "Pick up where you left off", "Open with the last queue, at the same moment of the same song.", &mut draft.restore_session);
        divider(ui);
        toggle_row(ui, "Gapless playback", "Songs follow one another without a pause. Off, a moment of silence between songs.", &mut draft.gapless);
        divider(ui);
        toggle_row(
            ui,
            "Crossfade",
            "Each song fades into the next. Songs of the same album still follow without one, as they were made to.",
            &mut draft.crossfade,
        );
        if draft.crossfade {
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                ui.label(RichText::new("Fade over").color(theme::TEXT_DIM));
                ui.spacing_mut().slider_width = (ui.available_width() - 150.0).clamp(160.0, 420.0);
                ui.add(
                    egui::Slider::new(&mut draft.crossfade_secs, 1.0..=12.0)
                        .step_by(1.0)
                        .suffix(" s")
                        .fixed_decimals(0)
                        .trailing_fill(true),
                );
            });
            ui.add_space(14.0);
        }
    });
}

fn quality(app: &mut App, ui: &mut Ui, draft: &mut Settings) {
    let ytdlp = app.backend.ytdlp_found().is_some();
    let runtime = app.js_runtime_found();
    let playing = app.stream_quality().map(str::to_owned);
    card(ui, "Sound quality", |ui| {
        let detail = "With YouTube Music Premium: 256 kb/s instead of about 130. A song starts at once and switches up mid-song, without a gap.";
        toggle_row(ui, "Premium quality", detail, &mut draft.premium_quality);
        if draft.premium_quality {
            let status = if !app.signed_in {
                Some("Sign in with a Premium account to hear it.".to_owned())
            } else if !ytdlp {
                Some("It needs yt-dlp. Install it with: winget install yt-dlp.yt-dlp".to_owned())
            } else if !runtime {
                Some("yt-dlp needs a JavaScript runtime. Install one with: winget install OpenJS.NodeJS.LTS".to_owned())
            } else {
                None
            };
            if let Some(status) = status {
                note(ui, Icon::Info, &status);
            }
        }
        if let Some(quality) = playing {
            divider(ui);
            row(ui, "Playing now", "The stream heard: its codec and bitrate.", 180.0, |ui| {
                ui.label(RichText::new(quality).font(theme::medium(13.5)).color(theme::TEXT_DIM));
            });
        }
        divider(ui);
        toggle_row(ui, "Synced lyrics", "Lyrics that follow the song, line by line, from LRCLIB, an open lyrics library. Otherwise YouTube Music's own.", &mut draft.synced_lyrics);
        divider(ui);
        toggle_row(
            ui,
            "Readings over Japanese lyrics",
            "In the full-window player, how each word sounds, set small over it: romaji, or furigana over the kanji.",
            &mut draft.lyric_readings,
        );
        if draft.lyric_readings {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for (style, label) in [(crate::readings::Style::Romaji, "Romaji"), (crate::readings::Style::Kana, "Furigana")] {
                    if widgets::chip(ui, label, draft.reading_style == style).clicked() {
                        draft.reading_style = style;
                    }
                }
            });
            ui.add_space(16.0);
        }
        divider(ui);
        toggle_row(ui, "Equalizer", "Shape the sound: more bass, clearer voices, or a curve of your own.", &mut draft.eq);
        if draft.eq {
            equalizer(ui, draft);
        }
    });
}

/// The equaliser's presets, then its ten bands, each a slider.
fn equalizer(ui: &mut Ui, draft: &mut Settings) {
    use crate::player::eq::{BANDS, PRESETS, RANGE};
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (name, gains) in PRESETS {
            if widgets::chip(ui, name, draft.eq_preset == name).clicked() {
                draft.eq_gains = gains;
                draft.eq_preset = name.to_owned();
            }
        }
        if draft.eq_preset == "Custom" {
            widgets::chip(ui, "Custom", true);
        }
    });
    ui.add_space(12.0);
    let mut changed = false;
    let column = ((ui.available_width() - 8.0) / BANDS.len() as f32).max(36.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().slider_width = 120.0;
        for (band, freq) in BANDS.iter().enumerate() {
            ui.allocate_ui_with_layout(vec2(column, 170.0), Layout::top_down(Align::Center), |ui| {
                let gain = &mut draft.eq_gains[band];
                // In a box its own width, so the column centres it.
                let thickness = ui.spacing().interact_size.y;
                let response = ui
                    .allocate_ui_with_layout(vec2(thickness, 124.0), Layout::top_down(Align::Center), |ui| {
                        ui.add(egui::Slider::new(&mut *gain, -RANGE..=RANGE).vertical().step_by(0.5).show_value(false))
                    })
                    .inner;
                if response.double_clicked() {
                    *gain = 0.0;
                    changed = true;
                }
                changed |= response.changed();
                let response = response.on_hover_text(format!("{:+.1} dB", *gain));
                let _ = response;
                let label = if *freq >= 1000.0 { format!("{}k", freq / 1000.0) } else { format!("{freq}") };
                ui.label(RichText::new(label).font(theme::regular(11.5)).color(theme::TEXT_DIM));
            });
        }
    });
    if changed {
        draft.eq_preset = "Custom".into();
    }
    ui.add_space(10.0);
}

/// The folders of the person's own music.
fn your_music(app: &mut App, ui: &mut Ui, draft: &mut Settings) {
    let songs = app.local.songs().len();
    let albums = app.local.albums();
    let scanning = app.local.scanning();
    let picking = app.picking_folder;
    card(ui, "Music on this PC", |ui| {
        let detail = if draft.local_folders.is_empty() {
            "Play your own MP3, FLAC, M4A, Ogg and WAV files beside YouTube Music. Their songs show in Library, On this PC, and come first in search. Add a folder here, or drop one on the window. Nothing leaves this PC.".to_owned()
        } else {
            match scanning {
                Some(seen) => format!("Looking through your folders\u{2026} {seen} files so far."),
                None => format!(
                    "{} in {}, in Library, On this PC, and first in search.",
                    widgets::songs(songs),
                    if albums == 1 { "1 album".to_owned() } else { format!("{albums} albums") }
                ),
            }
        };
        let (mut add, mut rescan) = (false, false);
        row(ui, "Your folders", &detail, 230.0, |ui| {
            add = widgets::pill(ui, Some(Icon::FolderPlus), if picking { "Choosing\u{2026}" } else { "Add a folder" }, draft.local_folders.is_empty()).clicked();
            if !draft.local_folders.is_empty() {
                rescan = widgets::icon_button(ui, Icon::Refresh, 18.0, theme::TEXT_DIM, "Look through the folders again").clicked();
            }
        });
        if add {
            app.actions.push(Action::AddLocalFolder);
        }
        if rescan {
            app.actions.push(Action::RescanLocal);
        }
        let mut gone = None;
        for folder in &draft.local_folders {
            divider(ui);
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
            Icon::Folder
                .image(theme::TEXT_DIM, 18.0)
                .paint_at(ui, Rect::from_center_size(pos2(rect.left() + 9.0, rect.center().y), Vec2::splat(18.0)));
            let text_w = rect.width() - 34.0 - 90.0;
            let mut label = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(pos2(rect.left() + 32.0, rect.top()), pos2(rect.left() + 32.0 + text_w, rect.bottom())))
                    .layout(Layout::left_to_right(Align::Center)),
            );
            widgets::clipped_label(&mut label, folder, theme::regular(14.0), theme::TEXT, text_w, Sense::hover());
            let mut buttons = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(pos2(rect.right() - 90.0, rect.top()), rect.max))
                    .layout(Layout::right_to_left(Align::Center)),
            );
            buttons.spacing_mut().item_spacing.x = 2.0;
            if widgets::icon_button(&mut buttons, Icon::Close, 16.0, theme::TEXT_DIM, "Stop playing music from this folder").clicked() {
                gone = Some(folder.clone());
            }
            if widgets::icon_button(&mut buttons, Icon::External, 16.0, theme::TEXT_DIM, "Open in Explorer").clicked() {
                crate::platform::open_folder(std::path::Path::new(folder));
            }
            let _ = response;
        }
        if let Some(folder) = gone {
            draft.local_folders.retain(|f| *f != folder);
        }
        if !draft.local_folders.is_empty() {
            ui.add_space(6.0);
        }
    });
}

/// Whether a newer version is out, and moving to it in one click.
fn updates(app: &mut App, ui: &mut Ui, draft: &mut Settings) {
    use crate::update::State;
    let state = app.updater.state();
    card(ui, "Updates", |ui| {
        let version = env!("CARGO_PKG_VERSION");
        let (title, detail) = match &state {
            State::Idle => (format!("Native YTM {version}"), "Look for a newer version on GitHub.".to_owned()),
            State::Checking => (format!("Native YTM {version}"), "Looking for a newer version\u{2026}".to_owned()),
            State::UpToDate => (format!("Native YTM {version}"), "You have the latest version.".to_owned()),
            State::Available(release) => (
                format!("Version {} is out", release.version),
                format!(
                    "{}{}Updating takes a few seconds: the app restarts and the music carries on where it was.",
                    release.summary,
                    if release.summary.is_empty() { "" } else { " " }
                ),
            ),
            State::Downloading(release, done) => (format!("Updating to {}", release.version), format!("Downloading\u{2026} {:.0}%", done * 100.0)),
            State::Ready(_) => ("Restarting\u{2026}".to_owned(), "The new version is starting.".to_owned()),
            State::Failed(error) => (format!("Native YTM {version}"), error.clone()),
        };
        let mut clicked = None;
        row(ui, &title, &detail, 250.0, |ui| match &state {
            State::Available(release) => {
                if widgets::pill(ui, Some(Icon::Update), "Update now", true).clicked() {
                    clicked = Some(Action::InstallUpdate);
                }
                if widgets::icon_button(ui, Icon::External, 16.0, theme::TEXT_DIM, "What's new, on GitHub").clicked() {
                    clicked = Some(Action::OpenUrl(release.page.clone()));
                }
            }
            State::Checking | State::Downloading(..) | State::Ready(_) => widgets::spinner_of(ui, 18.0, theme::TEXT_DIM),
            _ => {
                if widgets::pill(ui, Some(Icon::Refresh), "Check for updates", false).clicked() {
                    clicked = Some(Action::CheckForUpdates);
                }
            }
        });
        if let State::Downloading(_, done) = state {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(2), theme::SURFACE_ACTIVE);
            let filled = Rect::from_min_size(rect.min, vec2(rect.width() * done.clamp(0.0, 1.0), rect.height()));
            ui.painter().rect_filled(filled, CornerRadius::same(2), theme::ACCENT);
            ui.add_space(14.0);
            theme::repaint_in(ui.ctx(), std::time::Duration::from_millis(120));
        }
        if let Some(action) = clicked {
            app.actions.push(action);
        }
        divider(ui);
        toggle_row(ui, "Look for updates on its own", "Now and then, and a few seconds after the app starts. Nothing downloads until you choose Update.", &mut draft.check_updates);
    });
}

fn storage(app: &mut App, ui: &mut Ui, draft: &mut Settings) {
    let usage = app.audio_cache.usage();
    let artwork = app.artwork_bytes();
    card(ui, "Downloads and storage", |ui| {
        let detail = if usage.saved == 0 {
            "Nothing yet. Download a song, an album or a playlist from its menu or its page.".to_owned()
        } else {
            format!("{}, {}. They play without the network.", widgets::songs(usage.saved), megabytes(usage.saved_bytes))
        };
        let mut open = false;
        row(ui, "Downloaded songs", &detail, 120.0, |ui| {
            open = widgets::pill(ui, Some(Icon::CircleCheck), "Open", false).clicked();
        });
        if open {
            app.actions.push(Action::Navigate(Route::Library(LibraryTab::Cached)));
        }
        divider(ui);
        let detail = format!(
            "Songs you play are kept on this PC, so they start at once and play offline too; the oldest make way. Now {}, {}.",
            widgets::songs(usage.kept),
            megabytes(usage.kept_bytes)
        );
        let mut clear = false;
        row(ui, "Recently played songs", &detail, 120.0, |ui| {
            clear = widgets::pill(ui, Some(Icon::Trash), "Clear", false).clicked();
        });
        if clear {
            app.actions.push(Action::ClearSongCache);
        }
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            ui.label(RichText::new("Keep up to").color(theme::TEXT_DIM));
            ui.spacing_mut().slider_width = (ui.available_width() - 150.0).clamp(160.0, 420.0);
            ui.add(egui::Slider::new(&mut draft.cached_songs, 0..=1000).step_by(10.0).suffix(" songs").trailing_fill(true));
        });
        ui.add_space(8.0);
        divider(ui);
        let mut clear = false;
        row(ui, "Artwork", &format!("Album art saved for quick loading: {}.", megabytes(artwork)), 120.0, |ui| {
            clear = widgets::pill(ui, Some(Icon::Trash), "Clear", false).clicked();
        });
        if clear {
            app.actions.push(Action::ClearImageCache);
        }
    });
}

fn connections(app: &mut App, ui: &mut Ui, draft: &mut Settings) {
    use crate::lastfm::Status;
    card(ui, "Sharing", |ui| {
        toggle_row(
            ui,
            "Show it on Discord",
            "Your Discord status reads \u{201c}Listening to\u{201d} the artist, with the artwork, the song and a progress bar. Nothing while paused. Needs the Discord app.",
            &mut draft.discord,
        );
        if app.signed_in {
            divider(ui);
            toggle_row(ui, "Add songs to your YouTube history", "Songs you play show in your YouTube Music history and shape its recommendations, as on the web.", &mut draft.history);
        }
        divider(ui);
        match app.lastfm.status() {
            Status::Connected => {
                let user = app.lastfm.stored.user.clone().unwrap_or_else(|| "your account".into());
                let mut disconnect = false;
                let waiting = app.lastfm.waiting();
                let detail = if waiting > 0 {
                    format!("Connected as {user}. {waiting} plays wait to be sent.")
                } else {
                    format!("Connected as {user}. The player shows how often you've played the song and the artist.")
                };
                row(ui, "Last.fm", &detail, 130.0, |ui| {
                    disconnect = widgets::pill(ui, Some(Icon::SignOut), "Disconnect", false).clicked();
                });
                if disconnect {
                    app.actions.push(Action::LastFmDisconnect);
                }
                divider(ui);
                toggle_row(ui, "Scrobble", "Send each song you play to Last.fm, once you've heard half of it.", &mut draft.scrobble);
            }
            Status::Authorising => {
                row(ui, "Last.fm", "Waiting for you to allow access on Last.fm, in your browser\u{2026}", 40.0, |ui| {
                    widgets::spinner_of(ui, 18.0, theme::TEXT_DIM);
                });
            }
            Status::Disconnected => {
                let mut connect = false;
                let ready = !app.lastfm_form.0.trim().is_empty() && !app.lastfm_form.1.trim().is_empty();
                row(ui, "Last.fm", "Scrobble what you play, and see your play counts in the player. Connecting takes a free Last.fm API account: see Advanced, below.", 120.0, |ui| {
                    connect = widgets::pill(ui, None, "Connect", ready).clicked();
                });
                if connect {
                    if ready {
                        app.actions.push(Action::LastFmConnect);
                    } else {
                        ui.data_mut(|d| d.insert_temp(egui::Id::new("settings-advanced"), true));
                        app.toast("Enter the Last.fm API key and shared secret under Advanced first", crate::model::ToastKind::Info);
                    }
                }
            }
        }
    });
}

fn appearance(ui: &mut Ui, draft: &mut Settings) {
    card(ui, "Window", |ui| {
        toggle_row(
            ui,
            "Titles in Latin letters",
            "Japanese, Chinese and Korean titles as YouTube Music's player shows them: \u{201c}Tsubasa\u{201d} rather than \u{201c}ツバサ - Tsubasa\u{201d}. The original shows when you point at one.",
            &mut draft.romanized_titles,
        );
        divider(ui);
        toggle_row(ui, "Keep playing when closed", "Closing the window leaves the music playing, from the icon in the taskbar's corner.", &mut draft.close_to_tray);
        divider(ui);
        row(ui, "Interface size", "Everything larger or smaller.", 230.0, |ui| {
            ui.spacing_mut().slider_width = 150.0;
            ui.add(
                egui::Slider::new(&mut draft.zoom, 0.8..=1.6)
                    .step_by(0.05)
                    .trailing_fill(true)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            );
        });
        divider(ui);
        row(ui, "Language", "The language YouTube Music answers in: en, de, ja, ms\u{2026}", 120.0, |ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.language).desired_width(90.0).hint_text("en"));
        });
        divider(ui);
        row(ui, "Region", "Whose charts and suggestions: two letters, such as US or MY. Empty follows your connection.", 120.0, |ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.region).desired_width(90.0).hint_text("auto"));
        });
    });
}

fn keyboard(ui: &mut Ui) {
    let open = expander(ui, "settings-keys", "Keyboard shortcuts");
    if !open {
        return;
    }
    card_body(ui, |ui| {
        for (i, (keys, what)) in [
            ("Space", "Play or pause"),
            ("Ctrl + Right / Left", "Next / previous song"),
            ("Shift + Right / Left", "Forward / back 10 seconds"),
            ("Ctrl + Up / Down", "Volume"),
            ("M", "Mute"),
            ("F, Esc", "Open, close the full-window player"),
            ("F11", "Full screen"),
            ("Q", "Show the queue"),
            ("L", "Like the song"),
            ("Ctrl + F or /", "Search"),
            ("Alt + Left / Right", "Back / forward"),
        ]
        .into_iter()
        .enumerate()
        {
            if i > 0 {
                ui.add_space(2.0);
            }
            ui.horizontal(|ui| {
                ui.add_sized([190.0, 24.0], egui::Label::new(RichText::new(keys).font(theme::medium(13.5))));
                ui.label(RichText::new(what).color(theme::TEXT_DIM));
            });
        }
        ui.add_space(6.0);
    });
}

fn advanced(app: &mut App, ui: &mut Ui, draft: &mut Settings) {
    let open = expander(ui, "settings-advanced", "Advanced");
    if !open {
        return;
    }
    let found = app.backend.ytdlp_found();
    card_body(ui, |ui| {
        toggle_row(
            ui,
            "Fall back to yt-dlp",
            "When YouTube refuses the app's own stream request, have yt-dlp find the stream instead.",
            &mut draft.use_ytdlp,
        );
        row(ui, "yt-dlp", &match &found {
            Some(path) => format!("Using {}", path.display()),
            None => "Not found. Install it (winget install yt-dlp.yt-dlp) or give its path.".to_owned(),
        }, 260.0, |ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.ytdlp_path).desired_width(240.0).hint_text("found on PATH"));
        });
        divider(ui);
        row(ui, "Discord application", "Another Discord application's id, whose name your status then shows. Empty: YouTube Music.", 260.0, |ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.discord_client_id).desired_width(240.0).hint_text("YouTube Music"));
        });
        if app.lastfm.status() == crate::lastfm::Status::Disconnected {
            divider(ui);
            let mut create = false;
            row(ui, "Last.fm API account", "Last.fm asks each app user for an API account of their own: create one (any name will do), paste its key and secret here, then Connect under Sharing. You allow access on last.fm; your password never passes through the app.", 130.0, |ui| {
                create = widgets::pill(ui, Some(Icon::External), "Create one", false).clicked();
            });
            if create {
                app.actions.push(Action::OpenUrl("https://www.last.fm/api/account/create".into()));
            }
            let (key, secret) = &mut app.lastfm_form;
            ui.horizontal(|ui| {
                ui.add_sized([110.0, 24.0], egui::Label::new(RichText::new("API key").color(theme::TEXT_DIM)));
                ui.add(egui::TextEdit::singleline(key).desired_width(320.0));
            });
            ui.horizontal(|ui| {
                ui.add_sized([110.0, 24.0], egui::Label::new(RichText::new("Shared secret").color(theme::TEXT_DIM)));
                ui.add(egui::TextEdit::singleline(secret).desired_width(320.0).password(true));
            });
            ui.add_space(10.0);
        }
        divider(ui);
        let mut open_folder = false;
        row(ui, "Where things are kept", &format!("Settings in {}; songs and artwork in {}.", app.paths.config.display(), app.paths.cache.display()), 130.0, |ui| {
            open_folder = widgets::pill(ui, None, "Open folder", false).clicked();
        });
        if open_folder {
            crate::platform::open_folder(&app.paths.config);
        }
        divider(ui);
        row(ui, "Connection", if app.online() { "Online." } else { "Offline: downloaded songs play; the rest waits for the connection." }, 40.0, |_| {});
    });
}

fn about(ui: &mut Ui) {
    ui.add_space(28.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(format!("Native YTM {}", env!("CARGO_PKG_VERSION"))).font(theme::medium(13.5)).color(theme::TEXT_DIM));
        ui.label(
            RichText::new("A lightweight YouTube Music player for Windows. Not affiliated with Google or YouTube.")
                .font(theme::regular(12.5))
                .color(theme::TEXT_FAINT),
        );
    });
}

// ---- Pieces -----------------------------------------------------------------

/// A titled card of rows.
fn card(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.add_space(26.0);
    ui.label(RichText::new(title).font(theme::semibold(15.0)).color(theme::TEXT_DIM));
    ui.add_space(8.0);
    card_body(ui, add);
}

fn card_body(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(20, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            // Fields and slider rails in the card's own colour would vanish:
            // darker fields, lighter rails.
            let visuals = ui.visuals_mut();
            visuals.extreme_bg_color = theme::BG;
            for state in [&mut visuals.widgets.inactive, &mut visuals.widgets.hovered, &mut visuals.widgets.active] {
                state.bg_fill = theme::SURFACE_ACTIVE;
            }
            visuals.widgets.inactive.weak_bg_fill = theme::SURFACE_ACTIVE;
            add(ui);
        });
}

/// A heading that opens and closes what follows, remembered.
fn expander(ui: &mut Ui, id: &str, title: &str) -> bool {
    let id = egui::Id::new(id);
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    ui.add_space(26.0);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if response.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    let color = if response.hovered() { theme::TEXT } else { theme::TEXT_DIM };
    let galley = ui.painter().layout_no_wrap(title.to_owned(), theme::semibold(15.0), color);
    let y = rect.center().y - galley.size().y / 2.0;
    let text_w = galley.size().x;
    ui.painter().galley(pos2(rect.left(), y), galley, color);
    let icon = if open { Icon::ChevronUp } else { Icon::ChevronDown };
    icon.image(color, 16.0)
        .paint_at(ui, Rect::from_center_size(pos2(rect.left() + text_w + 14.0, rect.center().y), Vec2::splat(16.0)));
    if open {
        ui.add_space(8.0);
    }
    open
}

/// One setting: its name and a line on what it does on the left, its
/// control on the right, in `control_w`.
fn row(ui: &mut Ui, title: &str, detail: &str, control_w: f32, control: impl FnOnce(&mut Ui)) -> Response {
    let width = ui.available_width();
    let text_w = (width - control_w - 24.0).max(160.0);
    let title_g = ui.painter().layout(title.to_owned(), theme::medium(14.5), theme::TEXT, text_w);
    let detail_g = (!detail.is_empty()).then(|| ui.painter().layout(detail.to_owned(), theme::regular(12.5), theme::TEXT_DIM, text_w));
    let text_h = title_g.size().y + detail_g.as_ref().map_or(0.0, |g| 3.0 + g.size().y);
    let height = text_h.max(28.0) + 28.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let mut y = rect.center().y - text_h / 2.0;
    let title_h = title_g.size().y;
    ui.painter().galley(pos2(rect.left(), y), title_g, theme::TEXT);
    y += title_h + 3.0;
    if let Some(detail) = detail_g {
        ui.painter().galley(pos2(rect.left(), y), detail, theme::TEXT_DIM);
    }
    let control_rect = Rect::from_min_max(pos2(rect.right() - control_w, rect.top()), rect.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(control_rect).layout(Layout::right_to_left(Align::Center)));
    control(&mut child);
    response
}

/// A setting that is on or off; the whole row flips it.
fn toggle_row(ui: &mut Ui, title: &str, detail: &str, on: &mut bool) {
    let mut flipped = false;
    let response = row(ui, title, detail, TOGGLE_W, |ui| {
        flipped = switch(ui, *on).clicked();
    });
    if flipped || response.clicked() {
        *on = !*on;
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
}

/// A switch, sliding and colouring as it turns on.
fn switch(ui: &mut Ui, on: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let t = ui.ctx().animate_bool_with_time(response.id, on, 0.14);
    let off = theme::SURFACE_ACTIVE;
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    let fill = Color32::from_rgb(mix(off.r(), theme::ACCENT.r()), mix(off.g(), theme::ACCENT.g()), mix(off.b(), theme::ACCENT.b()));
    ui.painter().rect_filled(rect, CornerRadius::same(11), fill);
    if response.hovered() {
        ui.painter().rect_stroke(rect, CornerRadius::same(11), Stroke::new(1.0, Color32::from_white_alpha(40)), egui::StrokeKind::Outside);
    }
    let x = rect.left() + 11.0 + (rect.width() - 22.0) * t;
    ui.painter().circle_filled(pos2(x, rect.center().y), 8.0, Color32::WHITE);
    response
}

/// A line under a setting: what it needs, or what it is doing.
fn note(ui: &mut Ui, icon: Icon, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        icon.image(theme::ACCENT_TEXT, 15.0).paint_at(ui, rect);
        ui.label(RichText::new(text).font(theme::regular(12.5)).color(theme::TEXT_DIM));
    });
    ui.add_space(14.0);
}

fn divider(ui: &mut Ui) {
    let rect = ui.max_rect();
    let y = ui.cursor().top();
    ui.painter().hline(rect.x_range(), y, Stroke::new(1.0, theme::DIVIDER));
}

fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{mb:.0} MB") }
}
