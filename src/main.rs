//! The window: one copy per user, kept alive in the tray while the music
//! plays, and drawn by egui.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::sync::atomic::Ordering;

use native_ytm::app::{App, SHOW_REQUESTED};
use native_ytm::{platform, settings::Paths, theme, ui};

fn main() {
    // Started by an update: the old program is still closing, and holds
    // the one-copy slot until it has.
    let args: Vec<String> = std::env::args().collect();
    let updated_from = args
        .iter()
        .position(|a| a == "--updated-from")
        .and_then(|i| args.get(i + 1))
        .and_then(|pid| pid.parse::<u32>().ok());
    if let Some(pid) = updated_from {
        platform::wait_for_exit(pid, std::time::Duration::from_secs(15));
    }
    // Started with Windows: into the tray, no window.
    let background = args.iter().any(|a| a == "--background");
    let resume = if args.iter().any(|a| a == "--resume-playing") {
        Some(true)
    } else if args.iter().any(|a| a == "--resume") {
        Some(false)
    } else {
        None
    };
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
    // log file. The last few runs' logs are kept beside it.
    rotate_logs(&paths.logs(), "native-ytm", 3);
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
    let left_behind = std::env::current_exe().is_ok_and(|exe| exe.with_extension("exe.old").exists());
    if updated_from.is_some() || left_behind {
        native_ytm::update::clean_up_after(None);
    }
    let mut app = App::new(waker.clone());
    if let Some(playing) = resume {
        app.resume_after_update(playing);
    }
    let config = paths.config.clone();
    let result = fastframe_shell::Shell::new(app, &waker).start_hidden(background).run(|lease| {
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
                        platform::style_title_bar(window.hwnd.get(), theme::sidebar());
                        platform::set_main_window(window.hwnd.get());
                    }
                }
                dev_route(&mut app);
                let seek = std::env::var("NATIVE_YTM_SEEK").ok().and_then(|v| v.parse().ok());
                Ok(Box::new(Window {
                    app,
                    shot: Shot::from_env(),
                    seek,
                    simulate: Simulate::from_env(),
                    size: None,
                    resized_at: None,
                    released_once: false,
                    close_when_windowed: false,
                }))
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
    /// The window's size last frame, and when it last changed: a second
    /// after a resize, the memory the graphics driver kept for the sizes
    /// passed through goes back to Windows.
    size: Option<egui::Vec2>,
    resized_at: Option<std::time::Instant>,
    released_once: bool,
    /// Closing waits for full screen to end, so the window's size and
    /// place are saved as a window's.
    close_when_windowed: bool,
}

/// A screenshot taken on its own and saved, for checking the interface
/// without a person at the screen: `NATIVE_YTM_SHOT=out.png`, optionally
/// with `NATIVE_YTM_ROUTE` (search:QUERY, browse:ID, play:VIDEO_ID,
/// explore, library, settings, queue, lyrics) and `NATIVE_YTM_SHOT_DELAY`
/// in milliseconds. With `NATIVE_YTM_SHOT_EVERY` (milliseconds) and
/// `NATIVE_YTM_SHOT_COUNT`, a run of shots that far apart, numbered
/// (out-000.png, out-001.png...), for watching an animation.
struct Shot {
    path: std::path::PathBuf,
    due: std::time::Instant,
    asked: bool,
    every: Option<std::time::Duration>,
    count: u32,
    taken: u32,
}

impl Shot {
    fn from_env() -> Option<Self> {
        let path = std::env::var_os("NATIVE_YTM_SHOT")?;
        let delay = std::env::var("NATIVE_YTM_SHOT_DELAY")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(7000);
        let every = std::env::var("NATIVE_YTM_SHOT_EVERY").ok().and_then(|d| d.parse().ok()).map(std::time::Duration::from_millis);
        let count = std::env::var("NATIVE_YTM_SHOT_COUNT").ok().and_then(|d| d.parse().ok()).unwrap_or(1);
        Some(Self {
            path: path.into(),
            due: std::time::Instant::now() + std::time::Duration::from_millis(delay),
            asked: false,
            every,
            count,
            taken: 0,
        })
    }

    /// Where the next shot goes: numbered in a run.
    fn next_path(&self) -> std::path::PathBuf {
        if self.every.is_none() {
            return self.path.clone();
        }
        let stem = self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = self.path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "png".into());
        self.path.with_file_name(format!("{stem}-{:03}.{ext}", self.taken))
    }
}

/// Moves the last run's log aside before this run's starts: `name.log`
/// becomes `name.1.log`, that one `name.2.log`, and so on, `keep` of them.
fn rotate_logs(dir: &std::path::Path, name: &str, keep: usize) {
    let path = |n: usize| if n == 0 { dir.join(format!("{name}.log")) } else { dir.join(format!("{name}.{n}.log")) };
    let _ = std::fs::remove_file(path(keep));
    for n in (0..keep).rev() {
        let _ = std::fs::rename(path(n), path(n + 1));
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Pointer input made up for tests (dev): `NATIVE_YTM_WIGGLE=x,y` moves the
/// pointer in small circles about that point every frame, as a hand on a
/// mouse does; `NATIVE_YTM_WHEEL=x,y` rests it there and turns the wheel
/// down three lines a second; `NATIVE_YTM_CLICK=x,y[,seconds]` clicks there
/// once, ten seconds in unless told.
enum Simulate {
    Wiggle(egui::Pos2),
    Wheel(egui::Pos2, f64),
    /// Where, when, and how far along: 0 waiting, 1 pressed, 2 done.
    Click(egui::Pos2, f64, u8),
}

impl Simulate {
    fn from_env() -> Option<Self> {
        let point = |name: &str| {
            let value = std::env::var(name).ok()?;
            let (x, y) = value.split_once(',')?;
            Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
        };
        let click = || {
            let value = std::env::var("NATIVE_YTM_CLICK").ok()?;
            let mut parts = value.split(',').map(|p| p.trim().parse::<f64>().ok());
            let (x, y) = (parts.next()??, parts.next()??);
            let at = parts.next().flatten().unwrap_or(10.0);
            Some(Simulate::Click(egui::pos2(x as f32, y as f32), at, 0))
        };
        point("NATIVE_YTM_WIGGLE")
            .map(Simulate::Wiggle)
            .or_else(|| point("NATIVE_YTM_WHEEL").map(|p| Simulate::Wheel(p, 0.0)))
            .or_else(click)
    }
}

/// `download` in NATIVE_YTM_ROUTE: download the song playing, once one is.
static DOWNLOAD_PLAYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `update` in NATIVE_YTM_ROUTE: install an update as soon as one is
/// found (with NATIVE_YTM_VERSION, an older version to be).
static UPDATE_WHEN_FOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `local:N` in NATIVE_YTM_ROUTE: play the songs on this PC from the Nth,
/// once the scan has found them (N + 1, so 0 is none).
static PLAY_LOCAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `localradio:N`: start a radio from the Nth song on this PC, the same way.
static RADIO_LOCAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `localalbum:N`: open the page of the Nth album on this PC, the same way.
static ALBUM_LOCAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `deviceview:N`: On this PC as songs (0), albums (1) or artists (2).
static DEVICE_VIEW: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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
            "cached" => Action::Navigate(Route::Library(LibraryTab::Cached)),
            "download" => {
                DOWNLOAD_PLAYING.store(true, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            "update" => {
                UPDATE_WHEN_FOUND.store(true, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            "local" => {
                PLAY_LOCAL.store(arg.parse::<usize>().unwrap_or(0) + 1, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            "localradio" => {
                RADIO_LOCAL.store(arg.parse::<usize>().unwrap_or(0) + 1, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            "localalbum" => {
                ALBUM_LOCAL.store(arg.parse::<usize>().unwrap_or(0) + 1, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            "deviceview" => {
                DEVICE_VIEW.store(arg.parse::<usize>().unwrap_or(0) + 1, std::sync::atomic::Ordering::Relaxed);
                Action::Navigate(Route::Library(LibraryTab::Device))
            }
            "stats" => {
                app.actions.push(Action::Navigate(Route::Stats));
                // `stats:app` for the app's own.
                if arg == "app" {
                    app.actions.push(Action::ShowStats { source: native_ytm::stats::Source::App, period: native_ytm::stats::Period::Month });
                }
                continue;
            }
            "recent" => {
                app.search.input.clear();
                app.search.focus = true;
                continue;
            }
            "settings" => Action::Navigate(Route::Settings),
            "device" => Action::Navigate(Route::Library(LibraryTab::Device)),
            "artist" => Action::Navigate(Route::Artist(arg.to_owned())),
            "fullscreen" => Action::ToggleFullscreen,
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
            Some(Simulate::Click(at, when, stage)) => {
                if *stage < 2 && time > *when - 1.0 {
                    raw_input.events.push(egui::Event::PointerMoved(*at));
                }
                if *stage < 2 && time >= *when {
                    raw_input.events.push(egui::Event::PointerButton {
                        pos: *at,
                        button: egui::PointerButton::Primary,
                        pressed: *stage == 0,
                        modifiers: egui::Modifiers::NONE,
                    });
                    if *stage == 1 {
                        log::info!("clicked at {:?}", *at);
                    }
                    *stage += 1;
                }
                if *stage < 2 {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
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
            if self.app.quit && self.app.fullscreen {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.app.actions.push(native_ytm::model::Action::ToggleFullscreen);
                self.close_when_windowed = true;
            }
        }
        if self.close_when_windowed {
            if !self.app.fullscreen && ctx.input(|i| i.viewport().fullscreen) != Some(true) {
                self.close_when_windowed = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                theme::repaint_in(ctx, std::time::Duration::from_millis(30));
            }
        }
        if UPDATE_WHEN_FOUND.load(Ordering::Relaxed)
            && matches!(self.app.updater.state(), native_ytm::update::State::Available(_))
        {
            UPDATE_WHEN_FOUND.store(false, Ordering::Relaxed);
            self.app.actions.push(native_ytm::model::Action::InstallUpdate);
        }
        fastframe_shell::window::recover_offscreen(ctx, frame);
        // A folder dropped on the window joins the music on this PC.
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for folder in dropped.into_iter().filter(|p| p.is_dir()) {
            self.app.actions.push(native_ytm::model::Action::AddLocalFolderPath(folder.display().to_string()));
        }
        // A window saved in full screen opens as a window; full screen is
        // the player's, entered on purpose.
        if !self.app.fullscreen && ctx.input(|i| i.viewport().fullscreen) == Some(true) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        let size = ctx.content_rect().size();
        if self.size.is_some_and(|last| last != size) {
            self.resized_at = Some(std::time::Instant::now());
            self.released_once = false;
        }
        self.size = Some(size);
        if let Some(at) = self.resized_at {
            // Once a second after, and again a little later: the driver
            // goes over its old buffers once more as it lets them go.
            let (first, again) = (std::time::Duration::from_secs(1), std::time::Duration::from_secs(6));
            let since = at.elapsed();
            if since >= again {
                self.resized_at = None;
                platform::release_unused_memory();
            } else if since >= first && !self.released_once {
                self.released_once = true;
                platform::release_unused_memory();
                theme::repaint_in(ctx, again - since);
            } else {
                theme::repaint_in(ctx, if since < first { first - since } else { again - since });
            }
        }
        let local = PLAY_LOCAL.load(Ordering::Relaxed);
        if local > 0 && !self.app.local.songs().is_empty() {
            PLAY_LOCAL.store(0, Ordering::Relaxed);
            let tracks = self.app.local.songs().to_vec();
            let start = (local - 1).min(tracks.len() - 1);
            self.app.actions.push(native_ytm::model::Action::PlayTracks { tracks, start, source: Default::default() });
        }
        let album = ALBUM_LOCAL.load(Ordering::Relaxed);
        if album > 0 && !self.app.local.songs().is_empty() {
            ALBUM_LOCAL.store(0, Ordering::Relaxed);
            let albums = self.app.local.album_list();
            if let Some(found) = albums.get((album - 1).min(albums.len().saturating_sub(1))) {
                self.app.actions.push(native_ytm::model::Action::Navigate(native_ytm::model::Route::LocalAlbum(found.key.clone())));
            }
        }
        let view = DEVICE_VIEW.swap(0, Ordering::Relaxed);
        if view > 0 {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("device-view"), (view - 1) as u8));
        }
        let radio = RADIO_LOCAL.load(Ordering::Relaxed);
        if radio > 0 && !self.app.local.songs().is_empty() {
            RADIO_LOCAL.store(0, Ordering::Relaxed);
            let songs = self.app.local.songs();
            let track = songs[(radio - 1).min(songs.len() - 1)].clone();
            self.app.actions.push(native_ytm::model::Action::StartRadio(track));
        }
        if DOWNLOAD_PLAYING.load(Ordering::Relaxed)
            && self.app.playing()
            && let Some(track) = self.app.current().cloned()
        {
            DOWNLOAD_PLAYING.store(false, Ordering::Relaxed);
            self.app.actions.push(native_ytm::model::Action::SaveOffline(vec![track]));
        }
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
                    let _ = buffer.save(shot.next_path());
                }
                shot.taken += 1;
                match shot.every {
                    Some(every) if shot.taken < shot.count => {
                        shot.asked = false;
                        shot.due += every;
                    }
                    _ => {
                        self.app.quit = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            ctx.request_repaint_after(shot.every.unwrap_or(std::time::Duration::from_millis(100)).min(std::time::Duration::from_millis(100)));
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
        theme::bg().to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.save_session();
    }
}
