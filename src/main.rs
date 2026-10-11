//! The window: one copy per user, kept alive in the tray while the music
//! plays, and drawn by egui.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::sync::atomic::Ordering;

use native_ytm::app::{App, SHOW_REQUESTED};
use native_ytm::{platform, settings::Paths, theme, ui};

fn main() {
    let paths = Paths::new();
    let waker = fastframe_shell::Waker::default();
    // A test profile is an instance of its own, beside the real one.
    let slot_name = match native_ytm::settings::profile() {
        Some(dir) => format!("dev.native-ytm.NativeYtm.{:x}", fnv(&dir.to_string_lossy())),
        None => "dev.native-ytm.NativeYtm".to_owned(),
    };
    let slot = fastframe_instance::Slot::new(&slot_name);
    let wake = waker.clone();
    let _guard = match slot.claim("show", move |request| match request {
        "show" => {
            SHOW_REQUESTED.store(true, Ordering::SeqCst);
            wake.wake();
            Some("ok".to_owned())
        }
        _ => None,
    }) {
        fastframe_instance::Claim::First(guard) => guard,
        // The running copy shows its window.
        _ => return,
    };
    // Only the running copy logs, so a second launch never replaces its
    // log file.
    if let Err(error) = fastframe_log::Logging::new("native-ytm", env!("CARGO_PKG_VERSION"))
        .filter("warn,native_ytm=info,fastframe_audio=info")
        .file(paths.logs().join("native-ytm.log"))
        .panic_log(paths.logs().join("panic.log"))
        .init()
    {
        eprintln!("no logger: {error}");
    }

    // Before the media controls and the window: Windows names the app in
    // its media overlay from this id.
    if native_ytm::settings::profile().is_none() {
        platform::register_app_identity(&paths.config);
    }
    let app = App::new(waker.clone());
    let config = paths.config.clone();
    let result = fastframe_shell::Shell::new(app, &waker).run(|lease| {
        let viewport = egui::ViewportBuilder::default()
            .with_title("Native YTM")
            .with_app_id("native-ytm")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([880.0, 560.0])
            .with_icon(Arc::new(theme::app_icon(256)));
        let options = eframe::NativeOptions {
            viewport,
            persist_window: true,
            persistence_path: Some(config.join("window.ron")),
            ..eframe::NativeOptions::default()
        };
        eframe::run_native(
            "Native YTM",
            options,
            Box::new(move |cc| {
                let mut app = lease.take(&cc.egui_ctx);
                app.attach(&cc.egui_ctx);
                #[cfg(windows)]
                {
                    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    if let Ok(handle) = cc.window_handle()
                        && let RawWindowHandle::Win32(window) = handle.as_raw()
                    {
                        platform::style_title_bar(window.hwnd.get(), theme::SIDEBAR);
                    }
                }
                dev_route(&mut app);
                let seek = std::env::var("NATIVE_YTM_SEEK").ok().and_then(|v| v.parse().ok());
                Ok(Box::new(Window { app, shot: Shot::from_env(), seek, simulate: Simulate::from_env() }))
            }),
        )
    });
    if let Err(error) = result {
        log::error!("the window failed: {error}");
    }
}

struct Window {
    app: fastframe_shell::Held<App>,
    shot: Option<Shot>,
    /// `NATIVE_YTM_SEEK`: a position to jump to once the song plays.
    seek: Option<f64>,
    simulate: Option<Simulate>,
}

/// A screenshot taken on its own and saved, for checking the interface
/// without a person at the screen: `NATIVE_YTM_SHOT=out.png`, optionally
/// with `NATIVE_YTM_ROUTE` (search:QUERY, browse:ID, play:VIDEO_ID,
/// explore, library, settings, queue, lyrics) and `NATIVE_YTM_SHOT_DELAY`
/// in milliseconds.
struct Shot {
    path: std::path::PathBuf,
    due: std::time::Instant,
    asked: bool,
}

impl Shot {
    fn from_env() -> Option<Self> {
        let path = std::env::var_os("NATIVE_YTM_SHOT")?;
        let delay = std::env::var("NATIVE_YTM_SHOT_DELAY")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(7000);
        Some(Self {
            path: path.into(),
            due: std::time::Instant::now() + std::time::Duration::from_millis(delay),
            asked: false,
        })
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Pointer input made up for tests (dev): `NATIVE_YTM_WIGGLE=x,y` moves the
/// pointer in small circles about that point every frame, as a hand on a
/// mouse does; `NATIVE_YTM_WHEEL=x,y` rests it there and turns the wheel
/// down three lines a second.
enum Simulate {
    Wiggle(egui::Pos2),
    Wheel(egui::Pos2, f64),
}

impl Simulate {
    fn from_env() -> Option<Self> {
        let point = |name: &str| {
            let value = std::env::var(name).ok()?;
            let (x, y) = value.split_once(',')?;
            Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
        };
        point("NATIVE_YTM_WIGGLE")
            .map(Simulate::Wiggle)
            .or_else(|| point("NATIVE_YTM_WHEEL").map(|p| Simulate::Wheel(p, 0.0)))
    }
}

fn dev_route(app: &mut App) {
    use native_ytm::model::{Action, LibraryTab, Route, SideTab};
    let Ok(route) = std::env::var("NATIVE_YTM_ROUTE") else {
        return;
    };
    if std::env::var_os("NATIVE_YTM_MUTE").is_some() {
        app.player.set_volume(0.0);
    }
    app.scroll_to = std::env::var("NATIVE_YTM_SCROLL").ok().and_then(|v| v.parse().ok());
    for step in route.split('|') {
        let (kind, arg) = step.split_once(':').unwrap_or((step, ""));
        let action = match kind {
            "search" => Action::Search(arg.to_owned()),
            "browse" => Action::Navigate(Route::Browse { browse_id: arg.to_owned(), params: None }),
            "explore" => Action::Navigate(Route::Explore),
            "library" => Action::Navigate(Route::Library(LibraryTab::Playlists)),
            "settings" => Action::Navigate(Route::Settings),
            "queue" => Action::ShowSidePanel(SideTab::UpNext),
            "lyrics" => Action::ShowSidePanel(SideTab::Lyrics),
            "signin" => Action::OpenSignIn,
            "full" => Action::ToggleFullView,
            "play" => Action::PlayWatch { video_id: Some(arg.to_owned()), playlist_id: None, params: None, title: None },
            _ => continue,
        };
        app.actions.push(action);
    }
}

impl eframe::App for Window {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let time = raw_input.time.unwrap_or(0.0);
        match &mut self.simulate {
            Some(Simulate::Wiggle(at)) => {
                let t = time as f32;
                let pos = *at + egui::vec2((t * 7.0).sin() * 30.0, (t * 5.0).cos() * 20.0);
                raw_input.events.push(egui::Event::PointerMoved(pos));
                ctx.request_repaint();
            }
            Some(Simulate::Wheel(at, last)) => {
                raw_input.events.push(egui::Event::PointerMoved(*at));
                if time - *last > 1.0 && time > 8.0 {
                    *last = time;
                    raw_input.events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(0.0, -3.0),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            None => {}
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.background();
        if std::mem::take(&mut self.app.wants_show) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.app.on_close_requested();
        }
        fastframe_shell::window::recover_offscreen(ctx, frame);
        if self.seek.is_some() && self.app.playing() && self.app.position() > 0.5 {
            let to = self.seek.take().unwrap_or_default();
            self.app.actions.push(native_ytm::model::Action::Seek(to));
        }
        if let Some(shot) = &mut self.shot {
            if !shot.asked && std::time::Instant::now() >= shot.due {
                shot.asked = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            let image = ctx.input(|i| {
                i.raw.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = image {
                let size = image.size;
                let pixels: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                if let Some(buffer) = image::RgbaImage::from_raw(size[0] as u32, size[1] as u32, pixels) {
                    let _ = buffer.save(&shot.path);
                }
                self.app.quit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui::show(&mut self.app, ui);
        let actions = std::mem::take(&mut self.app.actions);
        for action in actions {
            self.app.apply(action);
        }
        // egui's own links (none of ours go this way any more): eframe is
        // built without its browser support, so open them here.
        let urls: Vec<String> = ui.ctx().output_mut(|o| {
            let mut urls = Vec::new();
            o.commands.retain(|command| match command {
                egui::OutputCommand::OpenUrl(open) => {
                    urls.push(open.url.clone());
                    false
                }
                _ => true,
            });
            urls
        });
        for url in urls {
            platform::open_url(&url);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.save_session();
    }
}
