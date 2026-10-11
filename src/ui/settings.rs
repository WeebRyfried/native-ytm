//! Settings: playback, the account, the stream source, and the window.

use egui::{RichText, Ui};

use crate::app::App;
use crate::model::Action;
use crate::theme::{self, Icon};
use crate::ui::widgets;

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.add_space(8.0);
    ui.label(RichText::new("Settings").font(theme::bold(30.0)));
    ui.add_space(20.0);
    let mut draft = app.settings_draft.take().unwrap_or_else(|| app.settings.clone());
    let width = ui.available_width().min(720.0);
    ui.set_max_width(width);

    group(ui, "Account");
    if app.signed_in {
        let name = app.account.as_ref().map_or("Signed in".to_owned(), |a| {
            match &a.handle {
                Some(handle) => format!("{} ({handle})", a.name),
                None => a.name.clone(),
            }
        });
        ui.label(RichText::new(name).font(theme::medium(15.0)));
        ui.label(RichText::new("Your sign-in is stored encrypted for this Windows account.").color(theme::TEXT_DIM));
        ui.add_space(6.0);
        if widgets::pill(ui, Some(Icon::SignOut), "Sign out", false).clicked() {
            app.actions.push(Action::SignOut);
        }
    } else {
        ui.label(RichText::new("Not signed in. Browsing, search and playback work without an account; likes, the library and playlists need one.").color(theme::TEXT_DIM));
        ui.add_space(6.0);
        if widgets::pill(ui, Some(Icon::SignIn), "Sign in", true).clicked() {
            app.actions.push(Action::OpenSignIn);
        }
    }

    group(ui, "Playback");
    ui.checkbox(&mut draft.autoplay, "Autoplay: keep playing similar songs when the queue ends");
    ui.checkbox(&mut draft.normalize, "Normalize volume (turn loud songs down, as YouTube does)");
    ui.checkbox(&mut draft.restore_session, "Restore the queue when the app starts");

    group(ui, "Window");
    ui.checkbox(&mut draft.close_to_tray, "Closing the window keeps the music playing in the tray");
    ui.horizontal(|ui| {
        ui.label("Interface size");
        ui.add(egui::Slider::new(&mut draft.zoom, 0.8..=1.6).step_by(0.05).show_value(true).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
    });

    group(ui, "Content");
    ui.horizontal(|ui| {
        ui.label("Language");
        ui.add(egui::TextEdit::singleline(&mut draft.language).desired_width(80.0).hint_text("en"));
        ui.label(RichText::new("such as en, de, ja, ms").color(theme::TEXT_FAINT));
    });
    ui.horizontal(|ui| {
        ui.label("Region");
        ui.add(egui::TextEdit::singleline(&mut draft.region).desired_width(80.0).hint_text("auto"));
        ui.label(RichText::new("two letters, such as US or MY; empty follows your connection").color(theme::TEXT_FAINT));
    });

    group(ui, "Streams");
    ui.label(RichText::new("Audio comes straight from YouTube's servers, as Opus (about 130 kb/s) or AAC (128 kb/s). When YouTube refuses that, yt-dlp can find the stream instead.").color(theme::TEXT_DIM));
    ui.add_space(4.0);
    ui.checkbox(&mut draft.premium_quality, "Premium quality: 256 kb/s audio with a Music Premium account");
    note(ui, "Signed in, songs come through yt-dlp with your account, which takes a few seconds for the first song (the next is fetched while one plays). Songs only Premium members can play always go this way.");
    if draft.premium_quality && !app.signed_in {
        note(ui, "Sign in to use it.");
    }
    if let Some(quality) = app.stream_quality() {
        note(ui, &format!("Playing now: {quality}"));
    }
    ui.add_space(6.0);
    ui.checkbox(&mut draft.use_ytdlp, "Use yt-dlp as a fallback");
    ui.horizontal(|ui| {
        ui.label("yt-dlp");
        ui.add(egui::TextEdit::singleline(&mut draft.ytdlp_path).desired_width(360.0).hint_text("found on PATH"));
    });
    let found = app.backend.ytdlp_found();
    ui.label(
        RichText::new(match &found {
            Some(path) => format!("Using {}", path.display()),
            None => "yt-dlp was not found. Install it (winget install yt-dlp) to have the fallback.".into(),
        })
        .color(theme::TEXT_FAINT)
        .font(theme::regular(12.5)),
    );

    group(ui, "Lyrics");
    ui.checkbox(&mut draft.synced_lyrics, "Synced lyrics from LRCLIB");
    note(ui, "Looks the song's title and artist up on lrclib.net, an open lyrics database, for lyrics that follow the song. Otherwise YouTube's own lyrics show, without times.");

    group(ui, "Discord");
    ui.checkbox(&mut draft.discord, "Show what's playing on Discord");
    note(ui, "Your status reads \u{201c}Listening to\u{201d} the artist; your profile shows the artwork, the song and a progress bar. Paused, it shows nothing. Needs the Discord app running.");
    ui.horizontal(|ui| {
        ui.label("Application ID");
        ui.add(egui::TextEdit::singleline(&mut draft.discord_client_id).desired_width(220.0).hint_text("YouTube Music (default)"));
    });
    note(ui, "Optional: the ID of a Discord application of your own, whose name then shows.");

    group(ui, "Last.fm");
    lastfm(app, ui, &mut draft);

    group(ui, "Storage");
    let size = crate::images::cache_size(&app.paths.images());
    ui.label(RichText::new(format!("Artwork cache: {:.1} MB", size as f64 / 1_048_576.0)).color(theme::TEXT_DIM));
    ui.add_space(4.0);
    if widgets::pill(ui, Some(Icon::Trash), "Clear artwork cache", false).clicked() {
        app.actions.push(Action::ClearImageCache);
    }
    ui.add_space(6.0);
    ui.label(RichText::new(format!("Settings live in {}", app.paths.config.display())).color(theme::TEXT_FAINT).font(theme::regular(12.5)));

    group(ui, "Keyboard");
    for (keys, what) in [
        ("Space", "Play or pause"),
        ("Ctrl + Right / Left", "Next / previous song"),
        ("Shift + Right / Left", "Forward / back 10 seconds"),
        ("Ctrl + Up / Down", "Volume"),
        ("M", "Mute"),
        ("Ctrl + F or /", "Search"),
        ("Alt + Left / Right", "Back / forward"),
        ("Q", "Show the queue"),
        ("L", "Like the song"),
    ] {
        ui.horizontal(|ui| {
            ui.add_sized([180.0, 20.0], egui::Label::new(RichText::new(keys).font(theme::medium(13.5))));
            ui.label(RichText::new(what).color(theme::TEXT_DIM));
        });
    }

    group(ui, "About");
    ui.label(format!("Native YTM {}", env!("CARGO_PKG_VERSION")));
    ui.label(RichText::new("A personal, lightweight YouTube Music client: Rust, egui, no browser engine. Not affiliated with Google or YouTube. Modelled on Spotifast.").color(theme::TEXT_DIM));

    if draft != app.settings {
        app.actions.push(Action::ApplySettings(Box::new(draft)));
    } else {
        app.settings_draft = None;
    }
}

fn lastfm(app: &mut App, ui: &mut Ui, draft: &mut crate::settings::Settings) {
    use crate::lastfm::Status;
    match app.lastfm.status() {
        Status::Connected => {
            let user = app.lastfm.stored.user.clone().unwrap_or_else(|| "your account".into());
            ui.label(RichText::new(format!("Connected as {user}")).font(theme::medium(15.0)));
            ui.checkbox(&mut draft.scrobble, "Scrobble the songs I play");
            let waiting = app.lastfm.waiting();
            if waiting > 0 {
                note(ui, &format!("{waiting} scrobbles are waiting to be sent."));
            }
            ui.add_space(4.0);
            if widgets::pill(ui, Some(Icon::SignOut), "Disconnect", false).clicked() {
                app.actions.push(Action::LastFmDisconnect);
            }
        }
        Status::Authorising => {
            ui.label(RichText::new("Waiting for you to allow access on Last.fm, in your browser\u{2026}").color(theme::TEXT_DIM));
        }
        Status::Disconnected => {
            note(ui, "Scrobbling uses a Last.fm API account of your own: create one (any name and description will do), then paste its API key and shared secret here and connect. You allow access on last.fm; your password never passes through the app.");
            ui.add_space(4.0);
            if widgets::pill(ui, Some(Icon::External), "Create an API account", false).clicked() {
                app.actions.push(Action::OpenUrl("https://www.last.fm/api/account/create".into()));
            }
            ui.add_space(6.0);
            let (key, secret) = &mut app.lastfm_form;
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(egui::vec2(110.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| ui.label("API key"));
                ui.add(egui::TextEdit::singleline(key).desired_width(300.0));
            });
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(egui::vec2(110.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| ui.label("Shared secret"));
                ui.add(egui::TextEdit::singleline(secret).desired_width(300.0).password(true));
            });
            ui.add_space(4.0);
            let ready = !app.lastfm_form.0.trim().is_empty() && !app.lastfm_form.1.trim().is_empty();
            if widgets::pill(ui, None, "Connect", ready).clicked() {
                app.actions.push(Action::LastFmConnect);
            }
        }
    }
}

fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::TEXT_FAINT).font(theme::regular(12.5)));
}

fn group(ui: &mut Ui, title: &str) {
    ui.add_space(22.0);
    ui.label(RichText::new(title).font(theme::semibold(18.0)));
    ui.add_space(6.0);
}
