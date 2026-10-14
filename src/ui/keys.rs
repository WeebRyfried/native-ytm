//! Keyboard shortcuts, and the mouse's back and forward buttons.

use egui::{Context, Key, Modifiers, PointerButton};

use crate::api::model::Like;
use crate::api::music::Rating;
use crate::app::App;
use crate::model::{Action, SideTab};

pub fn handle(app: &mut App, ctx: &Context) {
    let typing = ctx.text_edit_focused();
    let mut actions = Vec::new();
    ctx.input_mut(|input| {
        if input.pointer.button_pressed(PointerButton::Extra1) {
            actions.push(Action::Back);
        }
        if input.pointer.button_pressed(PointerButton::Extra2) {
            actions.push(Action::Forward);
        }
        let ctrl = Modifiers::COMMAND;
        if input.consume_key(Modifiers::ALT, Key::ArrowLeft) {
            actions.push(Action::Back);
        }
        if input.consume_key(Modifiers::ALT, Key::ArrowRight) {
            actions.push(Action::Forward);
        }
        if input.consume_key(ctrl, Key::F) {
            app.search.focus = true;
        }
        if input.consume_key(ctrl, Key::ArrowRight) {
            actions.push(Action::Next);
        }
        if input.consume_key(ctrl, Key::ArrowLeft) {
            actions.push(Action::Previous);
        }
        if input.consume_key(ctrl, Key::ArrowUp) {
            actions.push(Action::SetVolume((app.settings.volume + 0.05).min(1.0)));
        }
        if input.consume_key(ctrl, Key::ArrowDown) {
            actions.push(Action::SetVolume((app.settings.volume - 0.05).max(0.0)));
        }
        if input.consume_key(ctrl, Key::R) || input.consume_key(Modifiers::NONE, Key::F5) {
            actions.push(Action::Refresh);
        }
        if input.consume_key(Modifiers::NONE, Key::F11) {
            actions.push(Action::ToggleFullscreen);
        }
        if typing {
            return;
        }
        if input.consume_key(Modifiers::NONE, Key::Space) {
            actions.push(Action::TogglePlay);
        }
        if input.consume_key(Modifiers::SHIFT, Key::ArrowRight) {
            actions.push(Action::SeekBy(10.0));
        }
        if input.consume_key(Modifiers::SHIFT, Key::ArrowLeft) {
            actions.push(Action::SeekBy(-10.0));
        }
        if input.consume_key(Modifiers::NONE, Key::Slash) {
            app.search.focus = true;
        }
        if input.consume_key(Modifiers::NONE, Key::M) {
            actions.push(Action::ToggleMute);
        }
        if input.consume_key(Modifiers::NONE, Key::F) {
            actions.push(Action::ToggleFullView);
        }
        if app.fullscreen && input.consume_key(Modifiers::NONE, Key::Escape) {
            actions.push(Action::ToggleFullscreen);
        } else if app.full_view && input.consume_key(Modifiers::NONE, Key::Escape) {
            actions.push(Action::ToggleFullView);
        }
        if input.consume_key(Modifiers::NONE, Key::Q) {
            actions.push(if app.full_view {
                Action::ShowSidePanel(SideTab::UpNext)
            } else if app.settings.side_panel && app.side_tab == SideTab::UpNext {
                Action::ToggleSidePanel
            } else {
                Action::ShowSidePanel(SideTab::UpNext)
            });
        }
        if input.consume_key(Modifiers::NONE, Key::L)
            && let Some(track) = app.queue.current_track()
        {
            let liked = app.now.like == Some(Like::Liked);
            actions.push(Action::Rate(
                track.video_id.clone(),
                if liked { Rating::None } else { Rating::Like },
            ));
        }
    });
    app.actions.extend(actions);
}
