//! Listening stats: what the person listened to most over a stretch of
//! time, from Last.fm or from the plays the app has kept. Totals first,
//! then how the plays fell over the days, then the top artists, songs and
//! albums.

use egui::{Align, Color32, CornerRadius, CursorIcon, Layout, Rect, RichText, Sense, Ui, Vec2, pos2, vec2};

use crate::api::model::{Item, Kind, Target};
use crate::app::App;
use crate::model::{Action, Loadable, Route};
use crate::stats::{Bins, Period, Ranked, Source, Summary};
use crate::theme::{self, Icon};
use crate::ui::widgets::{self, Cx};

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.add_space(8.0);
    ui.label(RichText::new("Your stats").font(theme::bold(30.0)));
    ui.add_space(14.0);
    let (source, period) = (app.stats.source, app.stats.period);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (s, label) in [(Source::LastFm, "Last.fm"), (Source::App, "This app")] {
            let tip = match s {
                Source::LastFm => "Your scrobbles on Last.fm, from every device",
                Source::App => "The plays this app has heard, songs on this PC among them",
            };
            if widgets::chip(ui, label, source == s).on_hover_text(tip).clicked() && source != s {
                app.actions.push(Action::ShowStats { source: s, period });
            }
        }
        // A thin rule between the two choices.
        let (rule, _) = ui.allocate_exact_size(vec2(14.0, 28.0), Sense::hover());
        ui.painter().vline(rule.center().x, rule.y_range().shrink(4.0), egui::Stroke::new(1.0, theme::divider()));
        for p in Period::ALL {
            if widgets::chip(ui, p.label(), period == p).clicked() && period != p {
                app.actions.push(Action::ShowStats { source, period: p });
            }
        }
    });
    ui.add_space(24.0);
    let entry = app.stats.summaries.get(&(source, period)).map(|(summary, _, _)| match summary {
        Loadable::Ready(summary) => Ok(summary.clone()),
        Loadable::Failed(error) => Err(Some(error.clone())),
        Loadable::Loading => Err(None),
    });
    match entry {
        Some(Ok(summary)) => body(app, ui, &summary, source, period),
        Some(Err(Some(error))) => {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                Icon::LastFm.image(theme::text_dim(), 18.0).paint_at(ui, rect);
                ui.label(RichText::new(error).color(theme::text_dim()));
            });
            if !connected(app) {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if widgets::pill(ui, Some(Icon::Settings), "Open Settings", false).clicked() {
                        app.actions.push(Action::Navigate(Route::Settings));
                    }
                    if widgets::pill(ui, None, "See this app's instead", false).clicked() {
                        app.actions.push(Action::ShowStats { source: Source::App, period });
                    }
                });
            }
        }
        _ => widgets::spinner(ui),
    }
}

fn connected(app: &App) -> bool {
    app.lastfm.reader().is_some_and(|r| r.user().is_some())
}

fn body(app: &mut App, ui: &mut Ui, summary: &Summary, source: Source, period: Period) {
    if summary.plays == Some(0) || (summary.top_artists.is_empty() && summary.top_songs.is_empty()) {
        ui.add_space(12.0);
        let text = match source {
            Source::App if summary.since.is_none() => {
                "Nothing yet. The songs you hear for half their length, or 30 seconds, are counted here from now on.".to_owned()
            }
            _ => format!("No plays in the last {} yet.", period.label().to_lowercase()),
        };
        ui.label(RichText::new(text).color(theme::text_dim()));
        return;
    }
    let locals = app.local.album_list();
    let local_songs = app.local.songs();
    let mut cx = app.cx();

    // The totals, as tiles.
    let mut tiles: Vec<(String, &str)> = Vec::new();
    if let Some(plays) = summary.plays {
        tiles.push((widgets::thousands(plays), if source == Source::LastFm { "scrobbles" } else { "plays" }));
    }
    if let Some(seconds) = summary.seconds {
        tiles.push((listening_time(seconds), "listened"));
    }
    if let Some(n) = summary.artists {
        tiles.push((widgets::thousands(n), if n == 1 { "artist" } else { "artists" }));
    }
    if let Some(n) = summary.songs {
        tiles.push((widgets::thousands(n), if n == 1 { "song" } else { "songs" }));
    }
    let gap = 12.0;
    let tile_w = ((ui.available_width() - gap * (tiles.len().max(1) as f32 - 1.0)) / tiles.len().max(1) as f32).clamp(140.0, 260.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        for (value, label) in &tiles {
            let (rect, _) = ui.allocate_exact_size(vec2(tile_w, 84.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(12), theme::surface());
            ui.painter().text(pos2(rect.left() + 18.0, rect.top() + 16.0), egui::Align2::LEFT_TOP, value, theme::bold(26.0), theme::text());
            ui.painter().text(pos2(rect.left() + 18.0, rect.bottom() - 16.0), egui::Align2::LEFT_BOTTOM, *label, theme::regular(13.5), theme::text_dim());
        }
    });

    // How the plays fell over the stretch.
    if summary.bins.iter().any(|n| *n > 0) {
        ui.add_space(26.0);
        let title = match summary.bin {
            Bins::Days => "Each day",
            Bins::Weeks => "Each week",
            Bins::Months => "Each month",
        };
        ui.label(RichText::new(title).font(theme::semibold(15.0)).color(theme::text_dim()));
        ui.add_space(10.0);
        activity(ui, summary);
    }

    // The top artists and songs, side by side when there is room.
    ui.add_space(30.0);
    let wide = ui.available_width() > 880.0;
    let column_w = if wide { (ui.available_width() - 32.0) / 2.0 } else { ui.available_width() };
    let mut artist_clicked = None;
    let mut song_clicked = None;
    let mut columns = |ui: &mut Ui, cx: &mut Cx<'_>, which: u8| {
        ui.allocate_ui_with_layout(vec2(column_w, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(column_w);
            let (title, list, round) = if which == 0 {
                ("Top artists", &summary.top_artists, true)
            } else {
                ("Top songs", &summary.top_songs, false)
            };
            ui.label(RichText::new(title).font(theme::bold(22.0)));
            ui.add_space(10.0);
            let most = list.iter().map(|r| r.plays).max().unwrap_or(1).max(1);
            for (i, ranked) in list.iter().enumerate() {
                if ranked_row(ui, cx, i + 1, ranked, most, round, column_w).clicked() {
                    if which == 0 {
                        artist_clicked = Some(ranked.name.clone());
                    } else {
                        song_clicked = Some(ranked.clone());
                    }
                }
            }
        });
    };
    if wide {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 32.0;
            columns(ui, &mut cx, 0);
            columns(ui, &mut cx, 1);
        });
    } else {
        columns(ui, &mut cx, 0);
        ui.add_space(26.0);
        columns(ui, &mut cx, 1);
    }
    if let Some(name) = artist_clicked {
        cx.push(Action::Navigate(Route::Artist(name)));
    }
    if let Some(song) = song_clicked {
        // The song the app heard plays again (from this PC when it is
        // one of its own); a Last.fm one is found on YouTube Music.
        match song.track {
            Some(track) if crate::local::is_local_id(&track.video_id) => {
                if let Some(found) = local_songs.iter().find(|s| s.video_id == track.video_id) {
                    cx.push(Action::PlayTrack(found.clone()));
                }
            }
            Some(track) => cx.push(Action::PlayTrack(track)),
            None => cx.push(Action::PlaySong {
                query: format!("{} {}", song.by.clone().unwrap_or_default(), song.name).trim().to_owned(),
                artist: song.by.clone(),
            }),
        }
    }

    // The top albums, as covers.
    if !summary.top_albums.is_empty() {
        ui.add_space(30.0);
        ui.label(RichText::new("Top albums").font(theme::bold(22.0)));
        ui.add_space(10.0);
        let items: Vec<Item> = summary
            .top_albums
            .iter()
            .map(|album| {
                let by = album.by.clone().unwrap_or_default();
                // One of this PC's opens its page there; any other is looked up.
                let local = locals.iter().find(|l| l.title.eq_ignore_ascii_case(&album.name) && (by.is_empty() || l.artist.eq_ignore_ascii_case(&by)));
                let target = match local {
                    Some(l) => Target::Browse { browse_id: format!("{}{}", crate::local::ALBUM, l.key), params: None },
                    None => Target::Search { query: format!("{by} {}", album.name).trim().to_owned(), params: None },
                };
                let plays = if album.plays == 1 { "1 play".to_owned() } else { format!("{} plays", widgets::thousands(album.plays)) };
                Item {
                    kind: Kind::Album,
                    title: album.name.clone(),
                    subtitle: if by.is_empty() { plays } else { format!("{by} \u{2022} {plays}") },
                    thumbnail: album.art.clone().or_else(|| local.and_then(|l| l.art.clone())),
                    target: Some(target),
                    track: None,
                    play_playlist: None,
                    color: None,
                }
            })
            .collect();
        widgets::card_grid(ui, &mut cx, &items);
    }

    if let Some(since) = summary.since {
        ui.add_space(24.0);
        let text = match source {
            Source::LastFm => format!("Scrobbling since {}", month_year(since)),
            Source::App => format!("Counting plays since {}", month_year(since)),
        };
        ui.label(RichText::new(text).color(theme::text_faint()).font(theme::regular(12.5)));
    }
}

/// One of a top list: its rank, picture, name and plays, over a faint bar
/// of its plays against the first's.
fn ranked_row(ui: &mut Ui, cx: &mut Cx<'_>, rank: usize, ranked: &Ranked, most: u64, round: bool, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 56.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let share = ranked.plays as f32 / most as f32;
    let bar = Rect::from_min_size(rect.min, vec2(rect.width() * share, rect.height()));
    ui.painter().rect_filled(bar, CornerRadius::same(8), theme::accent().gamma_multiply(0.09));
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), Color32::from_white_alpha(12));
        widgets::hover_glow(ui, rect, 8.0, 1.0);
    }
    let center = rect.center().y;
    ui.painter().text(pos2(rect.left() + 18.0, center), egui::Align2::CENTER_CENTER, rank.to_string(), theme::semibold(14.0), theme::text_dim());
    let mut x = rect.left() + 38.0;
    if ranked.art.is_some() || round {
        let art = Rect::from_min_size(pos2(x, center - 20.0), Vec2::splat(40.0));
        widgets::paint_art(ui, cx.images, ranked.art.as_deref(), art, 4.0, round);
        x = art.right() + 12.0;
    }
    let plays = if ranked.plays == 1 { "1 play".to_owned() } else { format!("{} plays", widgets::thousands(ranked.plays)) };
    let plays_g = ui.painter().layout_no_wrap(plays, theme::regular(13.0), theme::text_dim());
    let text_w = (rect.right() - 12.0 - plays_g.size().x - 12.0 - x).max(40.0);
    let name = ui.painter().layout(ranked.name.clone(), theme::medium(14.5), theme::text(), f32::INFINITY);
    let clip = Rect::from_min_max(pos2(x, rect.top()), pos2(x + text_w, rect.bottom()));
    let painter = ui.painter().with_clip_rect(clip);
    match &ranked.by {
        Some(by) => {
            painter.galley(pos2(x, center - name.size().y - 1.0), name, theme::text());
            painter.text(pos2(x, center + 1.0), egui::Align2::LEFT_TOP, by, theme::regular(12.5), theme::text_dim());
        }
        None => painter.galley(pos2(x, center - name.size().y / 2.0), name, theme::text()),
    }
    ui.painter().galley(pos2(rect.right() - 12.0 - plays_g.size().x, center - plays_g.size().y / 2.0), plays_g, theme::text_dim());
    response
}

/// The plays over the stretch as bars, oldest first; pointing at one says
/// how many.
fn activity(ui: &mut Ui, summary: &Summary) {
    let bins = &summary.bins;
    let width = ui.available_width().min(900.0);
    let height = 96.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height + 18.0), Sense::hover());
    let chart = Rect::from_min_size(rect.min, vec2(width, height));
    let most = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
    let slot = width / bins.len() as f32;
    let bar_w = (slot * 0.68).clamp(3.0, 28.0);
    let pointer = response.hover_pos();
    let mut pointed = None;
    for (i, &n) in bins.iter().enumerate() {
        let x = chart.left() + slot * i as f32 + (slot - bar_w) / 2.0;
        let h = if n == 0 { 2.0 } else { (n as f32 / most * height).max(4.0) };
        let bar = Rect::from_min_max(pos2(x, chart.bottom() - h), pos2(x + bar_w, chart.bottom()));
        let hovered = pointer.is_some_and(|p| p.x >= chart.left() + slot * i as f32 && p.x < chart.left() + slot * (i + 1) as f32);
        if hovered {
            pointed = Some((i, n));
        }
        let color = if n == 0 {
            theme::surface_active()
        } else if hovered {
            theme::accent_text()
        } else {
            theme::accent().gamma_multiply(0.8)
        };
        ui.painter().rect_filled(bar, CornerRadius::same(3), color);
    }
    let unit = match summary.bin {
        Bins::Days => "days",
        Bins::Weeks => "weeks",
        Bins::Months => "months",
    };
    let label_y = chart.bottom() + 4.0;
    ui.painter().text(pos2(chart.left(), label_y), egui::Align2::LEFT_TOP, format!("{} {unit} ago", bins.len()), theme::regular(11.5), theme::text_faint());
    ui.painter().text(pos2(chart.right(), label_y), egui::Align2::RIGHT_TOP, "now", theme::regular(11.5), theme::text_faint());
    if let Some((i, n)) = pointed {
        let back = bins.len() - 1 - i;
        let when = match (summary.bin, back) {
            (Bins::Days, 0) => "Today".to_owned(),
            (Bins::Days, 1) => "Yesterday".to_owned(),
            (Bins::Days, b) => format!("{b} days ago"),
            (Bins::Weeks, 0) => "This week".to_owned(),
            (Bins::Weeks, b) => format!("{b} weeks ago"),
            (Bins::Months, 0) => "This month".to_owned(),
            (Bins::Months, b) => format!("{b} months ago"),
        };
        let plays = if n == 1 { "1 play".to_owned() } else { format!("{n} plays") };
        response.on_hover_text(format!("{when}: {plays}"));
    }
}

/// "3 h 20 min", or "45 min".
fn listening_time(seconds: u64) -> String {
    let minutes = (seconds + 30) / 60;
    match minutes {
        m if m >= 24 * 60 * 2 => format!("{} days", widgets::thousands(m / (24 * 60))),
        m if m >= 60 => format!("{} h {} min", m / 60, m % 60),
        m => format!("{m} min"),
    }
}

/// "October 2026", from Unix seconds.
fn month_year(unix: i64) -> String {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let (year, month, _) = civil(unix.div_euclid(86_400));
    format!("{} {year}", MONTHS[(month - 1) as usize])
}

/// The calendar date of a day counted from 1970-01-01 (Howard Hinnant's
/// algorithm): year, month (1 to 12), day.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times_read_well() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_377), (2025, 10, 16));
        assert_eq!(month_year(1_792_108_800), "October 2026");
        assert_eq!(listening_time(45 * 60), "45 min");
        assert_eq!(listening_time(3 * 3600 + 20 * 60), "3 h 20 min");
        assert_eq!(listening_time(5 * 86_400), "5 days");
    }
}
