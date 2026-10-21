//! The full-window player: the artwork large on the left over a wash of its
//! own colours, the title and artists under it, and what plays next or the
//! lyrics on the right. The player bar stays below.

use egui::{Align, Color32, CornerRadius, CursorIcon, Layout, Mesh, Rect, RichText, Sense, Shape, Ui, UiBuilder, pos2, vec2};

use crate::app::{App, Upgrade, UpgradePhase};
use crate::images::Images;
use crate::model::{Action, SideTab};
use crate::theme::{self, Icon};
use crate::ui::{lyrics, side_panel, widgets};

/// `rise`: how far below its place the content still is, opening; `foot`:
/// room kept free at the bottom (the floating bar's, in full screen);
/// `chrome`: how much the buttons along the top show (in full screen they
/// go with the bar while the mouse rests).
pub fn show(app: &mut App, ui: &mut Ui, rect: Rect, rise: f32, foot: f32, chrome: f32) {
    let track = app.current().cloned();
    let deck = track.as_ref().map(|t| deal(app, ui, t.thumbnail.clone()));
    let turning = deck.as_ref().and_then(|d| d.swap.as_ref()).map(|s| smooth(s.t));
    backdrop(app, ui, rect, track.as_ref().and_then(|t| t.thumbnail.as_deref()), turning);

    if chrome > 0.0 {
        let close = Rect::from_min_size(rect.min + vec2(20.0, 16.0 + rise), vec2(40.0, 40.0));
        let mut bar = ui.new_child(UiBuilder::new().max_rect(close).layout(Layout::left_to_right(Align::Center)));
        bar.multiply_opacity(chrome);
        if widgets::icon_button(&mut bar, Icon::ChevronDown, 26.0, theme::text(), "Close the player (Esc)").clicked() {
            app.actions.push(Action::ToggleFullView);
        }
        let corner = Rect::from_min_size(pos2(rect.right() - 60.0, rect.top() + 16.0 + rise), vec2(40.0, 40.0));
        let mut bar = ui.new_child(UiBuilder::new().max_rect(corner).layout(Layout::left_to_right(Align::Center)));
        bar.multiply_opacity(chrome);
        let (icon, tip) = if app.fullscreen {
            (Icon::ExitFullscreen, "Exit full screen (Esc)")
        } else {
            (Icon::Fullscreen, "Full screen (F11)")
        };
        if widgets::icon_button(&mut bar, icon, 22.0, theme::text(), tip).clicked() {
            app.actions.push(Action::ToggleFullscreen);
        }
    }

    let Some(track) = track else {
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
        child.add_space(rect.height() * 0.4);
        child.vertical_centered(|ui| {
            ui.label(RichText::new("Nothing is playing").font(theme::semibold(20.0)).color(theme::text_dim()));
        });
        return;
    };

    let content = Rect::from_min_max(rect.min + vec2(56.0, 72.0), rect.max - vec2(48.0, 28.0 + foot)).translate(vec2(0.0, rise));
    let narrow = content.width() < 820.0;
    let right_w = if narrow { content.width() * 0.48 } else { (content.width() * 0.4).clamp(360.0, 560.0) };
    let gap = if narrow { 32.0 } else { (content.width() * 0.07).clamp(56.0, 112.0) };
    // The artwork, as large as fits above its titles.
    let titles_h = 112.0;
    let art_size = (content.width() - right_w - gap).min(content.height() - titles_h).clamp(140.0, 660.0);
    // The artwork and the list as one piece, centred: the same margin on
    // either side, and the list close to the artwork it belongs to.
    let total = art_size + gap + right_w;
    let x0 = content.left() + ((content.width() - total) / 2.0).max(0.0);
    let left = Rect::from_min_max(pos2(x0, content.top()), pos2(x0 + art_size, content.bottom()));
    let right = Rect::from_min_max(pos2(left.right() + gap, content.top()), pos2((left.right() + gap + right_w).min(content.right()), content.bottom()));
    let block_h = art_size + titles_h;
    let art_top = left.top() + ((left.height() - block_h) / 2.0).max(0.0);
    let art = Rect::from_min_size(pos2(left.center().x - art_size / 2.0, art_top), vec2(art_size, art_size));
    // Room beside the artwork for the card that peeks out from behind it.
    let room = (art.left() - rect.left() - 24.0, right.left() - art.right() - 16.0);
    let moved = deck.as_ref().map_or(0.0, |deck| paint_deck(app, ui, deck, art, room));

    let titles = Rect::from_min_size(pos2(art.left() + moved, art.bottom() + 24.0), vec2(art.width(), titles_h - 24.0));
    let mut text = ui.new_child(UiBuilder::new().max_rect(titles).layout(Layout::top_down(Align::Min)));
    text.spacing_mut().item_spacing.y = 4.0;
    // The titles fade in with the new song.
    let fade = theme::appear(ui.ctx(), ui.id().with("np-titles"), egui::Id::new(&track.video_id).value(), 0.3);
    text.multiply_opacity(fade);
    let width = titles.width();
    let big = art_size > 420.0;
    let title_font = theme::bold(if big { 28.0 } else { 22.0 });
    let shown = app.shown_title(&track).to_owned();
    // The year it came out, small and grey, just after the title.
    let year = track.year.or_else(|| (app.now.video_id == track.video_id).then_some(app.now.year).flatten());
    let year = year.map(|y| text.painter().layout_no_wrap(y.to_string(), theme::medium(if big { 16.0 } else { 14.0 }), theme::text_dim()));
    let year_w = year.as_ref().map_or(0.0, |g| g.size().x + 12.0);
    let response = widgets::clipped_label(&mut text, &shown, title_font, theme::text(), width - year_w, Sense::hover());
    if let Some(year) = year {
        // On the title's baseline, four fifths down each line.
        let title = response.rect;
        let at = pos2(title.right() + 12.0, title.top() + title.height() * 0.8 - year.size().y * 0.8);
        text.painter().galley(at, year, theme::text_dim().gamma_multiply(0.9));
    }
    if shown != track.title {
        response.on_hover_text(&track.title);
    }
    let mut links = track.artists.clone();
    if let Some(album) = &track.album {
        links.push(album.clone());
    }
    let quality = app.stream_quality().map(str::to_owned);
    let upgrade = app.upgrade.clone().filter(|u| u.token == app.current_token());
    let mut cx = app.cx();
    if links.is_empty() {
        widgets::clipped_label(&mut text, track.extra.as_deref().unwrap_or(""), theme::medium(17.0), theme::text_dim(), width, Sense::hover());
    } else {
        widgets::links(&mut text, &mut cx, &links, theme::medium(17.0), theme::text_dim(), width);
    }
    if quality.is_some() || upgrade.is_some() {
        text.add_space(6.0);
        quality_chip(&mut text, quality.as_deref(), upgrade.as_ref());
    }

    // What plays next, or the words.
    let mut side = ui.new_child(UiBuilder::new().max_rect(right).layout(Layout::top_down(Align::Min)));
    // egui fades a list's edges into the panel colour, which is not what
    // shows here: the backdrop is.
    side.spacing_mut().scroll.fade.strength = 0.0;
    let (picked, _) = widgets::tab_header(&mut side, app.full_tab, false);
    if let Some(tab) = picked {
        app.actions.push(Action::ShowSidePanel(tab));
    }
    side.add_space(12.0);
    match app.full_tab {
        SideTab::UpNext => side_panel::queue(app, &mut side),
        SideTab::Lyrics => lyrics::show(app, &mut side, true),
    }
}

// ---- The deck ---------------------------------------------------------------

/// How long before the next song comes in its artwork starts to peek out
/// from behind this one's, in seconds.
const PEEK_LEAD: f64 = 8.0;
/// How long the card behind takes to slide out, or back in.
const PEEK_SECS: f32 = 0.9;
/// The card behind, to the size of the one in front...
const BEHIND: f32 = 0.9;
/// ...poking out by this much of it...
const POKE: f32 = 0.2;
/// ...and this much darker.
const DIM: f32 = 0.35;
/// How far the card going slides away, of its size: clear of the one
/// coming forward, so it never shows through it.
const SLIDE: f32 = 1.05;
/// A turn with no crossfade to keep time with: a skip, a gapless change.
const TURN_SECS: f32 = 0.6;

/// The artwork as a deck of cards. Near a song's end the next one's
/// artwork peeks out from behind it; as the next song comes in, its card
/// comes to the front while this one slides away and fades, in time with
/// the crossfade. Going back, the card before slides in on top instead.
#[derive(Clone, Default)]
struct Deck {
    /// The entry in front, by uid, and its artwork.
    front: Option<u64>,
    front_url: Option<String>,
    /// The entry behind, its artwork, and how far out it is (0 to 1).
    behind: Option<u64>,
    behind_url: Option<String>,
    peek: f32,
    /// The card behind is wanted soon: its artwork is fetched ahead.
    warm: bool,
    /// The pass it was last drawn in.
    pass: u64,
    swap: Option<Swap>,
}

#[derive(Clone)]
struct Swap {
    /// The artwork of the card going.
    leaving: Option<String>,
    /// The next came forward; otherwise the one before came back.
    forward: bool,
    /// How far out the card coming forward already was.
    peeked: f32,
    clock: Clock,
    /// How far along, from 0 to 1, never going back.
    t: f32,
}

#[derive(Clone, Copy)]
enum Clock {
    /// In time with the crossfade: the song coming in, from its second
    /// `from`, over `overlap` seconds.
    Music { from: f64, overlap: f64 },
    /// On its own, from `since`.
    Wall { since: f64 },
}

/// The deck moved on to this frame, `url` the artwork of the song in front.
fn deal(app: &mut App, ui: &Ui, url: Option<String>) -> Deck {
    let id = ui.id().with("np-deck");
    let mut deck: Deck = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let pass = ui.ctx().cumulative_pass_nr();
    // Not drawn last time (the player was closed): nothing to turn from.
    if deck.pass + 1 < pass {
        deck = Deck::default();
    }
    deck.pass = pass;
    let now = ui.input(|i| i.time);
    let dt = ui.input(|i| i.stable_dt).min(0.1);

    let uid = app.queue.current().map(|e| e.uid);
    if deck.front.is_some() && uid != deck.front {
        // Back one: the song that was in front comes after this one again.
        let was = deck.front.and_then(|u| app.queue.index_of(u));
        let back = deck.behind != uid && matches!((was, app.queue.current), (Some(w), Some(n)) if n + 1 == w);
        let clock = match app.player.blend() {
            Some(b) if !back && b.token == app.current_token() && b.overlap.as_secs_f64() >= 0.5 => Clock::Music {
                from: b.from.as_secs_f64(),
                overlap: b.overlap.as_secs_f64(),
            },
            _ => Clock::Wall { since: now },
        };
        let peeked = if deck.behind == uid { deck.peek } else { 0.0 };
        let leaving = deck.front_url.take();
        // The same artwork (the next song of one album): nothing to turn.
        deck.swap = (leaving != url).then_some(Swap { leaving, forward: !back, peeked, clock, t: 0.0 });
        deck.behind = None;
        deck.behind_url = None;
        deck.peek = 0.0;
    }
    deck.front = uid;
    deck.front_url = url;

    if let Some(swap) = &mut deck.swap {
        let t = match swap.clock {
            Clock::Music { from, overlap } => {
                let at = app.position();
                // Sought back out of the crossfade: it is over.
                if at < from - 0.5 { 1.0 } else { ((at - from) / overlap) as f32 }
            }
            Clock::Wall { since } => (now - since) as f32 / TURN_SECS,
        };
        swap.t = swap.t.max(t.clamp(0.0, 1.0));
        theme::next_frame(ui.ctx());
    }
    if deck.swap.as_ref().is_some_and(|s| s.t >= 1.0) {
        deck.swap = None;
    }

    // The card behind comes out once the next song is a few seconds off:
    // where the crossfade has it come in, or else at the end.
    let mut out = false;
    deck.warm = false;
    match app.coming_up().map(|(uid, token, t)| (uid, token, t.thumbnail.clone())) {
        Some((next, token, next_url)) => {
            if deck.behind != Some(next) {
                deck.behind = Some(next);
                deck.peek = 0.0;
            }
            deck.behind_url = next_url;
            let comes_in = app
                .player
                .blend()
                .filter(|b| token == Some(b.token))
                .map(|b| b.at.as_secs_f64())
                .or_else(|| app.duration());
            if let Some(comes_in) = comes_in {
                let left = comes_in - app.position();
                out = left <= PEEK_LEAD && deck.swap.is_none() && deck.behind_url != deck.front_url;
                deck.warm = left <= PEEK_LEAD + 20.0;
            }
        }
        // Nothing after this: the card behind goes back in, and away.
        None if deck.peek == 0.0 => {
            deck.behind = None;
            deck.behind_url = None;
        }
        None => {}
    }
    let step = dt / PEEK_SECS;
    let peek = if out { (deck.peek + step).min(1.0) } else { (deck.peek - step).max(0.0) };
    if peek != deck.peek || (out && peek < 1.0) {
        theme::next_frame(ui.ctx());
    }
    deck.peek = peek;
    ui.data_mut(|d| d.insert_temp(id, deck.clone()));
    deck
}

/// The deck drawn where the artwork goes, with `room` (to the left, to the
/// right) free beside it. How far the front card has moved over, for the
/// titles to keep to it.
fn paint_deck(app: &mut App, ui: &Ui, deck: &Deck, art: Rect, room: (f32, f32)) -> f32 {
    let size = art.width();
    let (left, right) = (room.0.max(0.0), room.1.max(0.0));
    // The card behind pokes out to the right. The deck stays about where
    // the artwork was: the front card moves left to make room.
    let poke = (size * POKE).min(left + right);
    let shift = (poke * 0.5).max(poke - right).min(left);
    let poke = poke.min(right + shift);
    let front_at = |q: f32| art.translate(vec2(-shift * q, 0.0));
    let behind_at = |q: f32| {
        let side = size * BEHIND;
        let edge = front_at(q).right() + poke * q;
        Rect::from_min_size(pos2(edge - side, art.center().y - side / 2.0), vec2(side, side))
    };
    // One image for each card however large it is drawn.
    let px = (size * ui.ctx().pixels_per_point()).round() as u32;
    let images = &mut app.images;
    if deck.warm
        && let Some(url) = &deck.behind_url
    {
        images.get(url, px);
        images.backdrop(url);
    }
    let card = |images: &mut Images, url: Option<&str>, rect: Rect, dim: f32, opacity: f32| {
        if opacity <= 0.0 {
            return;
        }
        let k = rect.width() / size.max(1.0);
        let shadow = egui::Shadow {
            offset: [0, (18.0 * k) as i8],
            blur: (60.0 * k) as u8,
            spread: 0,
            color: Color32::from_black_alpha((150.0 * opacity) as u8),
        };
        ui.painter().add(shadow.as_shape(rect, CornerRadius::same((12.0 * k) as u8)));
        let tint = Color32::from_gray(((1.0 - dim) * 255.0) as u8).gamma_multiply(opacity);
        widgets::paint_art_tinted(ui, images, url, rect, 12.0 * k, false, tint, Some(px));
    };

    let turning = deck.swap.is_some();
    let (last, shown, t) = art_change(ui, deck.front_url.as_deref(), turning);
    match &deck.swap {
        None => {
            let q = smooth(deck.peek);
            if q > 0.0 {
                card(images, deck.behind_url.as_deref(), behind_at(q), DIM, q);
            }
            // The front card's artwork arriving late fades in over what it
            // showed, settling from a touch smaller.
            let front = front_at(q);
            if t < 1.0 && last.is_some() {
                card(images, last.as_deref(), front, 0.0, 1.0 - t);
            }
            let settle = front.shrink(front.width() * 0.015 * (1.0 - t));
            card(images, shown.as_deref(), settle, 0.0, if last.is_some() { t } else { 1.0 });
            front.left() - art.left()
        }
        Some(swap) if swap.forward => {
            // The card behind comes forward and brightens; the one in
            // front is dealt off to the left, fading once it is clear.
            let e = smooth(swap.t);
            let q = smooth(swap.peeked);
            card(images, deck.front_url.as_deref(), behind_at(q).lerp_towards(&art, e), DIM * (1.0 - e), 1.0);
            let going = front_at(q).translate(vec2(-size * SLIDE * e, 0.0)).scale_from_center(1.0 - 0.06 * e);
            card(images, swap.leaving.as_deref(), going, 0.0, 1.0 - smooth((swap.t - 0.45) / 0.5));
            0.0
        }
        Some(swap) => {
            // Back: the card before slides in on top from the left, and the
            // one that was in front goes behind it.
            let e = smooth(swap.t);
            card(images, swap.leaving.as_deref(), art.lerp_towards(&behind_at(0.0), e), DIM * e, 1.0);
            let coming = art.translate(vec2(-size * SLIDE * (1.0 - e), 0.0)).scale_from_center(1.0 - 0.06 * (1.0 - e));
            card(images, deck.front_url.as_deref(), coming, 0.0, smooth(swap.t / 0.5));
            0.0
        }
    }
}

/// The front card's artwork changing under it, as when it arrives late:
/// the last one, the one shown, and how far that has faded in. While the
/// deck turns, the card coming forward is the change, and nothing fades
/// on it after.
fn art_change(ui: &Ui, url: Option<&str>, turning: bool) -> (Option<String>, Option<String>, f32) {
    let id = ui.id().with("np-art");
    let (last, shown): (Option<String>, Option<String>) = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let (last, shown) = if shown.as_deref() != url || (turning && last.is_some()) {
        let pair = (if turning { None } else { shown }, url.map(str::to_owned));
        ui.data_mut(|d| d.insert_temp(id, pair.clone()));
        pair
    } else {
        (last, shown)
    };
    let t = theme::appear(ui.ctx(), id.with("t"), egui::Id::new(&shown).value(), 0.38);
    (last, shown, t)
}

/// Eases in and out, for the deck's moves.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// The stream playing ("OPUS 257 KB/S"), small and quiet. On a quick
/// start it tells the story of the switch: a shimmer while the better
/// stream is fetched and while it waits its turn in the queue, then the
/// new label fading in with one pulse of the accent and a sparkle that
/// stays.
fn quality_chip(ui: &mut Ui, quality: Option<&str>, upgrade: Option<&Upgrade>) {
    let id = ui.id().with("quality-chip");
    let now = ui.input(|i| i.time);
    let font = theme::semibold(11.0);
    let (label, note, working, done_for) = match upgrade.map(|u| (u.phase, u)) {
        Some((UpgradePhase::Finding, u)) => (
            u.from.as_deref().or(quality).map_or_else(|| "STARTING".to_owned(), str::to_uppercase),
            Some("GETTING 256 KB/S".to_owned()),
            true,
            None,
        ),
        Some((UpgradePhase::Switching(_), u)) => (
            u.from.as_deref().or(quality).unwrap_or_default().to_uppercase(),
            Some(format!("\u{2192} {}", u.to.as_deref().unwrap_or("256 kb/s").to_uppercase())),
            true,
            None,
        ),
        Some((UpgradePhase::Done(at), u)) => (
            u.to.as_deref().or(quality).unwrap_or_default().to_uppercase(),
            None,
            false,
            Some(at.elapsed().as_secs_f32()),
        ),
        _ => (quality.unwrap_or_default().to_uppercase(), None, false, None),
    };
    let premium = upgrade.is_some_and(|u| matches!(u.phase, UpgradePhase::Done(_)) && u.to.is_some());
    let painter = ui.painter().clone();
    let mark = codec_mark(&label);
    let galley = painter.layout_no_wrap(label, font.clone(), theme::text_dim());
    let note_galley = note.map(|n| painter.layout_no_wrap(n, font.clone(), theme::text()));
    let sparkle = if premium { 15.0 } else { 0.0 };
    let mark_h = 11.0;
    let mark_w = mark.map_or(0.0, |(_, _, aspect)| mark_h * aspect + 5.0);
    let note_w = note_galley.as_ref().map_or(0.0, |g| g.size().x + 10.0);
    let target_w = galley.size().x + 14.0 + sparkle + mark_w + note_w;
    // The chip grows and shrinks with what it says.
    let width = ui.ctx().animate_value_with_time(id.with("w"), target_w, 0.28);
    let (rect, response) = ui.allocate_exact_size(vec2(width, galley.size().y + 8.0), Sense::hover());
    let clip = painter.with_clip_rect(rect.intersect(ui.clip_rect()));
    let radius = CornerRadius::same(4);
    clip.rect_filled(rect, radius, Color32::from_white_alpha(18));
    if working {
        // A soft band of light sweeping across while the better stream comes.
        let band = 46.0;
        // Thirty steps a second, as it is drawn.
        let now = (now * 30.0).floor() / 30.0;
        let x = rect.left() - band + ((now * 0.55).fract() as f32) * (rect.width() + band * 2.0);
        let mut mesh = Mesh::default();
        let clear = Color32::TRANSPARENT;
        let light = Color32::from_white_alpha(26);
        for (dx, color) in [(0.0, clear), (band, light), (band * 2.0, clear)] {
            mesh.colored_vertex(pos2(x + dx, rect.top()), color);
            mesh.colored_vertex(pos2(x + dx, rect.bottom()), color);
        }
        for i in [0u32, 2] {
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i + 1, i + 2, i + 3);
        }
        clip.add(Shape::mesh(mesh));
        theme::repaint_in(ui.ctx(), std::time::Duration::from_millis(33));
    }
    let mut x = rect.left() + 7.0;
    if premium {
        // The sparkle pops in with a little overshoot, then stays.
        let t = done_for.map_or(1.0, |s| (s / 0.45).min(1.0));
        let scale = ease_out_back(t);
        let size = 11.0 * scale;
        Icon::Mix
            .image(theme::accent_text(), size.max(1.0))
            .paint_at(ui, Rect::from_center_size(pos2(x + 5.5, rect.center().y), vec2(size, size)));
        x += sparkle;
    }
    // The new label fades in over the old.
    let fade_in = done_for.map_or(1.0, |s| (s / 0.4).min(1.0));
    // The codec's own symbol before its name.
    if let Some((uri, bytes, aspect)) = mark {
        let at = Rect::from_min_size(pos2(x, rect.center().y - mark_h / 2.0), vec2(mark_h * aspect, mark_h));
        if clip.clip_rect().intersects(at) {
            egui::Image::from_bytes(uri, bytes)
                .tint(Color32::WHITE.gamma_multiply(fade_in))
                .paint_at(ui, at);
        }
        x += mark_w;
    }
    let label_color = theme::text_dim().gamma_multiply(fade_in);
    clip.galley_with_override_text_color(pos2(x, rect.center().y - galley.size().y / 2.0), galley.clone(), label_color);
    if let Some(note) = note_galley {
        let pulse = 0.55 + 0.45 * ((now * 2.6).sin() as f32 * 0.5 + 0.5);
        let at = pos2(x + galley.size().x + 10.0, rect.center().y - note.size().y / 2.0);
        clip.galley_with_override_text_color(at, note, theme::text().gamma_multiply(pulse));
    }
    if let Some(since) = done_for
        && since < 1.6
    {
        // One pulse of the accent around the chip as the switch is heard.
        let t = since / 1.6;
        let alpha = (1.0 - t).powi(2);
        let grow = 1.0 + 5.0 * t;
        painter.rect_stroke(
            rect.expand(grow),
            CornerRadius::same(6),
            egui::Stroke::new(1.5, theme::accent().gamma_multiply(alpha)),
            egui::StrokeKind::Outside,
        );
        theme::next_frame(ui.ctx());
    }
    let tip = match upgrade.map(|u| u.phase) {
        Some(UpgradePhase::Finding) => "Playing at once in standard quality while Premium quality is fetched",
        Some(UpgradePhase::Switching(_)) => "Premium quality is ready and takes over in a moment, without a break",
        Some(UpgradePhase::Done(_)) if premium => "Switched to Premium quality without a break",
        _ => "The stream playing: its codec and bitrate",
    };
    response.on_hover_cursor(CursorIcon::Default).on_hover_text(tip);
}

/// The codec's symbol for a label such as "OPUS 257 KB/S": its image and
/// its width for its height.
fn codec_mark(label: &str) -> Option<(&'static str, &'static [u8], f32)> {
    const MARKS: [(&str, &str, &[u8], f32); 8] = [
        ("OPUS", "bytes://codec-opus.svg", include_bytes!("../../assets/codecs/opus.svg"), 193.2 / 129.2),
        ("AAC", "bytes://codec-aac.svg", include_bytes!("../../assets/codecs/aac.svg"), 908.0 / 783.0),
        ("FLAC", "bytes://codec-flac.svg", include_bytes!("../../assets/codecs/flac.svg"), 24.0 / 20.0),
        ("MP3", "bytes://codec-mp3.svg", include_bytes!("../../assets/codecs/mp3.svg"), 1.0),
        ("WAV", "bytes://codec-wav.svg", include_bytes!("../../assets/codecs/wav.svg"), 31.0 / 20.0),
        ("AIFF", "bytes://codec-aiff.svg", include_bytes!("../../assets/codecs/aiff.svg"), 31.0 / 20.0),
        ("ALAC", "bytes://codec-alac.svg", include_bytes!("../../assets/codecs/alac.svg"), 1.0),
        ("VORBIS", "bytes://codec-vorbis.svg", include_bytes!("../../assets/codecs/vorbis.svg"), 1.0),
    ];
    MARKS
        .iter()
        .find(|(name, ..)| label.split_whitespace().next() == Some(name))
        .map(|&(_, uri, bytes, aspect)| (uri, bytes, aspect))
}

/// Eases to 1 with a small overshoot, for things that pop in.
fn ease_out_back(t: f32) -> f32 {
    let (c1, c3) = (1.70158, 2.70158);
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// The artwork's colours, blurred across the window and darkened so text
/// reads on them, fading from the last song's when the song changes (as
/// far as `turning` when the deck is turning to it, in time with the
/// crossfade). One textured pass per image: the darkening, and the
/// deepening toward the player bar, are the mesh's vertex colours rather
/// than layers on top.
fn backdrop(app: &mut App, ui: &Ui, rect: Rect, url: Option<&str>, turning: Option<f32>) {
    let painter = ui.painter_at(rect);
    let id = ui.id().with("np-backdrop");
    let (mut current, mut previous): (Option<String>, Option<String>) = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if current.as_deref() != url {
        previous = current.take();
        current = url.map(str::to_owned);
        ui.data_mut(|d| d.insert_temp(id, (current.clone(), previous.clone())));
    }
    // How much of the image's brightness shows, at the top and the bottom.
    const TOP: f32 = 0.51;
    const BOTTOM: f32 = 0.22;
    let paint = |texture: &egui::TextureHandle, alpha: f32| {
        // Cover the window, keeping the square's proportions.
        let aspect = rect.width() / rect.height().max(1.0);
        let uv = if aspect > 1.0 {
            let h = 1.0 / aspect;
            Rect::from_min_max(pos2(0.0, 0.5 - h / 2.0), pos2(1.0, 0.5 + h / 2.0))
        } else {
            let w = aspect;
            Rect::from_min_max(pos2(0.5 - w / 2.0, 0.0), pos2(0.5 + w / 2.0, 1.0))
        };
        let shade = |level: f32| {
            let v = (level * alpha * 255.0).round() as u8;
            Color32::from_rgba_premultiplied(v, v, v, (alpha * 255.0).round() as u8)
        };
        let mut mesh = Mesh::with_texture(texture.id());
        mesh.vertices.extend([
            egui::epaint::Vertex { pos: rect.left_top(), uv: uv.left_top(), color: shade(TOP) },
            egui::epaint::Vertex { pos: rect.right_top(), uv: uv.right_top(), color: shade(TOP) },
            egui::epaint::Vertex { pos: rect.right_bottom(), uv: uv.right_bottom(), color: shade(BOTTOM) },
            egui::epaint::Vertex { pos: rect.left_bottom(), uv: uv.left_bottom(), color: shade(BOTTOM) },
        ]);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(mesh));
    };
    let images = &mut app.images;
    let fresh = current.as_deref().and_then(|u| images.backdrop(u));
    let old = previous.as_deref().and_then(|u| images.backdrop(u));
    match (&fresh, &old) {
        (Some(texture), old) => {
            let t = theme::appear(ui.ctx(), id.with("t"), egui::Id::new(current.as_deref()).value(), 0.8);
            let t = turning.map_or(t, |turn| turn.min(t));
            if t < 1.0 {
                match old {
                    Some(old) => paint(old, 1.0),
                    None => {
                        painter.rect_filled(rect, CornerRadius::ZERO, theme::bg());
                    }
                }
            }
            paint(texture, t);
        }
        (None, Some(old)) => paint(old, 1.0),
        (None, None) => {
            painter.rect_filled(rect, CornerRadius::ZERO, theme::bg());
        }
    }
}
