//! The page under the top bar: whichever route is open.

use egui::{Align, Color32, CornerRadius, CursorIcon, Layout, Rect, RichText, Sense, Ui, Vec2, pos2, vec2};

use crate::api::model::*;
use crate::api::music::Filter;
use crate::app::App;
use crate::cache::Mark;
use crate::model::{Action, LibraryTab, Loadable, Route};
use crate::queue::Source;
use crate::theme::{self, Icon};
use crate::ui::widgets::{self, Cx, RowPlace};

pub fn show(app: &mut App, ui: &mut Ui) {
    let route = app.route.clone();
    let key = route.key();
    let mut area = egui::ScrollArea::vertical()
        .id_salt(("page", key.clone()))
        .auto_shrink([false, false]);
    let ready = match &route {
        Route::Settings => true,
        Route::Artist(name) => matches!(app.artists.get(&name.to_lowercase()), Some(Loadable::Ready(_))),
        _ => app.pages.get(&key).is_some_and(|p| matches!(p.data, Loadable::Ready(_))),
    };
    if ready && let Some(y) = app.scroll_to.take() {
        area = area.vertical_scroll_offset(y);
    }
    // A new page fades in.
    let fade = theme::appear(ui.ctx(), egui::Id::new("page-appear"), egui::Id::new(&key).value(), 0.18);
    ui.multiply_opacity(fade);
    let scroll = area.show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: theme::PAGE_MARGIN as i8,
                    right: theme::PAGE_MARGIN as i8,
                    top: 8,
                    bottom: 48,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match &route {
                        Route::Search { .. } => search(app, ui),
                        Route::Settings => crate::ui::settings::show(app, ui),
                        Route::Library(tab) => library(app, ui, *tab, &key),
                        Route::Artist(name) => artist(app, ui, name),
                        _ => browse(app, ui, &route, &key),
                    }
                });
        });
    // Near the bottom: ask for more.
    let remaining = scroll.content_size.y - scroll.state.offset.y - scroll.inner_rect.height();
    if remaining < 900.0 {
        match &route {
            Route::Search { .. } => {
                if app.search.continuation.is_some() && !app.search.loading_more {
                    app.actions.push(Action::SearchMore);
                }
            }
            Route::Settings | Route::Artist(_) => {}
            _ => {
                if let Some(state) = app.pages.get(&key)
                    && let Loadable::Ready(page) = &state.data
                {
                    if page.tracks_continuation.is_some() && !state.loading_tracks {
                        app.actions.push(Action::LoadMoreTracks(key.clone()));
                    } else if (page.continuation.is_some()
                        || page.sections.last().is_some_and(|s| s.continuation.is_some()))
                        && !state.loading_more
                    {
                        app.actions.push(Action::LoadMore(key.clone()));
                    }
                }
            }
        }
    }
}

fn browse(app: &mut App, ui: &mut Ui, route: &Route, key: &str) {
    if !app.pages.contains_key(key) {
        app.actions.push(Action::Refresh);
        widgets::spinner(ui);
        return;
    }
    let state = app.pages.get(key).expect("checked");
    let loading_more = state.loading_more || state.loading_tracks;
    // An artist's page: their numbers on Last.fm, the person's plays too.
    let artist = match (&state.data, route) {
        (Loadable::Ready(page), Route::Browse { browse_id, .. }) => page
            .header
            .as_ref()
            .filter(|h| h.banner || browse_id.starts_with("UC") && h.channel_id.is_some())
            .map(|h| h.title.clone()),
        _ => None,
    };
    let stats = artist.and_then(|name| app.lastfm.artist_stats(&name));
    let current = app.queue.current_track().map(|t| t.video_id.clone());
    let playing = app.playing();
    let signed_in = app.signed_in;
    let online = app.online();
    let pages = &app.pages;
    let mut cx = Cx {
        images: &mut app.images,
        actions: &mut app.actions,
        current: current.as_deref(),
        playing,
        signed_in,
        offline: &app.offline,
        online,
        romanized: app.settings.romanized_titles,
    };
    let state = pages.get(key).expect("checked");
    match &state.data {
        Loadable::Loading => widgets::spinner(ui),
        Loadable::Failed(_) if !online => offline_note(ui, &mut cx),
        Loadable::Failed(error) => widgets::failed(ui, &friendly(error), Some(Action::Refresh), &mut cx),
        Loadable::Ready(page) => {
            if *route == Route::Home {
                home_title(ui, &mut cx, page);
            } else if *route == Route::Explore {
                ui.add_space(8.0);
                ui.label(RichText::new("Explore").font(theme::bold(30.0)));
                ui.add_space(16.0);
            }
            let browse_id = match route {
                Route::Browse { browse_id, .. } => browse_id.as_str(),
                _ => "",
            };
            if let Some(header) = &page.header {
                if header.banner || browse_id.starts_with("UC") && header.channel_id.is_some() {
                    artist_header(ui, &mut cx, header, stats);
                } else {
                    collection_header(ui, &mut cx, header, page, browse_id);
                }
            } else if !page.tracks.is_empty() {
                ui.add_space(8.0);
            }
            if !page.tracks.is_empty() {
                let album = browse_id.starts_with("MPRE");
                let place = if album {
                    RowPlace::Album
                } else if page.header.as_ref().is_some_and(|h| h.editable) {
                    RowPlace::OwnPlaylist(browse_id.trim_start_matches("VL"))
                } else {
                    RowPlace::List
                };
                let source = Source {
                    playlist_id: page.header.as_ref().and_then(|h| h.playlist_id.clone()),
                    continuation: page.tracks_continuation.clone(),
                    watch: false,
                    title: page.header.as_ref().map(|h| h.title.clone()),
                };
                ui.add_space(8.0);
                widgets::track_list(ui, &mut cx, &page.tracks, place, &source, !album);
                ui.add_space(24.0);
            }
            if page.sections.is_empty() && page.tracks.is_empty() && page.header.is_none() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("Nothing here yet").color(theme::TEXT_DIM));
                });
            }
            for (index, section) in page.sections.iter().enumerate() {
                widgets::section(ui, &mut cx, section, key, index);
                ui.add_space(28.0);
            }
            if loading_more {
                widgets::spinner(ui);
            }
        }
    }
}

/// In place of a page that cannot load without the network.
fn offline_note(ui: &mut Ui, cx: &mut Cx<'_>) {
    ui.add_space(56.0);
    ui.vertical_centered(|ui| {
        Icon::Offline.image(theme::TEXT_DIM, 40.0).paint_at(
            ui,
            Rect::from_center_size(ui.cursor().center_top() + vec2(0.0, 20.0), Vec2::splat(40.0)),
        );
        ui.add_space(52.0);
        ui.label(RichText::new("You're offline").font(theme::semibold(20.0)));
        ui.add_space(6.0);
        ui.label(
            RichText::new("This page loads when the connection is back. Your downloads play now.")
                .color(theme::TEXT_DIM),
        );
        ui.add_space(16.0);
        if widgets::pill(ui, Some(Icon::CircleCheck), "Go to your downloads", true).clicked() {
            cx.push(Action::Navigate(Route::Library(LibraryTab::Cached)));
        }
    });
}

fn friendly(error: &str) -> String {
    if error.contains("error sending request") || error.contains("dns") || error.contains("connect") {
        "Cannot reach YouTube Music. Check the connection.".into()
    } else if error.to_lowercase().contains("sign in") {
        "Sign in to see this.".into()
    } else {
        format!("Something went wrong: {error}")
    }
}

fn home_title(ui: &mut Ui, cx: &mut Cx<'_>, page: &Page) {
    ui.add_space(4.0);
    if !page.chips.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for chip in &page.chips {
                if widgets::chip(ui, &chip.title, chip.selected).clicked() {
                    cx.push(Action::Open(chip.target.clone()));
                }
            }
        });
        ui.add_space(20.0);
    }
}

fn collection_header(ui: &mut Ui, cx: &mut Cx<'_>, header: &Header, page: &Page, browse_id: &str) {
    ui.add_space(16.0);
    let width = ui.available_width();
    let art = if width > 760.0 { 232.0 } else { 168.0 };
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(art), Sense::hover());
        widgets::paint_art(ui, cx.images, header.thumbnail.as_deref(), rect, 8.0, false);
        ui.add_space(28.0);
        ui.vertical(|ui| {
            let text_w = (width - art - 28.0).max(200.0);
            ui.set_width(text_w);
            ui.add_space(8.0);
            ui.add(egui::Label::new(RichText::new(&header.title).font(theme::bold(32.0))).wrap());
            ui.add_space(6.0);
            let mut line = Vec::new();
            if let Some(s) = &header.subtitle {
                line.push(s.clone());
            }
            if let Some(author) = &header.author {
                ui.horizontal(|ui| {
                    widgets::links(ui, cx, std::slice::from_ref(author), theme::medium(15.0), theme::TEXT, text_w);
                });
            }
            if !line.is_empty() {
                ui.label(RichText::new(line.join(" \u{2022} ")).color(theme::TEXT_DIM).font(theme::regular(14.0)));
            }
            if let Some(s) = &header.second_subtitle {
                ui.label(RichText::new(s).color(theme::TEXT_DIM).font(theme::regular(14.0)));
            }
            if let Some(description) = &header.description {
                ui.add_space(8.0);
                let id = egui::Id::new(("description", browse_id));
                widgets::read_more(ui, id, description, theme::regular(14.0), theme::TEXT_DIM, text_w, 3);
            }
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let tracks = page.tracks.clone();
                let playlist_id = header.playlist_id.clone();
                let source = Source {
                    playlist_id: playlist_id.clone(),
                    continuation: page.tracks_continuation.clone(),
                    watch: false,
                    title: Some(header.title.clone()),
                };
                if widgets::pill(ui, Some(Icon::Play), "Play", true).clicked() {
                    play_all(cx, &tracks, &playlist_id, page.tracks_continuation.is_some(), false, &header.title, source.clone());
                }
                if widgets::pill(ui, Some(Icon::Shuffle), "Shuffle", false).clicked() {
                    play_all(cx, &tracks, &playlist_id, page.tracks_continuation.is_some(), true, &header.title, source.clone());
                }
                if let Some(saved) = header.saved
                    && let Some(id) = &header.playlist_id
                    && !header.editable
                {
                    let (icon, label) = if saved { (Icon::Saved, "Saved") } else { (Icon::Save, "Save") };
                    if widgets::pill(ui, Some(icon), label, false).clicked() {
                        let target = if browse_id.starts_with("MPRE") { id.clone() } else { browse_id.trim_start_matches("VL").to_owned() };
                        cx.push(Action::SavePlaylist(target, !saved));
                    }
                }
                download_pill(ui, cx, &tracks, &playlist_id, page.tracks_continuation.is_some(), &header.title);
                let more = widgets::icon_button(ui, Icon::More, 20.0, theme::TEXT, "More");
                egui::Popup::menu(&more).show(|ui| {
                    ui.set_min_width(220.0);
                    let entry = |ui: &mut Ui, icon: Icon, text: &str| {
                        ui.add(egui::Button::image_and_text(icon.image(theme::TEXT, 18.0), text).frame_when_inactive(false)).clicked()
                    };
                    if !tracks.is_empty() {
                        if entry(ui, Icon::PlayNext, "Play next") {
                            cx.push(Action::PlayNext(tracks.clone()));
                        }
                        if entry(ui, Icon::AddToQueue, "Add to queue") {
                            cx.push(Action::AddToQueue(tracks.clone()));
                        }
                        if cx.signed_in && entry(ui, Icon::Plus, "Save to playlist") {
                            cx.push(Action::PickPlaylist(tracks.clone()));
                        }
                    }
                    if let Some(radio) = &header.radio
                        && entry(ui, Icon::Radio, "Start radio")
                    {
                        cx.push(Action::Open(radio.clone()));
                    } else if let Some(id) = &playlist_id
                        && entry(ui, Icon::Radio, "Start radio")
                    {
                        cx.push(Action::PlayWatch {
                            video_id: None,
                            playlist_id: Some(format!("RDAMPL{id}")),
                            params: Some("wAEB".into()),
                            title: Some(format!("{} radio", header.title)),
                        });
                    }
                    let url = if browse_id.starts_with("MPRE") {
                        playlist_id.as_ref().map(|id| format!("https://music.youtube.com/playlist?list={id}"))
                    } else {
                        Some(format!("https://music.youtube.com/playlist?list={}", browse_id.trim_start_matches("VL")))
                    };
                    if let Some(url) = url {
                        if entry(ui, Icon::Link, "Copy link") {
                            cx.push(Action::CopyLink(url.clone()));
                        }
                        if entry(ui, Icon::External, "Open in browser") {
                            cx.push(Action::OpenUrl(url));
                        }
                    }
                });
            });
        });
    });
    ui.add_space(20.0);
}

/// Download, for a whole album or playlist: "Downloaded" once every song
/// is, "Downloading" with how far while they come.
fn download_pill(ui: &mut Ui, cx: &mut Cx<'_>, tracks: &[Track], playlist_id: &Option<String>, has_more: bool, title: &str) {
    if tracks.is_empty() && playlist_id.is_none() {
        return;
    }
    let marks: Vec<Option<&Mark>> = tracks.iter().map(|t| cx.offline.get(&t.video_id)).collect();
    let saved = marks.iter().filter(|m| matches!(m, Some(Mark::Saved))).count();
    let coming = marks.iter().filter(|m| matches!(m, Some(Mark::Saving(_) | Mark::Waiting))).count();
    if !tracks.is_empty() && saved == tracks.len() && !has_more {
        let response = widgets::pill(ui, Some(Icon::CircleCheck), "Downloaded", false)
            .on_hover_text("Every song is on this PC and plays offline. Click to remove the downloads.");
        if response.clicked() {
            cx.push(Action::RemoveOffline(tracks.iter().map(|t| t.video_id.clone()).collect()));
        }
    } else if coming > 0 {
        let label = format!("Downloading {} of {}", saved + 1, saved + coming);
        widgets::pill(ui, Some(Icon::Download), &label, false).on_hover_text("Downloading for offline listening");
    } else if widgets::pill(ui, Some(Icon::Download), "Download", false)
        .on_hover_text("Keep every song on this PC, to play offline")
        .clicked()
    {
        match playlist_id {
            Some(id) if has_more || tracks.is_empty() => cx.push(Action::SaveOfflinePlaylist {
                playlist_id: id.clone(),
                title: title.to_owned(),
            }),
            _ => cx.push(Action::SaveOffline(tracks.to_vec())),
        }
    }
}

/// Play every song of a page: the list in hand when it is all there, or the
/// playlist in full when the page has more to load.
fn play_all(
    cx: &mut Cx<'_>,
    tracks: &[Track],
    playlist_id: &Option<String>,
    has_more: bool,
    shuffle: bool,
    title: &str,
    source: Source,
) {
    if has_more && let Some(id) = playlist_id {
        cx.push(Action::PlayPlaylist {
            playlist_id: id.clone(),
            shuffle,
            title: Some(title.to_owned()),
        });
        return;
    }
    if tracks.is_empty() {
        if let Some(id) = playlist_id {
            cx.push(Action::PlayPlaylist {
                playlist_id: id.clone(),
                shuffle,
                title: Some(title.to_owned()),
            });
        }
        return;
    }
    let start = if shuffle { fastrand::usize(..tracks.len()) } else { 0 };
    // Shuffle plays a random song first, then the rest in random order.
    cx.push(Action::SetShuffle(shuffle));
    cx.push(Action::PlayTracks {
        tracks: tracks.to_vec(),
        start,
        source,
    });
}

fn artist_header(ui: &mut Ui, cx: &mut Cx<'_>, header: &Header, stats: Option<crate::lastfm::ArtistStats>) {
    let width = ui.available_width() + theme::PAGE_MARGIN * 2.0;
    let height = (width * 0.32).clamp(240.0, 400.0);
    let top = ui.cursor().top() - 8.0;
    let rect = Rect::from_min_size(pos2(ui.max_rect().left() - theme::PAGE_MARGIN, top), vec2(width, height));
    widgets::paint_art(ui, cx.images, header.thumbnail.as_deref(), rect, 0.0, false);
    // Fade into the page towards the bottom, so the name reads and the
    // banner has no edge.
    let mut mesh = egui::Mesh::default();
    let fade_top = rect.top() + rect.height() * 0.25;
    let stops = [(0.0f32, 0.0f32), (0.45, 0.55), (0.8, 0.9), (1.0, 1.0)];
    for (i, (t, alpha)) in stops.iter().enumerate() {
        let y = fade_top + (rect.bottom() - fade_top) * t;
        let color = theme::BG.gamma_multiply(*alpha);
        mesh.colored_vertex(pos2(rect.left(), y), color);
        mesh.colored_vertex(pos2(rect.right(), y), color);
        if i > 0 {
            let b = (i as u32) * 2;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b, b + 1);
        }
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(50));
    painter.add(egui::Shape::mesh(mesh));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                pos2(rect.left() + theme::PAGE_MARGIN, rect.bottom() - if stats.is_some() { 184.0 } else { 156.0 }),
                pos2(rect.right() - theme::PAGE_MARGIN, rect.bottom()),
            ))
            .layout(Layout::top_down(Align::Min)),
    );
    child.add(egui::Label::new(RichText::new(&header.title).font(theme::bold(46.0)).color(Color32::WHITE)).truncate());
    if let Some(subtitle) = &header.subtitle {
        child.label(RichText::new(subtitle).color(theme::TEXT_DIM).font(theme::regular(14.0)));
    }
    if let Some(stats) = stats {
        lastfm_line(&mut child, stats);
    }
    child.add_space(14.0);
    child.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if let Some(shuffle) = &header.shuffle
            && widgets::pill(ui, Some(Icon::Shuffle), "Shuffle", true).clicked()
        {
            cx.push(Action::Open(shuffle.clone()));
        }
        if let Some(radio) = &header.radio
            && widgets::pill(ui, Some(Icon::Radio), "Radio", false).clicked()
        {
            cx.push(Action::Open(radio.clone()));
        }
        if let Some(channel) = &header.channel_id {
            let subscribed = header.subscribed.unwrap_or(false);
            let label = match (&header.subscribers, subscribed) {
                (_, true) => "Subscribed".to_owned(),
                (Some(count), false) => format!("Subscribe \u{2022} {count}"),
                (None, false) => "Subscribe".to_owned(),
            };
            if widgets::pill(ui, None, &label, false).clicked() {
                cx.push(Action::Subscribe(channel.clone(), !subscribed));
            }
        }
    });
    // The banner's height, less the top margin it reaches into.
    ui.add_space(height + 12.0);
}

/// The person's scrobbles of an artist, and everyone's listeners, beside
/// Last.fm's mark.
fn lastfm_line(ui: &mut Ui, stats: crate::lastfm::ArtistStats) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        Icon::LastFm.image(theme::ACCENT_TEXT, 16.0).paint_at(ui, rect);
        let mine = match stats.plays.unwrap_or(0) {
            0 => "Not in your scrobbles yet".to_owned(),
            1 => "You've scrobbled them once".to_owned(),
            n => format!("You've scrobbled them {} times", widgets::thousands(n)),
        };
        ui.label(RichText::new(mine).font(theme::medium(14.0)).color(theme::ACCENT_TEXT));
        if let Some(n) = stats.listeners {
            ui.label(RichText::new(format!("\u{00b7} {} listeners on Last.fm", widgets::thousands(n))).color(theme::TEXT_DIM).font(theme::regular(14.0)));
        }
    })
    .response
    .on_hover_text(match stats.scrobbles {
        Some(n) => format!("{} scrobbles of theirs on Last.fm, all told", widgets::thousands(n)),
        None => "From your Last.fm".to_owned(),
    });
}

fn library(app: &mut App, ui: &mut Ui, tab: LibraryTab, key: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new("Library").font(theme::bold(30.0)));
    ui.add_space(12.0);
    // Downloads and the songs on this PC are the person's whether signed
    // in or not.
    let tabs: &[LibraryTab] = if app.signed_in { &LibraryTab::ALL } else { &[LibraryTab::Cached, LibraryTab::Device] };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for &t in tabs {
            if widgets::chip(ui, t.label(), t == tab).clicked() && t != tab {
                app.actions.push(Action::Navigate(Route::Library(t)));
            }
        }
    });
    ui.add_space(20.0);
    if tab == LibraryTab::Cached {
        cached(app, ui);
        return;
    }
    if tab == LibraryTab::Device {
        device(app, ui);
        return;
    }
    if !app.signed_in {
        ui.add_space(4.0);
        ui.label(RichText::new("Sign in to see your playlists, liked songs, albums and history.").color(theme::TEXT_DIM));
        ui.add_space(12.0);
        if widgets::pill(ui, Some(Icon::SignIn), "Sign in", true).clicked() {
            app.actions.push(Action::OpenSignIn);
        }
        return;
    }
    browse(app, ui, &Route::Library(tab), key);
}

/// The songs on this PC: the downloads, then the songs kept from recent
/// plays, every one playing without the network.
fn cached(app: &mut App, ui: &mut Ui) {
    let (saved, kept) = app.cached_lists();
    let usage = app.audio_cache.usage();
    let downloading = app.downloads.active().map(|(track, done)| (track.title.clone(), done));
    let waiting = app.downloads.waiting.len();
    let current = app.queue.current_track().map(|t| t.video_id.clone());
    let playing = app.playing();
    let signed_in = app.signed_in;
    let online = app.online();
    let mut cx = Cx {
        images: &mut app.images,
        actions: &mut app.actions,
        current: current.as_deref(),
        playing,
        signed_in,
        offline: &app.offline,
        online,
        romanized: app.settings.romanized_titles,
    };

    // What is downloading, while anything is.
    if let Some((title, done)) = downloading {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().min(640.0), 64.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(10), theme::SURFACE);
        let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(16.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
        row.spacing_mut().item_spacing.x = 12.0;
        widgets::offline_mark(&mut row, Mark::Saving(done));
        row.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let line = if waiting > 0 { format!("Downloading \u{201c}{title}\u{201d} \u{00b7} {waiting} more to go") } else { format!("Downloading \u{201c}{title}\u{201d}") };
            widgets::clipped_label(ui, &line, theme::medium(14.0), theme::TEXT, rect.width() - 160.0, Sense::hover());
            ui.label(RichText::new(format!("{:.0}%", done * 100.0)).font(theme::regular(12.5)).color(theme::TEXT_DIM));
        });
        row.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill(ui, None, "Stop", false).clicked() {
                cx.push(Action::CancelDownloads);
            }
        });
        ui.add_space(20.0);
    } else if waiting > 0 && !online {
        ui.label(RichText::new(format!("{} wait for the connection to download.", widgets::songs(waiting))).color(theme::TEXT_DIM));
        ui.add_space(16.0);
    }

    // The downloads.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Downloaded").font(theme::bold(22.0)));
        if usage.saved > 0 {
            ui.label(
                RichText::new(format!("{} \u{00b7} {:.0} MB", widgets::songs(usage.saved), usage.saved_bytes as f64 / 1_048_576.0))
                    .color(theme::TEXT_DIM),
            );
        }
    });
    ui.add_space(10.0);
    if saved.is_empty() {
        ui.label(
            RichText::new("Nothing downloaded yet. Choose Download on an album or a playlist, or in a song's menu, and it plays here without the network.")
                .color(theme::TEXT_DIM),
        );
    } else {
        let source = Source {
            title: Some("Downloaded".into()),
            ..Source::default()
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if widgets::pill(ui, Some(Icon::Play), "Play", true).clicked() {
                cx.push(Action::SetShuffle(false));
                cx.push(Action::PlayTracks { tracks: saved.to_vec(), start: 0, source: source.clone() });
            }
            if widgets::pill(ui, Some(Icon::Shuffle), "Shuffle", false).clicked() {
                cx.push(Action::SetShuffle(true));
                cx.push(Action::PlayTracks { tracks: saved.to_vec(), start: fastrand::usize(..saved.len()), source: source.clone() });
            }
            let more = widgets::icon_button(ui, Icon::More, 20.0, theme::TEXT, "More");
            egui::Popup::menu(&more).show(|ui| {
                ui.set_min_width(220.0);
                if ui
                    .add(egui::Button::image_and_text(Icon::Trash.image(theme::TEXT, 18.0), "Remove every download").frame_when_inactive(false))
                    .clicked()
                {
                    cx.push(Action::ClearDownloads);
                }
            });
        });
        ui.add_space(8.0);
        widgets::track_list(ui, &mut cx, &saved, RowPlace::List, &source, true);
    }

    // The songs kept from plays.
    if !kept.is_empty() {
        ui.add_space(32.0);
        ui.label(RichText::new("Played lately").font(theme::bold(22.0)));
        ui.add_space(4.0);
        ui.label(
            RichText::new("Kept on this PC from your recent plays, so they play offline too, until newer songs take their place. Download one to keep it.")
                .color(theme::TEXT_DIM),
        );
        ui.add_space(10.0);
        let source = Source {
            title: Some("Played lately".into()),
            ..Source::default()
        };
        widgets::track_list(ui, &mut cx, &kept, RowPlace::List, &source, true);
    }
}

/// The songs in the person's own folders: all of them, to play, shuffle
/// or filter; or, before any folder is added, how to add one.
fn device(app: &mut App, ui: &mut Ui) {
    let songs = app.local.songs();
    let scanning = app.local.scanning();
    let scanned = app.local.scanned;
    let albums = app.local.albums();
    let folders = app.settings.local_folders.len();
    let picking = app.picking_folder;
    let filter_id = egui::Id::new("device-filter");
    let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    let shown = if filter.trim().is_empty() { songs.clone() } else { app.local_hits(filter.trim()) };
    let current = app.queue.current_track().map(|t| t.video_id.clone());
    let playing = app.playing();
    let signed_in = app.signed_in;
    let online = app.online();
    let mut cx = Cx {
        images: &mut app.images,
        actions: &mut app.actions,
        current: current.as_deref(),
        playing,
        signed_in,
        offline: &app.offline,
        online,
        romanized: app.settings.romanized_titles,
    };
    if folders == 0 {
        device_welcome(ui, &mut cx, picking);
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(RichText::new("Your music").font(theme::bold(22.0)));
        let albums = if albums == 1 { "1 album".to_owned() } else { format!("{albums} albums") };
        let line = match scanning {
            Some(seen) if songs.is_empty() => format!("Looking through your folders\u{2026} {seen} files so far"),
            Some(_) => format!("{} \u{00b7} {albums} \u{00b7} looking for changes", widgets::songs(songs.len())),
            None => format!("{} \u{00b7} {albums}", widgets::songs(songs.len())),
        };
        ui.label(RichText::new(line).color(theme::TEXT_DIM));
        if scanning.is_some() {
            widgets::spinner_of(ui, 14.0, theme::TEXT_DIM);
        }
    });
    ui.add_space(12.0);
    let source = Source {
        title: Some("On this PC".into()),
        ..Source::default()
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if !songs.is_empty() {
            if widgets::pill(ui, Some(Icon::Play), "Play", true).clicked() && !shown.is_empty() {
                cx.push(Action::SetShuffle(false));
                cx.push(Action::PlayTracks { tracks: shown.to_vec(), start: 0, source: source.clone() });
            }
            if widgets::pill(ui, Some(Icon::Shuffle), "Shuffle", false).clicked() && !shown.is_empty() {
                cx.push(Action::SetShuffle(true));
                cx.push(Action::PlayTracks { tracks: shown.to_vec(), start: fastrand::usize(..shown.len()), source: source.clone() });
            }
        }
        if widgets::pill(ui, Some(Icon::FolderPlus), if picking { "Choosing\u{2026}" } else { "Add a folder" }, false)
            .on_hover_text("Add another folder of music")
            .clicked()
        {
            cx.push(Action::AddLocalFolder);
        }
        if widgets::icon_button(ui, Icon::Refresh, 18.0, theme::TEXT_DIM, "Look through the folders again").clicked() {
            cx.push(Action::RescanLocal);
        }
        if !songs.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut filter)
                        .hint_text(RichText::new("Filter by title, artist or album").color(theme::TEXT_FAINT))
                        .desired_width(240.0)
                        .margin(egui::Margin::symmetric(10, 6)),
                );
                if field.changed() {
                    ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
                }
            });
        }
    });
    ui.add_space(12.0);
    if songs.is_empty() {
        if scanned && scanning.is_none() {
            ui.add_space(20.0);
            ui.label(
                RichText::new("No music found in these folders yet. MP3, FLAC, M4A, Ogg, Opus, WAV and AIFF files play; add the folder that holds them.")
                    .color(theme::TEXT_DIM),
            );
        } else {
            widgets::spinner(ui);
        }
        return;
    }
    if shown.is_empty() {
        ui.add_space(12.0);
        ui.label(RichText::new(format!("Nothing matches \u{201c}{}\u{201d}", filter.trim())).color(theme::TEXT_DIM));
        return;
    }
    widgets::track_list(ui, &mut cx, &shown, RowPlace::List, &source, true);
}

/// Before any folder: what the page is for, and the way to begin.
fn device_welcome(ui: &mut Ui, cx: &mut Cx<'_>, picking: bool) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(72.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 36.0, Color32::from_white_alpha(14));
        Icon::Folder.image(theme::TEXT, 32.0).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(32.0)));
        ui.add_space(18.0);
        ui.label(RichText::new("Play the music on this PC").font(theme::semibold(20.0)));
        ui.add_space(8.0);
        ui.set_max_width(460.0);
        ui.label(
            RichText::new("Add the folders where you keep MP3, FLAC, M4A, Ogg or WAV files. Their songs show here and come first when you search, beside YouTube Music. Nothing leaves this PC.")
                .color(theme::TEXT_DIM),
        );
        ui.add_space(18.0);
        if widgets::pill(ui, Some(Icon::FolderPlus), if picking { "Choosing\u{2026}" } else { "Add a folder" }, true).clicked() {
            cx.push(Action::AddLocalFolder);
        }
    });
}

/// An artist of songs on this PC, as YouTube Music shows one: their picture
/// (from YouTube Music), what Last.fm says of them, their songs here, and
/// their popular songs and albums, which play from YouTube Music.
fn artist(app: &mut App, ui: &mut Ui, name: &str) {
    let key = name.to_lowercase();
    let (page, loading) = match app.artists.get(&key) {
        Some(Loadable::Ready(page)) => (page.clone(), false),
        Some(_) => (crate::app::ArtistPage::default(), true),
        None => {
            app.actions.push(Action::Refresh);
            (crate::app::ArtistPage::default(), true)
        }
    };
    let songs = app.artist_songs(name);
    let connected = app.lastfm.reader().is_some();
    let info = page.lastfm.as_ref().and_then(|r| r.as_ref().ok());
    let shown_name = info.map_or(name, |i| i.name.as_str()).to_owned();
    // Their picture: YouTube Music's, else an album cover, else a song's.
    let picture = page
        .youtube
        .as_ref()
        .and_then(|i| i.thumbnail.clone())
        .or_else(|| info.and_then(|i| i.albums.iter().find_map(|(_, art)| art.clone())))
        .or_else(|| songs.iter().find_map(|s| s.thumbnail.clone()));
    let current = app.queue.current_track().map(|t| t.video_id.clone());
    let playing = app.playing();
    let signed_in = app.signed_in;
    let online = app.online();
    let mut cx = Cx {
        images: &mut app.images,
        actions: &mut app.actions,
        current: current.as_deref(),
        playing,
        signed_in,
        offline: &app.offline,
        online,
        romanized: app.settings.romanized_titles,
    };

    // The header: their picture round over a wash of its colours.
    let width = ui.available_width() + theme::PAGE_MARGIN * 2.0;
    let top = ui.cursor().top() - 8.0;
    let band = Rect::from_min_size(pos2(ui.max_rect().left() - theme::PAGE_MARGIN, top), vec2(width, 300.0));
    if let Some(url) = &picture
        && let Some(wash) = cx.images.backdrop(url)
    {
        let mut mesh = egui::Mesh::with_texture(wash.id());
        let uv = Rect::from_min_max(pos2(0.0, 0.3), pos2(1.0, 0.7));
        // Darkened at the top, and fading into the page at the foot.
        let top_shade = Color32::from_rgba_premultiplied(107, 107, 107, 255);
        for (y, v, color) in [(band.top(), uv.top(), top_shade), (band.bottom(), uv.bottom(), Color32::TRANSPARENT)] {
            mesh.vertices.push(egui::epaint::Vertex { pos: pos2(band.left(), y), uv: pos2(0.0, v), color });
            mesh.vertices.push(egui::epaint::Vertex { pos: pos2(band.right(), y), uv: pos2(1.0, v), color });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        ui.painter_at(band).add(egui::Shape::mesh(mesh));
    }
    ui.add_space(36.0);
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(184.0), Sense::hover());
        ui.painter().add(
            egui::Shadow { offset: [0, 10], blur: 34, spread: 0, color: Color32::from_black_alpha(120) }
                .as_shape(rect, CornerRadius::same(92)),
        );
        widgets::paint_art(ui, cx.images, picture.as_deref(), rect, 0.0, true);
        ui.add_space(28.0);
        ui.vertical(|ui| {
            ui.add_space(26.0);
            ui.label(RichText::new("ARTIST").font(theme::semibold(12.0)).color(theme::TEXT_DIM));
            ui.add(egui::Label::new(RichText::new(&shown_name).font(theme::bold(44.0)).color(Color32::WHITE)).truncate());
            let mut stats = Vec::new();
            if let Some(info) = info {
                if let Some(n) = info.listeners {
                    stats.push(format!("{} listeners", widgets::thousands(n)));
                }
                if let Some(n) = info.scrobbles {
                    stats.push(format!("{} scrobbles on Last.fm", widgets::thousands(n)));
                }
            }
            if !songs.is_empty() {
                stats.push(format!("{} on this PC", widgets::songs(songs.len())));
            }
            if !stats.is_empty() {
                ui.label(RichText::new(stats.join(" \u{00b7} ")).color(theme::TEXT_DIM));
            }
            if let Some(plays) = info.and_then(|i| i.plays).filter(|n| *n > 0) {
                let times = if plays == 1 { "once".to_owned() } else { format!("{} times", widgets::thousands(plays)) };
                ui.label(RichText::new(format!("You've played them {times}")).color(theme::ACCENT_TEXT));
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let source = Source { title: Some(shown_name.clone()), ..Source::default() };
                if !songs.is_empty() {
                    if widgets::pill(ui, Some(Icon::Play), "Play", true).clicked() {
                        cx.push(Action::SetShuffle(false));
                        cx.push(Action::PlayTracks { tracks: songs.to_vec(), start: 0, source: source.clone() });
                    }
                    if widgets::pill(ui, Some(Icon::Shuffle), "Shuffle", false).clicked() {
                        cx.push(Action::SetShuffle(true));
                        cx.push(Action::PlayTracks { tracks: songs.to_vec(), start: fastrand::usize(..songs.len()), source });
                    }
                }
                if let Some(Target::Browse { browse_id, params }) = page.youtube.as_ref().and_then(|i| i.target.clone())
                    && widgets::pill(ui, Some(Icon::Music), "On YouTube Music", songs.is_empty())
                        .on_hover_text("Their page on YouTube Music: songs, albums, videos and radio")
                        .clicked()
                {
                    cx.push(Action::Navigate(Route::Browse { browse_id, params }));
                }
                if let Some(info) = info.filter(|i| !i.url.is_empty())
                    && widgets::pill(ui, Some(Icon::LastFm), "Last.fm", false).clicked()
                {
                    cx.push(Action::OpenUrl(info.url.clone()));
                }
            });
        });
    });
    ui.add_space(28.0);

    // What Last.fm says of them, up top: a few lines, the rest a click away,
    // and their tags.
    if let Some(info) = info
        && (!info.bio.is_empty() || !info.tags.is_empty())
    {
        let width = ui.available_width().min(760.0);
        ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
            if !info.bio.is_empty() {
                let id = egui::Id::new(("artist-bio", &key));
                widgets::read_more(ui, id, &info.bio, theme::regular(14.5), theme::TEXT_DIM, width, 3);
                ui.add_space(14.0);
            }
            if !info.tags.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    for tag in &info.tags {
                        if widgets::chip(ui, tag, false).on_hover_text("Search YouTube Music").clicked() {
                            cx.push(Action::Search(tag.clone()));
                        }
                    }
                });
            }
        });
        ui.add_space(32.0);
    } else {
        ui.add_space(8.0);
    }

    if !songs.is_empty() {
        ui.label(RichText::new("On this PC").font(theme::bold(22.0)));
        ui.add_space(10.0);
        let source = Source { title: Some(shown_name.clone()), ..Source::default() };
        widgets::track_list(ui, &mut cx, &songs, RowPlace::List, &source, true);
        ui.add_space(28.0);
    }

    if loading {
        widgets::spinner(ui);
        return;
    }
    let Some(info) = info else {
        // No Last.fm account, or Last.fm does not know them.
        let text = if connected {
            format!("Last.fm has nothing on {shown_name} yet.")
        } else {
            format!("Connect Last.fm in Settings, under Sharing, to see {shown_name}'s story, popular songs and albums here.")
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            Icon::LastFm.image(theme::TEXT_DIM, 18.0).paint_at(ui, Rect::from_min_size(ui.cursor().min + vec2(0.0, 1.0), Vec2::splat(18.0)));
            ui.add_space(26.0);
            ui.label(RichText::new(text).color(theme::TEXT_DIM));
        });
        if !connected {
            ui.add_space(10.0);
            if widgets::pill(ui, Some(Icon::Settings), "Open Settings", false).clicked() {
                cx.push(Action::Navigate(Route::Settings));
            }
        }
        return;
    };

    // Their most played songs, which play from this PC when they are here
    // and from YouTube Music when not.
    if !info.top_tracks.is_empty() {
        ui.label(RichText::new("Popular on Last.fm").font(theme::bold(22.0)));
        ui.add_space(10.0);
        let row_w = ui.available_width().min(760.0);
        for (i, (title, listeners)) in info.top_tracks.iter().enumerate() {
            let (rect, response) = ui.allocate_exact_size(vec2(row_w, 46.0), Sense::click());
            let response = response.on_hover_cursor(CursorIcon::PointingHand);
            let here = songs.iter().position(|s| crate::local::loose(&s.title) == crate::local::loose(title));
            if response.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(6), Color32::from_white_alpha(14));
            }
            let center = rect.center().y;
            if response.hovered() {
                Icon::Play.image(theme::TEXT, 16.0).paint_at(ui, Rect::from_center_size(pos2(rect.left() + 22.0, center), Vec2::splat(16.0)));
            } else {
                ui.painter().text(pos2(rect.left() + 22.0, center), egui::Align2::CENTER_CENTER, (i + 1).to_string(), theme::regular(14.0), theme::TEXT_DIM);
            }
            let title_g = ui.painter().layout_no_wrap(title.clone(), theme::medium(14.5), theme::TEXT);
            let title_w = title_g.size().x;
            ui.painter().galley(pos2(rect.left() + 52.0, center - title_g.size().y / 2.0), title_g, theme::TEXT);
            if here.is_some() {
                Icon::Computer
                    .image(theme::TEXT_DIM, 14.0)
                    .paint_at(ui, Rect::from_center_size(pos2(rect.left() + 52.0 + title_w + 16.0, center), Vec2::splat(14.0)));
            }
            if let Some(n) = listeners {
                ui.painter().text(
                    pos2(rect.right() - 12.0, center),
                    egui::Align2::RIGHT_CENTER,
                    format!("{} listeners", widgets::thousands(*n)),
                    theme::regular(13.0),
                    theme::TEXT_DIM,
                );
            }
            let response = response.on_hover_text(if here.is_some() { "On this PC" } else { "Plays from YouTube Music" });
            if response.clicked() {
                match here {
                    Some(start) => cx.push(Action::PlayTracks {
                        tracks: songs.to_vec(),
                        start,
                        source: Source { title: Some(shown_name.clone()), ..Source::default() },
                    }),
                    None => cx.push(Action::PlaySong {
                        query: format!("{shown_name} {title}"),
                        artist: Some(shown_name.clone()),
                    }),
                }
            }
        }
        ui.add_space(28.0);
    }

    // Their albums: a click looks them up on YouTube Music.
    if !info.albums.is_empty() {
        ui.label(RichText::new("Albums").font(theme::bold(22.0)));
        ui.add_space(10.0);
        let items: Vec<Item> = info
            .albums
            .iter()
            .map(|(title, art)| Item {
                kind: Kind::Album,
                title: title.clone(),
                subtitle: shown_name.clone(),
                thumbnail: art.clone(),
                target: Some(Target::Search { query: format!("{shown_name} {title}"), params: None }),
                track: None,
                play_playlist: None,
                color: None,
            })
            .collect();
        widgets::card_grid(ui, &mut cx, &items);
        ui.add_space(20.0);
    }

    if !info.similar.is_empty() {
        ui.label(RichText::new("Fans also like").font(theme::bold(22.0)));
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            for other in &info.similar {
                if widgets::pill(ui, Some(Icon::User), other, false).clicked() {
                    cx.push(Action::Navigate(Route::Artist(other.clone())));
                }
            }
        });
    }
}

/// The songs on this PC matching a search, before YouTube Music's: a few,
/// and the rest a click away. Any of them plays them all, from there.
fn from_device(ui: &mut Ui, cx: &mut Cx<'_>, hits: &[Track], query: &str) {
    const FEW: usize = 4;
    let open_id = egui::Id::new(("from-device-open", query));
    let open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
        Icon::Computer.image(theme::TEXT, 20.0).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(20.0)));
        ui.label(RichText::new("From your device").font(theme::bold(22.0)));
        ui.label(RichText::new(widgets::songs(hits.len())).color(theme::TEXT_DIM));
        if hits.len() > FEW {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if open { "Show fewer".to_owned() } else { format!("Show all {}", hits.len()) };
                if widgets::pill(ui, None, &label, false).clicked() {
                    ui.data_mut(|d| d.insert_temp(open_id, !open));
                }
            });
        }
    });
    ui.add_space(10.0);
    let source = Source {
        title: Some(format!("\u{201c}{query}\u{201d} on this PC")),
        ..Source::default()
    };
    let count = if open { hits.len() } else { hits.len().min(FEW) };
    let clip = ui.clip_rect();
    for (i, track) in hits[..count].iter().enumerate() {
        let top = ui.cursor().top();
        if top > clip.bottom() + 200.0 || top + theme::ROW_HEIGHT < clip.top() - 200.0 {
            ui.allocate_space(vec2(ui.available_width(), theme::ROW_HEIGHT));
            continue;
        }
        if widgets::track_row(ui, cx, track, Some(i + 1), RowPlace::List, true).play {
            cx.push(Action::PlayTracks { tracks: hits.to_vec(), start: i, source: source.clone() });
        }
    }
}

fn search(app: &mut App, ui: &mut Ui) {
    ui.add_space(4.0);
    let filter = app.search.filter;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for f in Filter::ALL {
            if widgets::chip(ui, f.label(), f == filter).clicked() && f != filter {
                app.actions.push(Action::SetSearchFilter(f));
            }
        }
    });
    ui.add_space(20.0);
    // Songs on this PC come first, for a search of everything or of songs.
    let query = app.search.query.clone();
    let local = if matches!(filter, Filter::All | Filter::Songs) && !query.trim().is_empty() {
        app.local_hits(query.trim())
    } else {
        Default::default()
    };
    let current = app.queue.current_track().map(|t| t.video_id.clone());
    let playing = app.playing();
    let signed_in = app.signed_in;
    let online = app.online();
    let loading_more = app.search.loading_more;
    let search = &app.search;
    let mut cx = Cx {
        images: &mut app.images,
        actions: &mut app.actions,
        current: current.as_deref(),
        playing,
        signed_in,
        offline: &app.offline,
        online,
        romanized: app.settings.romanized_titles,
    };
    if !local.is_empty() {
        from_device(ui, &mut cx, &local, query.trim());
        ui.add_space(28.0);
    }
    match &search.results {
        Loadable::Loading => widgets::spinner(ui),
        Loadable::Failed(_) if !online && !local.is_empty() => {}
        Loadable::Failed(_) if !online => offline_note(ui, &mut cx),
        Loadable::Failed(error) => widgets::failed(ui, &friendly(error), Some(Action::Search(search.query.clone())), &mut cx),
        Loadable::Ready(sections) => {
            if sections.is_empty() && local.is_empty() {
                ui.add_space(32.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(format!("No results for \u{201c}{}\u{201d}", search.query)).color(theme::TEXT_DIM));
                });
            }
            for (index, section) in sections.iter().enumerate() {
                widgets::section(ui, &mut cx, section, "search", index);
                ui.add_space(20.0);
            }
            if loading_more {
                widgets::spinner(ui);
            }
        }
    }
    let _ = (CursorIcon::Default, Rect::NOTHING);
}
