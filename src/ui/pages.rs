//! The page under the top bar: whichever route is open.

use egui::{Align, Color32, CornerRadius, CursorIcon, Layout, Rect, RichText, Sense, Ui, Vec2, pos2, vec2};

use crate::api::model::*;
use crate::api::music::Filter;
use crate::app::App;
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
    let ready = route == Route::Settings || app.pages.get(&key).is_some_and(|p| matches!(p.data, Loadable::Ready(_)));
    if ready && let Some(y) = app.scroll_to.take() {
        area = area.vertical_scroll_offset(y);
    }
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
            Route::Settings => {}
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
    let current = app.queue.current_track().map(|t| t.video_id.clone());
    let playing = app.playing();
    let signed_in = app.signed_in;
    let pages = &app.pages;
    let mut cx = Cx {
        images: &mut app.images,
        actions: &mut app.actions,
        current: current.as_deref(),
        playing,
        signed_in,
    };
    let state = pages.get(key).expect("checked");
    match &state.data {
        Loadable::Loading => widgets::spinner(ui),
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
                    artist_header(ui, &mut cx, header);
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
                let mut job = egui::text::LayoutJob::single_section(
                    description.replace('\n', " "),
                    egui::TextFormat::simple(theme::regular(14.0), theme::TEXT_DIM),
                );
                job.wrap = egui::text::TextWrapping {
                    max_width: text_w,
                    max_rows: 3,
                    break_anywhere: false,
                    overflow_character: Some('\u{2026}'),
                };
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                let response = ui.label(galley);
                if description.len() > 200 {
                    response.on_hover_text(description);
                }
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

fn artist_header(ui: &mut Ui, cx: &mut Cx<'_>, header: &Header) {
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
                pos2(rect.left() + theme::PAGE_MARGIN, rect.bottom() - 156.0),
                pos2(rect.right() - theme::PAGE_MARGIN, rect.bottom()),
            ))
            .layout(Layout::top_down(Align::Min)),
    );
    child.add(egui::Label::new(RichText::new(&header.title).font(theme::bold(46.0)).color(Color32::WHITE)).truncate());
    if let Some(subtitle) = &header.subtitle {
        child.label(RichText::new(subtitle).color(theme::TEXT_DIM).font(theme::regular(14.0)));
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

fn library(app: &mut App, ui: &mut Ui, tab: LibraryTab, key: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new("Library").font(theme::bold(30.0)));
    ui.add_space(12.0);
    if !app.signed_in {
        ui.add_space(24.0);
        ui.label(RichText::new("Sign in to see your playlists, liked songs, albums and history.").color(theme::TEXT_DIM));
        ui.add_space(12.0);
        if widgets::pill(ui, Some(Icon::SignIn), "Sign in", true).clicked() {
            app.actions.push(Action::OpenSignIn);
        }
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for t in LibraryTab::ALL {
            if widgets::chip(ui, t.label(), t == tab).clicked() && t != tab {
                app.actions.push(Action::Navigate(Route::Library(t)));
            }
        }
    });
    ui.add_space(20.0);
    browse(app, ui, &Route::Library(tab), key);
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
    let current = app.queue.current_track().map(|t| t.video_id.clone());
    let playing = app.playing();
    let signed_in = app.signed_in;
    let loading_more = app.search.loading_more;
    let search = &app.search;
    let mut cx = Cx {
        images: &mut app.images,
        actions: &mut app.actions,
        current: current.as_deref(),
        playing,
        signed_in,
    };
    match &search.results {
        Loadable::Loading => widgets::spinner(ui),
        Loadable::Failed(error) => widgets::failed(ui, &friendly(error), Some(Action::Search(search.query.clone())), &mut cx),
        Loadable::Ready(sections) => {
            if sections.is_empty() {
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
