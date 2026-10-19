//! The bar along the bottom: the seek bar across its top edge, the
//! transport on the left, the song in the middle, and the rest on the right.

use egui::{Align, Align2, Color32, CornerRadius, CursorIcon, Layout, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::api::model::{Like, format_duration};
use crate::api::music::Rating;
use crate::app::App;
use crate::model::{Action, SideTab};
use crate::settings::Repeat;
use crate::theme::{self, Icon};
use crate::ui::widgets::{self, RowPlace, thousands};

/// The seek bar's line lies along the bar's top edge; this much of the bar
/// below it takes its clicks and drags.
const SEEK_GRAB: f32 = 12.0;

pub fn show(app: &mut App, ui: &mut Ui) {
    let rect = ui.max_rect();
    // Under the seek bar's line and knob, in the same layer: drawn first.
    super::visualiser::paint(app, ui, &overlay(ui), rect);
    seek_bar(app, ui, Rect::from_min_size(rect.min, vec2(rect.width(), SEEK_GRAB)));
    // The rest, centred below the line, with the same room above and below
    // the artwork.
    let inner = Rect::from_min_max(rect.min + vec2(16.0, 3.0), rect.max - vec2(16.0, 0.0));
    let left_w = 260.0f32.min(inner.width() * 0.3);
    let right_w = 424.0f32.min(inner.width() * 0.38);
    let left = Rect::from_min_size(inner.min, vec2(left_w, inner.height()));
    let right = Rect::from_min_max(pos2(inner.right() - right_w, inner.top()), inner.max);
    let middle = Rect::from_min_max(pos2(left.right() + 16.0, inner.top()), pos2(right.left() - 16.0, inner.bottom()));
    transport(app, &mut ui.new_child(egui::UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center))));
    now(app, &mut ui.new_child(egui::UiBuilder::new().max_rect(middle).layout(Layout::left_to_right(Align::Center))));
    extras(app, &mut ui.new_child(egui::UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center))));
}

/// What draws over the page from the bar: above the page, unclipped, so
/// the seek knob can rise over the page's bottom edge, and the bar's own
/// clip would cut it in half. Floating over the full screen, the bar is a
/// layer of its own already, on top.
fn overlay(ui: &Ui) -> egui::Painter {
    if ui.layer_id().order == egui::Order::Foreground {
        ui.painter().with_clip_rect(ui.ctx().content_rect())
    } else {
        ui.ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Middle, ui.id().with("seek-layer")))
    }
}

fn seek_bar(app: &mut App, ui: &mut Ui, rect: Rect) {
    let duration = app.duration();
    let response = ui.interact(rect, ui.id().with("seek"), Sense::click_and_drag());
    let active = response.hovered() || response.dragged();
    // The line thickens and the knob grows in as the pointer comes.
    let grow = ui.ctx().animate_bool_with_time(ui.id().with("seek-grow"), active, 0.12);
    let line_y = rect.top() + 1.0;
    let thickness = 2.0 + 2.0 * grow;
    let painter = &overlay(ui);
    let track = Rect::from_min_max(pos2(rect.left(), line_y - thickness / 2.0), pos2(rect.right(), line_y + thickness / 2.0));
    painter.rect_filled(track, CornerRadius::ZERO, Color32::from_white_alpha(28));
    let Some(duration) = duration.filter(|d| *d > 0.0) else {
        if app.busy() {
            // An indeterminate sweep while the song loads.
            let t = ui.input(|i| i.time) as f32;
            let w = rect.width() * 0.25;
            let x = rect.left() + (t * 0.8).fract() * (rect.width() + w) - w;
            painter.rect_filled(
                Rect::from_min_max(pos2(x.max(rect.left()), track.top()), pos2((x + w).min(rect.right()), track.bottom())),
                CornerRadius::ZERO,
                theme::accent().gamma_multiply(0.7),
            );
            theme::next_frame(ui.ctx());
        }
        return;
    };
    let downloaded = app.player.status().downloaded.clamp(0.0, 1.0);
    if downloaded > 0.0 && app.player.status().token != 0 {
        painter.rect_filled(
            Rect::from_min_max(track.min, pos2(rect.left() + rect.width() * downloaded, track.bottom())),
            CornerRadius::ZERO,
            Color32::from_white_alpha(30),
        );
    }
    let drag_id = ui.id().with("seek-drag");
    let mut position = app.position();
    if let Some(pointer) = response.interact_pointer_pos()
        && (response.dragged() || response.is_pointer_button_down_on())
    {
        let f = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        position = f64::from(f) * duration;
        ui.data_mut(|d| d.insert_temp(drag_id, position));
    }
    if response.drag_stopped() || response.clicked() {
        if let Some(to) = ui.data_mut(|d| d.remove_temp::<f64>(drag_id)) {
            app.actions.push(Action::Seek(to));
        }
    } else if let Some(dragging) = ui.data(|d| d.get_temp::<f64>(drag_id)) {
        position = dragging;
    }
    let f = (position / duration).clamp(0.0, 1.0) as f32;
    let x = rect.left() + rect.width() * f;
    painter.rect_filled(
        Rect::from_min_max(track.min, pos2(x, track.bottom())),
        CornerRadius::ZERO,
        theme::accent(),
    );
    if grow > 0.0 {
        painter.circle_filled(pos2(x, line_y), 7.0 * grow, theme::accent());
    }
    if active {
        if let Some(hover) = response.hover_pos() {
            let f = ((hover.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let at = (f64::from(f) * duration) as u32;
            let text = format_duration(at);
            let galley = painter.layout_no_wrap(text, theme::medium(12.0), theme::text());
            let bubble = Rect::from_center_size(pos2(hover.x.clamp(rect.left() + 24.0, rect.right() - 24.0), line_y - 18.0), galley.size() + vec2(12.0, 6.0));
            let layer = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.id().with("seek-tip")));
            layer.rect_filled(bubble, CornerRadius::same(4), theme::popup());
            layer.galley(bubble.center() - galley.size() / 2.0, galley, theme::text());
        }
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
}

fn transport(app: &mut App, ui: &mut Ui) {
    ui.spacing_mut().item_spacing.x = 2.0;
    if widgets::icon_button(ui, Icon::Previous, 22.0, theme::text(), "Previous (Ctrl+Left)").clicked() {
        app.actions.push(Action::Previous);
    }
    // Play or pause, in a white circle, or a spinner while loading.
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let playing = app.playing();
    let busy = app.busy();
    let fill = if response.hovered() { Color32::from_gray(225) } else { Color32::WHITE };
    ui.painter().circle_filled(rect.center(), 22.0, fill);
    if busy {
        let t = ui.input(|i| i.time) as f32;
        let start = t * 5.0;
        let points: Vec<_> = (0..=24)
            .map(|i| {
                let a = start + i as f32 / 24.0 * std::f32::consts::PI * 1.4;
                rect.center() + vec2(a.cos(), a.sin()) * 11.0
            })
            .collect();
        ui.painter().add(egui::Shape::line(points, Stroke::new(2.5, Color32::BLACK)));
        theme::next_frame(ui.ctx());
    } else {
        // Play turns into pause, and back, with a quick scale and fade.
        let p = ui.ctx().animate_bool_with_time(response.id.with("morph"), playing, 0.16);
        let press = if response.is_pointer_button_down_on() { 0.9 } else { 1.0 };
        for (icon, amount, offset) in [(Icon::Pause, p, 0.0), (Icon::Play, 1.0 - p, 1.5)] {
            if amount <= 0.01 {
                continue;
            }
            let size = 22.0 * (0.6 + 0.4 * amount) * press;
            icon.image(Color32::BLACK.gamma_multiply(amount), size)
                .paint_at(ui, Rect::from_center_size(rect.center() + vec2(offset, 0.0), Vec2::splat(size)));
        }
    }
    if response.on_hover_text(if playing { "Pause (Space)" } else { "Play (Space)" }).clicked() {
        app.actions.push(Action::TogglePlay);
    }
    if widgets::icon_button(ui, Icon::Next, 22.0, theme::text(), "Next (Ctrl+Right)").clicked() {
        app.actions.push(Action::Next);
    }
    ui.add_space(12.0);
    if let Some(duration) = app.duration() {
        let position = app.position().min(duration);
        ui.label(
            egui::RichText::new(format!(
                "{} / {}",
                format_duration(position as u32),
                format_duration(duration as u32)
            ))
            .font(theme::regular(12.5))
            .color(theme::text_dim()),
        );
        if app.playing() {
            theme::repaint_in(ui.ctx(), std::time::Duration::from_millis(250));
        }
    }
}

fn now(app: &mut App, ui: &mut Ui) {
    let Some(track) = app.current().cloned() else {
        return;
    };
    let width = ui.available_width();
    ui.spacing_mut().item_spacing.x = 12.0;
    let (art, art_response) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
    // With Last.fm connected, how often the person has played the song and
    // its artist there, before its logo.
    let plays = match track.artists.first() {
        Some(artist) if app.lastfm.status() == crate::lastfm::Status::Connected => {
            Some((app.lastfm.plays(&artist.name, track.romanized_title()), artist.name.clone()))
        }
        _ => None,
    };
    let counts = plays.as_ref().map(|(plays, _)| {
        let painter = ui.painter();
        let font = theme::regular(11.5);
        [plays.and_then(|p| p.track), plays.and_then(|p| p.artist)]
            .map(|n| painter.layout_no_wrap(n.map_or_else(|| "\u{2013}".to_owned(), thousands), font.clone(), theme::text_dim()))
    });
    let counts_w = counts.as_ref().map_or(0.0, |[a, b]| 16.0 + a.size().x.max(b.size().x));
    let lastfm_w = if plays.is_some() { counts_w + 12.0 + 38.0 + 12.0 } else { 0.0 };
    let full = app.full_view;
    let mut cx = app.cx();
    widgets::paint_art(ui, cx.images, track.thumbnail.as_deref(), art, 8.0, false);
    let art_response = art_response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(if full { "Close the player (Esc)" } else { "Open the player (F)" });
    if art_response.clicked() {
        cx.push(Action::ToggleFullView);
    }
    // Dislike, like and more: three icon buttons and their gaps (a song on
    // this PC has only More).
    let local = track.is_local();
    let buttons_w = if local { 38.0 + 12.0 } else { 3.0 * 38.0 + 2.0 * 12.0 + 12.0 } + lastfm_w;
    let text_w = (width - 48.0 - 12.0 - buttons_w).max(60.0);
    ui.allocate_ui_with_layout(vec2(text_w, 44.0), Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.set_width(text_w);
        let shown = cx.title(&track.title).to_owned();
        let response = widgets::clipped_label(ui, &shown, theme::medium(15.0), theme::text(), text_w, Sense::hover());
        if shown != track.title {
            response.on_hover_text(&track.title);
        }
        let mut links = track.artists.clone();
        if let Some(album) = &track.album {
            links.push(album.clone());
        }
        if links.is_empty() {
            widgets::clipped_label(ui, track.extra.as_deref().unwrap_or(""), theme::regular(13.0), theme::text_dim(), text_w, Sense::hover());
        } else {
            widgets::links(ui, &mut cx, &links, theme::regular(13.0), theme::text_dim(), text_w);
        }
    });
    let like = app.now.like.filter(|_| app.now.video_id == track.video_id);
    let (dislike_icon, like_icon) = match like {
        Some(Like::Liked) => (Icon::Dislike, Icon::Liked),
        Some(Like::Disliked) => (Icon::Disliked, Icon::Like),
        _ => (Icon::Dislike, Icon::Like),
    };
    // Liking pops the thumb: up a third, and back.
    let pop_id = ui.id().with("like-pop");
    let now = ui.input(|i| i.time);
    let popped_at: f64 = ui.data(|d| d.get_temp(pop_id)).unwrap_or(-10.0);
    let since = (now - popped_at) as f32;
    let pop = if since < 0.35 {
        theme::next_frame(ui.ctx());
        1.0 + 0.32 * (since / 0.35 * std::f32::consts::PI).sin()
    } else {
        1.0
    };
    if let (Some((plays, artist)), Some([song, by])) = (&plays, counts) {
        let (rect, response) = ui.allocate_exact_size(vec2(counts_w, 36.0), Sense::hover());
        let shown = ui.ctx().animate_bool_with_time(ui.id().with(("plays", &track.video_id)), plays.is_some(), 0.25);
        for (i, (icon, galley)) in [(Icon::Music, song), (Icon::User, by)].into_iter().enumerate() {
            let y = rect.center().y + if i == 0 { -8.5 } else { 8.5 };
            icon.image(theme::text_faint().gamma_multiply(shown), 11.0)
                .paint_at(ui, Rect::from_center_size(pos2(rect.left() + 5.5, y), Vec2::splat(11.0)));
            ui.painter().galley_with_override_text_color(
                pos2(rect.left() + 16.0, y - galley.size().y / 2.0),
                galley,
                theme::text_dim().gamma_multiply(shown),
            );
        }
        if let Some(plays) = plays {
            let times = |n: Option<u64>| match n {
                Some(1) => "once".to_owned(),
                Some(n) => format!("{} times", thousands(n)),
                None => "\u{2013}".to_owned(),
            };
            response.on_hover_text(format!(
                "On Last.fm you've played this song {}, and {artist} {}",
                times(plays.track),
                times(plays.artist)
            ));
        }
        if widgets::icon_button(ui, Icon::LastFm, 20.0, theme::text(), "Open the song on Last.fm").clicked() {
            app.actions.push(Action::OpenUrl(crate::lastfm::LastFm::track_url(artist, track.romanized_title())));
        }
    }
    let mut cx = app.cx();
    if !local && widgets::icon_button(ui, dislike_icon, 20.0, theme::text(), "Dislike").clicked() {
        cx.push(Action::Rate(
            track.video_id.clone(),
            if like == Some(Like::Disliked) { Rating::None } else { Rating::Dislike },
        ));
    }
    if !local && widgets::icon_button_scaled(ui, like_icon, 20.0, theme::text(), "Like", pop).clicked() {
        if like != Some(Like::Liked) {
            ui.data_mut(|d| d.insert_temp(pop_id, now));
        }
        cx.push(Action::Rate(
            track.video_id.clone(),
            if like == Some(Like::Liked) { Rating::None } else { Rating::Like },
        ));
    }
    let more = widgets::icon_button(ui, Icon::More, 20.0, theme::text(), "More");
    egui::Popup::menu(&more).show(|ui| {
        let uid = app.queue.current().map_or(0, |e| e.uid);
        let mut cx = app.cx();
        widgets::track_menu(ui, &mut cx, &track, RowPlace::Queue(uid));
    });
}

fn extras(app: &mut App, ui: &mut Ui) {
    ui.spacing_mut().item_spacing.x = 2.0;
    let (icon, tip) = if app.full_view {
        (Icon::Collapse, "Close the player (Esc)")
    } else {
        (Icon::Expand, "Open the player (F)")
    };
    let (screen_icon, screen_tip) = if app.fullscreen {
        (Icon::ExitFullscreen, "Exit full screen (Esc)")
    } else {
        (Icon::Fullscreen, "Full screen (F11)")
    };
    if widgets::icon_button(ui, screen_icon, 18.0, theme::text_dim(), screen_tip).clicked() {
        app.actions.push(Action::ToggleFullscreen);
    }
    if widgets::icon_button(ui, icon, 19.0, theme::text_dim(), tip).clicked() {
        app.actions.push(Action::ToggleFullView);
    }
    // The visualiser: a click turns it on or off; the right button picks
    // how it draws, and what it follows.
    let (on, mode, focus) = (app.settings.visualiser, app.settings.visualiser_mode, app.settings.visualiser_focus);
    let tip = if on { "Visualiser is on (right-click for its styles)" } else { "Visualiser" };
    let r = widgets::icon_button(ui, Icon::Visualiser, 20.0, if on { theme::text() } else { theme::text_faint() }, tip);
    if on {
        dot(ui, r.rect);
    }
    if r.clicked() {
        app.actions.push(Action::SetVisualiser { on: !on, mode, focus });
    }
    r.context_menu(|ui| {
        for choice in crate::visualiser::Mode::ALL {
            let picked = on && choice == mode;
            if ui.add(egui::Button::new(choice.name()).selected(picked)).clicked() {
                app.actions.push(Action::SetVisualiser { on: true, mode: choice, focus });
                ui.close();
            }
        }
        ui.separator();
        ui.label(egui::RichText::new("Reacts to").font(theme::regular(12.0)).color(theme::text_dim()));
        for choice in crate::visualiser::Focus::ALL {
            if ui.add(egui::Button::new(choice.name()).selected(choice == focus)).clicked() {
                app.actions.push(Action::SetVisualiser { on: true, mode, focus: choice });
                ui.close();
            }
        }
        if on {
            ui.separator();
            if ui.button("Turn off").clicked() {
                app.actions.push(Action::SetVisualiser { on: false, mode, focus });
                ui.close();
            }
        }
    });
    let (lyrics_on, queue_on) = if app.full_view {
        (app.full_tab == SideTab::Lyrics, app.full_tab == SideTab::UpNext)
    } else {
        (
            app.settings.side_panel && app.side_tab == SideTab::Lyrics,
            app.settings.side_panel && app.side_tab == SideTab::UpNext,
        )
    };
    // In the full view these pick its tab; the panel there never closes.
    let full = app.full_view;
    if widgets::icon_button(ui, Icon::Lyrics, 20.0, if lyrics_on { theme::text() } else { theme::text_dim() }, "Lyrics").clicked() {
        app.actions.push(if lyrics_on && !full { Action::ToggleSidePanel } else { Action::ShowSidePanel(SideTab::Lyrics) });
    }
    if widgets::icon_button(ui, Icon::Queue, 20.0, if queue_on { theme::text() } else { theme::text_dim() }, "Up next (Q)").clicked() {
        app.actions.push(if queue_on && !full { Action::ToggleSidePanel } else { Action::ShowSidePanel(SideTab::UpNext) });
    }
    let shuffle = app.queue.shuffle;
    let shuffle_tint = if shuffle { theme::text() } else { theme::text_faint() };
    let r = widgets::icon_button(ui, Icon::Shuffle, 20.0, shuffle_tint, if shuffle { "Shuffle is on" } else { "Shuffle" });
    if shuffle {
        dot(ui, r.rect);
    }
    if r.clicked() {
        app.actions.push(Action::ToggleShuffle);
    }
    let (icon, tip, on) = match app.settings.repeat {
        Repeat::Off => (Icon::Repeat, "Repeat", false),
        Repeat::All => (Icon::Repeat, "Repeat all", true),
        Repeat::One => (Icon::RepeatOne, "Repeat one", true),
    };
    let r = widgets::icon_button(ui, icon, 20.0, if on { theme::text() } else { theme::text_faint() }, tip);
    if on {
        dot(ui, r.rect);
    }
    if r.clicked() {
        app.actions.push(Action::CycleRepeat);
    }
    ui.add_space(8.0);
    volume(app, ui);
}


/// The small dot under a toggle that is on.
fn dot(ui: &Ui, rect: Rect) {
    ui.painter()
        .circle_filled(pos2(rect.center().x, rect.bottom() - 3.0), 2.0, theme::accent());
}

fn volume(app: &mut App, ui: &mut Ui) {
    let (rect, response) = ui.allocate_exact_size(vec2(96.0, 28.0), Sense::click_and_drag());
    let volume = if app.settings.muted { 0.0 } else { app.settings.volume };
    let line = Rect::from_min_max(pos2(rect.left() + 4.0, rect.center().y - 2.0), pos2(rect.right() - 4.0, rect.center().y + 2.0));
    ui.painter().rect_filled(line, CornerRadius::same(2), Color32::from_white_alpha(40));
    let x = line.left() + line.width() * volume;
    ui.painter().rect_filled(Rect::from_min_max(line.min, pos2(x, line.bottom())), CornerRadius::same(2), theme::text());
    if response.hovered() || response.dragged() {
        ui.painter().circle_filled(pos2(x, line.center().y), 6.0, theme::text());
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if let Some(pointer) = response.interact_pointer_pos()
        && (response.dragged() || response.clicked() || response.is_pointer_button_down_on())
    {
        let f = ((pointer.x - line.left()) / line.width()).clamp(0.0, 1.0);
        app.actions.push(Action::SetVolume(f));
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            app.actions.push(Action::SetVolume((volume + scroll / 600.0).clamp(0.0, 1.0)));
        }
    }
    if response.drag_stopped() {
        app.save_settings();
    }
    let response = response.on_hover_text(format!("Volume {}%", (volume * 100.0).round()));
    let _ = response;
    let icon = if volume == 0.0 {
        Icon::Mute
    } else if volume < 0.5 {
        Icon::VolumeLow
    } else {
        Icon::Volume
    };
    if widgets::icon_button(ui, icon, 20.0, theme::text(), if app.settings.muted { "Unmute (M)" } else { "Mute (M)" }).clicked() {
        app.actions.push(Action::ToggleMute);
    }
    let _ = Align2::CENTER_CENTER;
}
