//! The Artwork look: the app in the colours of the song playing, turning
//! to the next song's over a moment, as the full-window player's backdrop
//! does.

use std::time::Instant;

use egui::Color32;

use super::App;
use crate::theme::Theme;

/// How long the look takes to turn to a new song's colours.
const TURN: f32 = 0.8;

#[derive(Default)]
pub struct ArtTheme {
    /// The colour shown, the one it is turning from and to, and since when.
    shown: Option<Color32>,
    from: Option<Color32>,
    to: Option<Color32>,
    since: Option<Instant>,
    /// The logo, window icon and tray drawn in the colour turned to.
    settled: bool,
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

impl App {
    pub(super) fn art_theme_tick(&mut self) {
        if self.settings.theme != Theme::Artwork {
            self.art_theme = ArtTheme::default();
            return;
        }
        let url = self.current().and_then(|t| t.thumbnail.clone());
        let wanted = url.and_then(|url| self.images.seed(&url));
        let state = &mut self.art_theme;
        // Nothing playing yet, or its artwork still coming: the look stays.
        let Some(wanted) = wanted.or(state.to).or(Some(Color32::from_rgb(0xff, 0x00, 0x33))) else {
            return;
        };
        if state.to != Some(wanted) {
            state.from = state.shown;
            state.to = Some(wanted);
            state.since = Some(Instant::now());
            state.settled = false;
        }
        let t = state.since.map_or(1.0, |at| (at.elapsed().as_secs_f32() / TURN).min(1.0));
        let colour = match state.from {
            Some(from) if t < 1.0 => mix(from, wanted, crate::theme::ease_in_out(t)),
            _ => wanted,
        };
        if state.shown != Some(colour) {
            state.shown = Some(colour);
            crate::theme::set_artwork(colour);
            crate::theme::apply(&self.ctx);
        }
        if t < 1.0 {
            crate::theme::next_frame(&self.ctx);
        } else if !state.settled {
            state.settled = true;
            crate::theme::settle_logo();
            crate::platform::restyle_main_window(crate::theme::sidebar());
            crate::platform::set_main_window_icon(crate::theme::app_icon_rgba);
            if let Some(tray) = &mut self.tray {
                tray.set_icon(crate::theme::app_icon_rgba, None);
            }
            self.ctx.request_repaint();
        }
    }
}
