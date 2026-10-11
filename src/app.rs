//! The application: its state, the frame loop, and what every action does.
//!
//! Views draw from `App` and push `Action`s; `apply` runs them after the
//! frame. Network answers and player events come in at the start of each
//! frame. The interface is optimistic: a clicked song is the playing song at
//! once, and the backend catches up behind it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::api::client::{Auth, parse_cookies};
use crate::api::model::*;
use crate::api::music::{Filter, Rating};
use crate::backend::{Backend, Purpose, Request, Response};
use crate::images::Images;
use crate::model::*;
use crate::player::{self, Player, State as PlayState, StreamInfo};
use crate::queue::{Queue, Source};
use crate::settings::{self, Paths, Repeat, Session, Settings};
use crate::{discord, lastfm};

/// Set by another launch of the app, which asks for the window.
pub static SHOW_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const TRAY_SHOW: &str = "show";
const TRAY_PLAY: &str = "play";
const TRAY_NEXT: &str = "next";
const TRAY_PREVIOUS: &str = "previous";
const TRAY_QUIT: &str = "quit";

/// Why a watch request was made, so its answer goes to the right place.
#[derive(Clone, Debug)]
enum WatchPurpose {
    /// Becomes the queue, starting at this video (or the first).
    Start {
        video_id: Option<String>,
        title: Option<String>,
    },
    /// The radio that follows a single song.
    Radio { after_uid: u64 },
    /// More of the queue's radio or mix.
    Extend,
    /// Lyrics and rating for the song now playing.
    Info { video_id: String },
}

#[derive(Default)]
pub struct NowInfo {
    pub video_id: String,
    pub lyrics_id: Option<String>,
    pub like: Option<Like>,
    /// The song's info arrived (or failed): whether YouTube has lyrics for
    /// it is known.
    pub info_ready: bool,
}

pub struct SearchState {
    pub input: String,
    pub query: String,
    pub filter: Filter,
    pub seq: u64,
    pub results: Loadable<Vec<Section>>,
    pub continuation: Option<String>,
    pub loading_more: bool,
    pub suggestions: Suggestions,
    pub suggest_seq: u64,
    pub suggest_for: String,
    pub focus: bool,
}

pub struct SignInDialog {
    pub text: String,
    pub error: Option<String>,
    pub busy: bool,
}

pub struct App {
    pub settings: Settings,
    pub paths: Paths,
    pub backend: Backend,
    pub player: Player,
    player_events: Receiver<player::Event>,
    pub images: Images,
    pub queue: Queue,
    pub actions: Vec<Action>,
    pub ctx: egui::Context,

    pub route: Route,
    history: Vec<Route>,
    forward: Vec<Route>,
    pub pages: HashMap<String, PageState>,
    pub search: SearchState,

    seq: u64,
    watch_purposes: HashMap<u64, WatchPurpose>,
    current_token: u64,
    current_uid: Option<u64>,
    preload: Option<(u64, u64)>,
    preload_wanted_for: Option<u64>,
    /// Play once the current song is resolved, or hold it paused.
    want_play: bool,
    /// Where to start the current song once it is resolved (a restored
    /// session, or a retry).
    start_at: f64,
    failures: u32,
    retried: Option<u64>,
    streams: HashMap<String, (StreamInfo, Instant)>,
    /// Songs being resolved ahead of time, so a slow resolve (yt-dlp) is
    /// done before they are wanted.
    prefetching: HashSet<String>,
    /// A queue answer is on its way; the bar shows it loading.
    pub pending_queue: Option<u64>,
    extending: bool,
    radio_dry: bool,
    /// The queue ran out and more is being fetched to carry on.
    ended_waiting: bool,
    pub now: NowInfo,
    pub lyrics: HashMap<String, Loadable<Option<Lyrics>>>,

    pub signed_in: bool,
    pub account: Option<Account>,
    pub playlists: Vec<Item>,
    pub sign_in: Option<SignInDialog>,
    pub picker: Option<Vec<Track>>,
    pub new_playlist: Option<(String, Vec<Track>)>,
    pub toasts: Vec<Toast>,
    pub side_tab: SideTab,
    /// The full-window player: the artwork, with the queue or the lyrics.
    pub full_view: bool,
    pub full_tab: SideTab,
    pub ready: bool,
    /// Settings as edited on the Settings page, until applied.
    pub settings_draft: Option<Settings>,

    now_playing: Option<fastframe_now_playing::NowPlaying>,
    presence: Option<discord::Presence>,
    pub lastfm: lastfm::LastFm,
    /// The Last.fm API account as typed in Settings, until it connects.
    pub lastfm_form: (String, String),
    /// Set while music plays, so a timer wakes the app now and then with
    /// the window closed (scrobbling, preloading).
    ticking: Arc<AtomicBool>,
    tray: Option<fastframe_tray::Tray>,
    tray_playing: Option<bool>,
    waker: fastframe_shell::Waker,
    pub quit: bool,
    pub hide: bool,
    pub wants_show: bool,
    last_saved: Instant,
    pub window_styled: bool,
    pub drag_from: Option<usize>,
    /// A scroll position to jump the page to once (screenshots).
    pub scroll_to: Option<f32>,
}

impl App {
    pub fn new(waker: fastframe_shell::Waker) -> Self {
        let paths = Paths::new();
        let settings: Settings = settings::load(&paths.settings());
        let wake: Arc<dyn Fn() + Send + Sync> = {
            let waker = waker.clone();
            Arc::new(move || waker.wake())
        };
        let ytdlp = (!settings.ytdlp_path.trim().is_empty())
            .then(|| std::path::PathBuf::from(settings.ytdlp_path.trim()));
        crate::stream::clean_scratch(&paths.scratch());
        let backend = Backend::new(
            &settings.language,
            crate::stream::Resolver::new(ytdlp, settings.use_ytdlp, settings.premium_quality, paths.scratch()),
            wake.clone(),
        );
        if !settings.region.trim().is_empty() {
            backend.send(Request::SetLanguage {
                language: settings.language.clone(),
                region: Some(settings.region.trim().to_owned()),
            });
        }
        let (events_tx, player_events) = std::sync::mpsc::channel();
        let mut player = Player::spawn(
            backend.runtime.handle().clone(),
            backend.http.clone(),
            events_tx,
            wake,
            settings.normalize,
        );
        player.set_volume(if settings.muted { 0.0 } else { settings.volume });
        let ctx = egui::Context::default();
        let images = Images::new(
            ctx.clone(),
            backend.runtime.handle().clone(),
            backend.http.clone(),
            paths.images(),
        );
        let mut queue = Queue::default();
        queue.shuffle = settings.shuffle;
        let lastfm = lastfm::LastFm::new(&paths.config, backend.runtime.handle().clone(), backend.http.clone());
        let ticking = Arc::new(AtomicBool::new(false));
        {
            let (ticking, waker) = (ticking.clone(), waker.clone());
            let _ = std::thread::Builder::new().name("ticker".into()).spawn(move || loop {
                std::thread::sleep(Duration::from_secs(5));
                if ticking.load(std::sync::atomic::Ordering::Relaxed) {
                    waker.wake();
                }
            });
        }
        let mut app = Self {
            settings,
            backend,
            player,
            player_events,
            images,
            queue,
            actions: Vec::new(),
            ctx,
            route: Route::Home,
            history: Vec::new(),
            forward: Vec::new(),
            pages: HashMap::new(),
            search: SearchState {
                input: String::new(),
                query: String::new(),
                filter: Filter::All,
                seq: 0,
                results: Loadable::Ready(Vec::new()),
                continuation: None,
                loading_more: false,
                suggestions: Suggestions::default(),
                suggest_seq: 0,
                suggest_for: String::new(),
                focus: false,
            },
            seq: 0,
            watch_purposes: HashMap::new(),
            current_token: 0,
            current_uid: None,
            preload: None,
            preload_wanted_for: None,
            want_play: true,
            start_at: 0.0,
            failures: 0,
            retried: None,
            streams: HashMap::new(),
            prefetching: HashSet::new(),
            pending_queue: None,
            extending: false,
            radio_dry: false,
            ended_waiting: false,
            now: NowInfo::default(),
            lyrics: HashMap::new(),
            signed_in: false,
            account: None,
            playlists: Vec::new(),
            sign_in: None,
            picker: None,
            new_playlist: None,
            toasts: Vec::new(),
            side_tab: SideTab::UpNext,
            full_view: false,
            full_tab: SideTab::UpNext,
            ready: false,
            settings_draft: None,
            now_playing: None,
            presence: None,
            lastfm,
            lastfm_form: (String::new(), String::new()),
            ticking,
            tray: None,
            tray_playing: None,
            waker,
            quit: false,
            hide: false,
            wants_show: false,
            last_saved: Instant::now(),
            window_styled: false,
            drag_from: None,
            scroll_to: None,
            paths,
        };
        if let Some(cookie) = crate::platform::load_cookie(&app.paths.account()) {
            let cookies = parse_cookies(&cookie);
            let auth = Auth {
                cookies,
                auth_user: 0,
            };
            if auth.is_usable() {
                app.backend.send(Request::SetAuth(Some(auth)));
                app.signed_in = true;
                app.backend.send(Request::Account);
                app.backend.send(Request::LibraryPlaylists);
            }
        }
        app.backend.send(Request::Bootstrap);
        app.request_page(&Route::Home, false);
        if app.settings.restore_session {
            let session: Session = settings::load(&app.paths.session());
            if !session.tracks.is_empty() {
                let current = session.current.unwrap_or(0);
                app.queue.replace(
                    session.tracks,
                    current,
                    Source {
                        playlist_id: session.playlist_id,
                        ..Source::default()
                    },
                );
                app.start_at = session.position_secs;
                app.current_uid = app.queue.current().map(|e| e.uid);
            }
        }
        app.now_playing = Some(fastframe_now_playing::NowPlaying::start(
            fastframe_now_playing::App::new("native-ytm", "Native YTM"),
            {
                let waker = app.waker.clone();
                move || waker.wake()
            },
        ));
        // NATIVE_YTM_NO_DISCORD keeps test runs off the person's profile.
        if app.settings.discord && std::env::var_os("NATIVE_YTM_NO_DISCORD").is_none() {
            app.presence = Some(discord::Presence::spawn(&app.discord_client_id()));
        }
        app.tray = fastframe_tray::Tray::spawn(tray_config(), {
            let waker = app.waker.clone();
            move || waker.wake()
        });
        app
    }

    /// Attaches the app to a new window's context.
    pub fn attach(&mut self, ctx: &egui::Context) {
        self.ctx = ctx.clone();
        self.images = Images::new(
            ctx.clone(),
            self.backend.runtime.handle().clone(),
            self.backend.http.clone(),
            self.paths.images(),
        );
        crate::theme::install(ctx);
        ctx.set_zoom_factor(self.settings.zoom.clamp(0.7, 2.0));
        self.window_styled = false;
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    pub fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        let text = text.into();
        self.toasts.retain(|t| t.text != text);
        self.toasts.push(Toast {
            text,
            kind,
            until: Instant::now() + Duration::from_secs(if kind == ToastKind::Error { 5 } else { 3 }),
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    // ---- The frame -------------------------------------------------------

    /// Everything that happens with or without a window: answers, player
    /// events, media keys, the tray, timers.
    pub fn background(&mut self) {
        if SHOW_REQUESTED.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.wants_show = true;
        }
        for response in self.backend.responses() {
            self.on_response(response);
        }
        while let Ok(event) = self.player_events.try_recv() {
            self.on_player(event);
        }
        self.images.poll();
        self.media_controls();
        self.tray_events();
        self.maybe_preload();
        self.maybe_extend();
        self.presence_tick();
        self.scrobble_tick();
        self.ticking
            .store(self.playing(), std::sync::atomic::Ordering::Relaxed);
        self.toasts.retain(|t| t.until > Instant::now());
        if self.last_saved.elapsed() > Duration::from_secs(15) {
            self.save_session();
        }
        let actions = std::mem::take(&mut self.actions);
        for action in actions {
            self.apply(action);
        }
    }

    pub fn playing(&self) -> bool {
        matches!(
            self.player.status().state,
            PlayState::Playing | PlayState::Buffering
        )
    }

    /// The song the bar shows: the queue's current entry.
    pub fn current(&self) -> Option<&Track> {
        self.queue.current_track()
    }

    pub fn position(&self) -> f64 {
        let status = self.player.status();
        if status.token == self.current_token && status.token != 0 {
            self.player.position().as_secs_f64()
        } else {
            self.start_at
        }
    }

    pub fn duration(&self) -> Option<f64> {
        let status = self.player.status();
        if status.token == self.current_token
            && let Some(d) = status.duration
        {
            return Some(d.as_secs_f64());
        }
        self.current().and_then(|t| t.duration).map(f64::from)
    }

    /// Loading or buffering: the bar shows a spinner.
    pub fn busy(&self) -> bool {
        if self.pending_queue.is_some() {
            return true;
        }
        let status = self.player.status();
        self.current().is_some()
            && self.want_play
            && (status.token != self.current_token
                || matches!(status.state, PlayState::Loading | PlayState::Buffering))
            && status.state != PlayState::Paused
            && !(status.state == PlayState::Idle && status.token == self.current_token && self.current_token != 0)
    }

    // ---- Responses ------------------------------------------------------

    fn on_response(&mut self, response: Response) {
        match response {
            Response::Ready => self.ready = true,
            Response::Page { key, result } => {
                let state = self.pages.entry(key).or_insert_with(PageState::loading);
                state.data = match result {
                    Ok(page) => Loadable::Ready(page),
                    Err(error) => Loadable::Failed(error),
                };
                state.loading_more = false;
            }
            Response::More {
                key,
                sections,
                items,
                token,
                tracks,
            } => {
                if key == "queue" {
                    self.extending = false;
                    let found: Vec<Track> = items.into_iter().filter_map(|i| i.track).collect();
                    if found.is_empty() {
                        self.radio_dry = true;
                    }
                    self.queue.extend_unique(found);
                    self.queue.source.continuation = token;
                    return;
                }
                let Some(state) = self.pages.get_mut(&key) else {
                    return;
                };
                if tracks {
                    state.loading_tracks = false;
                } else {
                    state.loading_more = false;
                }
                let Some(page) = state.data.ready_mut() else {
                    return;
                };
                if tracks {
                    let thumb = page.header.as_ref().and_then(|h| h.thumbnail.clone());
                    page.tracks.extend(items.into_iter().filter_map(|i| i.track).map(|mut t| {
                        if t.thumbnail.is_none() {
                            t.thumbnail = thumb.clone();
                        }
                        t
                    }));
                    page.tracks_continuation = token;
                } else if !sections.is_empty() {
                    page.sections.extend(sections);
                    page.continuation = token;
                } else if !items.is_empty() {
                    // More items for the page's last shelf (a library grid).
                    if let Some(last) = page.sections.last_mut() {
                        last.items.extend(items);
                        last.continuation = token;
                    }
                } else {
                    page.continuation = None;
                    if let Some(last) = page.sections.last_mut() {
                        last.continuation = None;
                    }
                }
            }
            Response::Search { seq, result, more } => {
                if seq != self.search.seq {
                    return;
                }
                self.search.loading_more = false;
                match result {
                    Ok((sections, token)) => {
                        if more {
                            if let Loadable::Ready(existing) = &mut self.search.results {
                                for s in sections {
                                    match existing.last_mut() {
                                        Some(last) if last.layout == Layout::List && s.layout == Layout::List => {
                                            last.items.extend(s.items)
                                        }
                                        _ => existing.push(s),
                                    }
                                }
                            }
                        } else {
                            self.search.results = Loadable::Ready(sections);
                        }
                        self.search.continuation = token;
                    }
                    Err(error) => {
                        if !more {
                            self.search.results = Loadable::Failed(error);
                        }
                    }
                }
            }
            Response::Suggest { seq, result } => {
                if seq == self.search.suggest_seq {
                    self.search.suggestions = result;
                }
            }
            Response::Watch { seq, result, more } => {
                let Some(purpose) = self.watch_purposes.remove(&seq) else {
                    return;
                };
                self.on_watch(purpose, result, more, seq);
            }
            Response::PlaylistTracks { seq, result } => {
                let Some(WatchPurpose::Start { title, .. }) = self.watch_purposes.remove(&seq) else {
                    return;
                };
                if self.pending_queue == Some(seq) {
                    self.pending_queue = None;
                }
                match result {
                    Ok(tracks) if !tracks.is_empty() => {
                        let shuffle = self.queue.shuffle;
                        let start = if shuffle { fastrand::usize(..tracks.len()) } else { 0 };
                        self.queue.replace(
                            tracks,
                            start,
                            Source {
                                title,
                                ..Source::default()
                            },
                        );
                        self.start_current(0.0);
                    }
                    Ok(_) => self.toast("That playlist is empty", ToastKind::Info),
                    Err(error) => self.toast(format!("Could not load the playlist: {error}"), ToastKind::Error),
                }
            }
            Response::Lyrics { video_id, result } => {
                self.lyrics.insert(
                    video_id,
                    match result {
                        Ok(lyrics) => Loadable::Ready(lyrics),
                        Err(error) => Loadable::Failed(error),
                    },
                );
            }
            Response::Resolved {
                token,
                video_id,
                purpose,
                start,
                result,
            } => self.on_resolved(token, video_id, purpose, start, result),
            Response::Account(result) => match result {
                Ok(account) => {
                    if let Some(dialog) = &mut self.sign_in {
                        dialog.busy = false;
                        if let Err(_) = self.save_cookie_from_dialog() {
                            return;
                        }
                        self.sign_in = None;
                        self.toast(format!("Signed in as {}", account.name), ToastKind::Info);
                        self.pages.clear();
                        self.request_page(&self.route.clone(), false);
                    }
                    self.account = Some(account);
                }
                Err(error) => {
                    if let Some(dialog) = &mut self.sign_in {
                        dialog.busy = false;
                        dialog.error = Some(format!(
                            "YouTube Music did not accept that cookie ({error}). Copy it again from a signed-in tab."
                        ));
                        self.backend.send(Request::SetAuth(None));
                        self.signed_in = false;
                    } else if error.contains("not signed in") {
                        // The saved cookie no longer signs in: YouTube ended
                        // the session (a sign-out in that browser, say).
                        self.backend.send(Request::SetAuth(None));
                        crate::platform::forget_cookie(&self.paths.account());
                        self.signed_in = false;
                        self.playlists.clear();
                        self.toast(
                            "Your YouTube Music sign-in has ended. Sign in again to see your library.",
                            ToastKind::Error,
                        );
                    } else {
                        // Most likely offline: keep the sign-in for later.
                        log::warn!("account: {error}");
                    }
                }
            },
            Response::LibraryPlaylists(result) => match result {
                Ok(items) => self.playlists = items,
                Err(error) => log::warn!("library playlists: {error}"),
            },
            Response::Done {
                message,
                ok,
                refresh,
            } => {
                self.toast(message, if ok { ToastKind::Info } else { ToastKind::Error });
                if let Some(key) = refresh {
                    if key == "library-playlists" {
                        self.backend.send(Request::LibraryPlaylists);
                    } else if self.pages.contains_key(&key) || key.starts_with("VL") {
                        // Reload a playlist page someone has open.
                        let route_key = format!("browse:{key}:");
                        if self.pages.remove(&route_key).is_some() && self.route.key() == route_key {
                            self.request_page(&self.route.clone(), true);
                        }
                    }
                }
            }
        }
    }

    fn save_cookie_from_dialog(&mut self) -> Result<(), ()> {
        let Some(dialog) = &self.sign_in else {
            return Err(());
        };
        let cookies = parse_cookies(&dialog.text);
        let header: String = cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        if !crate::platform::save_cookie(&self.paths.account(), &header) {
            self.toast(
                "Signed in, but the sign-in could not be saved for next time",
                ToastKind::Error,
            );
        }
        self.backend.send(Request::LibraryPlaylists);
        Ok(())
    }

    fn on_watch(&mut self, purpose: WatchPurpose, result: Result<WatchQueue, String>, more: bool, seq: u64) {
        match purpose {
            WatchPurpose::Start { video_id, title } => {
                if self.pending_queue != Some(seq) {
                    return;
                }
                self.pending_queue = None;
                match result {
                    Ok(watch) if !watch.tracks.is_empty() => {
                        let start = video_id
                            .as_ref()
                            .and_then(|id| watch.tracks.iter().position(|t| &t.video_id == id))
                            .unwrap_or(0);
                        let source = Source {
                            playlist_id: watch.playlist_id.clone(),
                            continuation: watch.continuation.clone(),
                            watch: true,
                            title,
                        };
                        self.radio_dry = false;
                        self.queue.replace(watch.tracks, start, source);
                        self.start_current(0.0);
                    }
                    Ok(_) => self.toast("Nothing to play there", ToastKind::Info),
                    Err(error) => self.toast(format!("Could not start playback: {error}"), ToastKind::Error),
                }
            }
            WatchPurpose::Radio { after_uid } => {
                if let Ok(watch) = result {
                    // Only if the queue is still the single song it was.
                    if self.queue.entries.len() == 1 && self.queue.entries[0].uid == after_uid {
                        let added = self.queue.extend_unique(watch.tracks);
                        self.queue.source = Source {
                            playlist_id: watch.playlist_id,
                            continuation: watch.continuation,
                            watch: true,
                            title: self.queue.source.title.clone(),
                        };
                        self.radio_dry = added == 0;
                    }
                }
            }
            WatchPurpose::Extend => {
                self.extending = false;
                match result {
                    Ok(watch) => {
                        let added = self.queue.extend_unique(watch.tracks);
                        if more || watch.continuation.is_some() {
                            self.queue.source.continuation = watch.continuation;
                        }
                        if self.queue.source.playlist_id.is_none() {
                            self.queue.source.playlist_id = watch.playlist_id;
                            self.queue.source.watch = true;
                        }
                        if added == 0 {
                            self.radio_dry = true;
                        }
                        // The queue had run out: carry on.
                        if added > 0 && self.player.status().state == PlayState::Idle && self.ended_waiting {
                            self.ended_waiting = false;
                            self.advance(false);
                        }
                    }
                    Err(error) => {
                        log::warn!("radio: {error}");
                        self.radio_dry = true;
                    }
                }
            }
            WatchPurpose::Info { video_id } => {
                if self.now.video_id != video_id {
                    return;
                }
                self.now.info_ready = true;
                if let Ok(watch) = result {
                    self.now.lyrics_id = watch.lyrics_id;
                    self.now.like = watch.like;
                }
                if self.lyrics_shown() {
                    self.request_lyrics();
                }
            }
        }
    }
}


// ---- Playback ---------------------------------------------------------------

impl App {
    /// Plays the queue's current entry from `start` seconds.
    fn start_current(&mut self, start: f64) {
        let Some(entry) = self.queue.current().cloned() else {
            self.player.stop();
            return;
        };
        self.seq += 1;
        let token = self.seq;
        self.current_token = token;
        self.current_uid = Some(entry.uid);
        self.preload = None;
        self.preload_wanted_for = None;
        self.player.clear_preload();
        self.want_play = true;
        self.start_at = start;
        self.ended_waiting = false;
        self.player.stop();
        let video_id = entry.track.video_id.clone();
        if let Some((stream, at)) = self.streams.get(&video_id)
            && at.elapsed() < STREAM_FRESH
        {
            self.player
                .load(token, stream.clone(), Duration::from_secs_f64(start), true);
        } else {
            self.backend.send(Request::Resolve {
                token,
                video_id: video_id.clone(),
                purpose: Purpose::Load,
                start,
            });
        }
        self.track_info(&video_id);
    }

    /// Asks for lyrics and the rating of the song that is now current.
    fn track_info(&mut self, video_id: &str) {
        if self.now.video_id == video_id {
            return;
        }
        self.now = NowInfo {
            video_id: video_id.to_owned(),
            ..NowInfo::default()
        };
        if self.lyrics.len() > 200 {
            self.lyrics.clear();
        }
        let seq = self.next_seq();
        self.watch_purposes.insert(
            seq,
            WatchPurpose::Info {
                video_id: video_id.to_owned(),
            },
        );
        self.backend.send(Request::Watch {
            seq,
            video_id: Some(video_id.to_owned()),
            playlist_id: None,
            params: None,
        });
    }

    /// Asks for the current song's lyrics, once its info says whether
    /// YouTube has them: synced ones from LRCLIB first, then YouTube's.
    pub fn request_lyrics(&mut self) {
        let Some(track) = self.current().cloned() else {
            return;
        };
        // A restored song that has not played yet has no info yet.
        if self.now.video_id != track.video_id {
            self.track_info(&track.video_id);
            return;
        }
        let video_id = track.video_id.clone();
        if self.lyrics.contains_key(&video_id) || !self.now.info_ready {
            return;
        }
        let query = track
            .artists
            .first()
            .filter(|_| self.settings.synced_lyrics)
            .map(|artist| crate::api::lrclib::Query {
                title: track.title.clone(),
                artist: artist.name.clone(),
                album: track.album.as_ref().map(|a| a.name.clone()),
                duration: track.duration,
            });
        let browse_id = self.now.lyrics_id.clone();
        if query.is_none() && browse_id.is_none() {
            self.lyrics.insert(video_id, Loadable::Ready(None));
            return;
        }
        self.lyrics.insert(video_id.clone(), Loadable::Loading);
        self.backend.send(Request::Lyrics {
            video_id,
            browse_id,
            query,
        });
    }

    /// Lyrics are on screen: the side panel's tab, or the full view's.
    pub fn lyrics_shown(&self) -> bool {
        if self.full_view {
            self.full_tab == SideTab::Lyrics
        } else {
            self.settings.side_panel && self.side_tab == SideTab::Lyrics
        }
    }

    /// The current stream's codec and rate, such as "Opus 257 kb/s".
    pub fn stream_quality(&self) -> Option<&str> {
        let track = self.current()?;
        self.streams
            .get(&track.video_id)
            .map(|(stream, _)| stream.quality.as_str())
    }

    /// Resolves the song after this one ahead of time, when resolving is
    /// slow (the account's streams come through yt-dlp, in seconds).
    fn prefetch_next(&mut self) {
        if !(self.settings.premium_quality && self.signed_in) {
            return;
        }
        let Some(entry) = self
            .queue
            .next_index(self.settings.repeat, false)
            .and_then(|i| self.queue.entries.get(i))
        else {
            return;
        };
        let video_id = entry.track.video_id.clone();
        let fresh = self
            .streams
            .get(&video_id)
            .is_some_and(|(_, at)| at.elapsed() < STREAM_FRESH);
        if fresh || self.prefetching.contains(&video_id) || Some(video_id.as_str()) == self.current().map(|t| t.video_id.as_str()) {
            return;
        }
        self.prefetching.insert(video_id.clone());
        self.backend.send(Request::Resolve {
            token: 0,
            video_id,
            purpose: Purpose::Prefetch,
            start: 0.0,
        });
    }

    fn on_resolved(
        &mut self,
        token: u64,
        video_id: String,
        purpose: Purpose,
        start: f64,
        result: Result<(StreamInfo, crate::stream::Via), String>,
    ) {
        if purpose == Purpose::Prefetch {
            self.prefetching.remove(&video_id);
        }
        match (purpose, result) {
            (Purpose::Prefetch, Ok((stream, _))) => {
                self.streams.insert(video_id, (stream, Instant::now()));
            }
            (Purpose::Prefetch, Err(error)) => log::info!("prefetch of {video_id} failed: {error}"),
            (Purpose::Load, Ok((stream, _))) => {
                self.streams
                    .insert(video_id, (stream.clone(), Instant::now()));
                if token == self.current_token {
                    self.player.load(
                        token,
                        stream,
                        Duration::from_secs_f64(start),
                        self.want_play,
                    );
                }
            }
            (Purpose::Preload, Ok((stream, _))) => {
                self.streams
                    .insert(video_id, (stream.clone(), Instant::now()));
                if self.preload.is_some_and(|(t, _)| t == token) {
                    self.player.preload(token, stream);
                }
            }
            (Purpose::Load, Err(error)) => {
                if token != self.current_token {
                    return;
                }
                let title = self.current().map(|t| t.title.clone()).unwrap_or_default();
                log::warn!("cannot play {video_id}: {error}");
                self.failures += 1;
                let why = if error.contains("Premium members") {
                    if self.signed_in {
                        " (a Music Premium song; it needs yt-dlp, set in Settings)"
                    } else {
                        ": only Music Premium members can play it. Sign in with a Premium account."
                    }
                } else {
                    ""
                };
                self.toast(format!("Cannot play \u{201c}{title}\u{201d}{why}"), ToastKind::Error);
                if self.failures < 3 && self.queue.next_index(self.settings.repeat, true).is_some() {
                    self.advance(true);
                } else {
                    self.failures = 0;
                    self.player.stop();
                    self.want_play = false;
                }
            }
            (Purpose::Preload, Err(error)) => {
                log::info!("preload of {video_id} failed: {error}");
                if self.preload.is_some_and(|(t, _)| t == token) {
                    self.preload = None;
                }
            }
        }
    }

    fn on_player(&mut self, event: player::Event) {
        match event {
            player::Event::Audible(token) => {
                if token == self.current_token {
                    self.failures = 0;
                    self.retried = None;
                    self.start_at = 0.0;
                    self.prefetch_next();
                } else if let Some((preload_token, uid)) = self.preload
                    && preload_token == token
                {
                    // The preloaded song took over without a gap.
                    log::info!("gapless switch to the next song");
                    self.preload = None;
                    self.preload_wanted_for = None;
                    if let Some(index) = self.queue.index_of(uid) {
                        self.queue.current = Some(index);
                    }
                    self.current_token = token;
                    self.current_uid = Some(uid);
                    self.start_at = 0.0;
                    if let Some(video_id) = self.current().map(|t| t.video_id.clone()) {
                        self.track_info(&video_id);
                    }
                    self.prefetch_next();
                }
            }
            player::Event::Ended(token) => {
                if token == self.current_token {
                    self.advance(false);
                }
            }
            player::Event::Failed {
                token,
                message,
                expired,
                position,
            } => {
                if token != self.current_token {
                    if self.preload.is_some_and(|(t, _)| t == token) {
                        self.preload = None;
                    }
                    return;
                }
                let video_id = self.current().map(|t| t.video_id.clone()).unwrap_or_default();
                self.streams.remove(&video_id);
                if expired && self.retried != Some(token) {
                    // The address ran out; find it again and carry on from
                    // the same place.
                    self.retried = Some(token);
                    self.backend.send(Request::Resolve {
                        token,
                        video_id,
                        purpose: Purpose::Load,
                        start: position.as_secs_f64(),
                    });
                    return;
                }
                log::warn!("playback failed: {message}");
                self.toast("Playback stopped: the stream failed", ToastKind::Error);
                self.advance(true);
            }
        }
    }

    /// Moves to the next entry and plays it. `manual` is the Next button.
    fn advance(&mut self, manual: bool) {
        match self.queue.next_index(self.settings.repeat, manual) {
            Some(index) => {
                self.queue.current = Some(index);
                self.start_current(0.0);
            }
            None => {
                if self.settings.autoplay && !self.radio_dry {
                    self.ended_waiting = true;
                    self.extend_queue();
                } else {
                    self.player.stop();
                    self.want_play = false;
                    self.start_at = 0.0;
                }
            }
        }
    }

    /// Resolves the next song while this one has a little left, so it starts
    /// without a gap.
    fn maybe_preload(&mut self) {
        if self.preload.is_some() || self.settings.repeat == Repeat::One {
            return;
        }
        let status = self.player.status();
        if status.token != self.current_token || status.state != PlayState::Playing {
            return;
        }
        let Some(duration) = status.duration else {
            return;
        };
        let left = duration.saturating_sub(self.player.position());
        if left > Duration::from_secs(20) {
            return;
        }
        let Some(next) = self.queue.next_index(self.settings.repeat, false) else {
            return;
        };
        let Some(entry) = self.queue.entries.get(next).cloned() else {
            return;
        };
        if self.preload_wanted_for == Some(entry.uid) {
            return;
        }
        self.preload_wanted_for = Some(entry.uid);
        let token = self.next_seq();
        self.preload = Some((token, entry.uid));
        if let Some((stream, at)) = self.streams.get(&entry.track.video_id)
            && at.elapsed() < STREAM_FRESH
        {
            self.player.preload(token, stream.clone());
        } else {
            self.backend.send(Request::Resolve {
                token,
                video_id: entry.track.video_id,
                purpose: Purpose::Preload,
                start: 0.0,
            });
        }
    }

    /// Keeps a few songs ahead in the queue: more of its radio or playlist.
    fn maybe_extend(&mut self) {
        if !self.settings.autoplay || self.extending || self.radio_dry || self.queue.current.is_none() {
            return;
        }
        if self.queue.upcoming() < 3 && self.settings.repeat == Repeat::Off {
            self.extend_queue();
        }
    }

    fn extend_queue(&mut self) {
        if self.extending {
            return;
        }
        let source = self.queue.source.clone();
        match (&source.continuation, source.watch) {
            (Some(token), true) => {
                self.extending = true;
                let seq = self.next_seq();
                self.watch_purposes.insert(seq, WatchPurpose::Extend);
                self.backend.send(Request::WatchMore {
                    seq,
                    token: token.clone(),
                    playlist_id: source.playlist_id.clone(),
                });
            }
            (Some(token), false) => {
                self.extending = true;
                self.backend.send(Request::More {
                    key: "queue".into(),
                    token: token.clone(),
                    tracks: true,
                });
            }
            (None, _) => {
                // The list ran out: carry on with the radio of its last song.
                let Some(last) = self.queue.entries.last().map(|e| e.track.video_id.clone()) else {
                    return;
                };
                self.extending = true;
                let seq = self.next_seq();
                self.watch_purposes.insert(seq, WatchPurpose::Extend);
                self.queue.source.watch = true;
                self.queue.source.playlist_id = Some(format!("RDAMVM{last}"));
                self.backend.send(Request::Watch {
                    seq,
                    video_id: Some(last),
                    playlist_id: None,
                    params: None,
                });
            }
        }
    }

    /// The next entry changed (an edit, shuffle): what was preloaded is no
    /// longer next.
    fn queue_changed(&mut self) {
        let next = self
            .queue
            .next_index(self.settings.repeat, false)
            .and_then(|i| self.queue.entries.get(i))
            .map(|e| e.uid);
        if let Some((_, uid)) = self.preload
            && Some(uid) != next
        {
            self.preload = None;
            self.preload_wanted_for = None;
            self.player.clear_preload();
        }
        if self.playing() {
            self.prefetch_next();
        }
    }

    fn toggle_play(&mut self) {
        let status = self.player.status();
        match status.state {
            PlayState::Playing | PlayState::Buffering => self.player.pause(),
            PlayState::Paused => {
                if status.token == self.current_token {
                    self.player.play();
                } else {
                    self.start_current(self.start_at);
                }
            }
            PlayState::Loading => {
                self.want_play = !self.want_play;
                if self.want_play {
                    self.player.play();
                } else {
                    self.player.pause();
                }
            }
            PlayState::Idle => {
                if self.current().is_some() {
                    if self.current_token != 0 && status.token == self.current_token {
                        // It played to the end: from the top.
                        self.start_current(0.0);
                    } else if self.pending_resolve() {
                        self.want_play = !self.want_play;
                    } else {
                        let at = self.start_at;
                        self.start_current(at);
                    }
                }
            }
        }
    }

    /// The current song is being resolved and the player has not got it yet.
    fn pending_resolve(&self) -> bool {
        self.current_token != 0 && self.player.status().token != self.current_token
    }

    pub fn save_session(&mut self) {
        self.last_saved = Instant::now();
        if !self.settings.restore_session {
            return;
        }
        let session = Session {
            tracks: self.queue.tracks(),
            current: self.queue.current,
            position_secs: self.position(),
            playlist_id: self.queue.source.playlist_id.clone(),
        };
        settings::save(&self.paths.session(), &session);
    }

    pub fn save_settings(&self) {
        settings::save(&self.paths.settings(), &self.settings);
    }
}

// ---- Navigation and actions --------------------------------------------------

impl App {
    pub fn request_page(&mut self, route: &Route, force: bool) {
        let key = route.key();
        if !force
            && let Some(state) = self.pages.get(&key)
            && !matches!(state.data, Loadable::Failed(_))
        {
            return;
        }
        match route {
            Route::Search { query, filter } => {
                let seq = self.next_seq();
                self.search.seq = seq;
                self.search.query = query.clone();
                self.search.filter = *filter;
                self.search.results = Loadable::Loading;
                self.search.continuation = None;
                self.backend.send(Request::Search {
                    seq,
                    query: query.clone(),
                    filter: *filter,
                });
            }
            Route::Settings => {}
            _ => {
                if let Some((browse_id, params)) = route.browse() {
                    self.pages.insert(key.clone(), PageState::loading());
                    self.backend.send(Request::Browse {
                        key,
                        browse_id,
                        params,
                    });
                }
            }
        }
    }

    pub fn navigate(&mut self, route: Route) {
        if route == self.route {
            if matches!(route, Route::Search { .. }) {
                self.request_page(&route, true);
            }
            return;
        }
        let previous = std::mem::replace(&mut self.route, route.clone());
        self.history.push(previous);
        if self.history.len() > 50 {
            self.history.remove(0);
        }
        self.forward.clear();
        if let Route::Search { query, .. } = &route {
            self.search.input = query.clone();
            self.request_page(&route, true);
        } else {
            self.request_page(&route, false);
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    fn go_to(&mut self, route: Route) {
        if let Route::Search { query, .. } = &route {
            self.search.input = query.clone();
            if self.search.query != *query || self.search.filter != route_filter(&route) {
                self.request_page(&route, true);
            }
        } else {
            self.request_page(&route, false);
        }
    }

    fn open(&mut self, target: Target) {
        match target {
            Target::Browse { browse_id, params } => {
                let route = match browse_id.as_str() {
                    "FEmusic_home" if params.is_none() => Route::Home,
                    "FEmusic_explore" => Route::Explore,
                    "FEmusic_liked_playlists" => Route::Library(LibraryTab::Playlists),
                    _ => Route::Browse { browse_id, params },
                };
                self.navigate(route);
            }
            Target::Search { query, .. } => self.navigate(Route::Search {
                query,
                filter: Filter::All,
            }),
            Target::Watch {
                video_id,
                playlist_id,
                params,
            } => {
                self.actions.push(Action::PlayWatch {
                    video_id: Some(video_id),
                    playlist_id,
                    params,
                    title: None,
                });
            }
            Target::WatchPlaylist {
                playlist_id,
                params,
            } => self.actions.push(Action::PlayWatch {
                video_id: None,
                playlist_id: Some(playlist_id),
                params,
                title: None,
            }),
        }
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Navigate(route) => self.navigate(route),
            Action::Back => {
                if let Some(route) = self.history.pop() {
                    let current = std::mem::replace(&mut self.route, route.clone());
                    self.forward.push(current);
                    self.go_to(route);
                }
            }
            Action::Forward => {
                if let Some(route) = self.forward.pop() {
                    let current = std::mem::replace(&mut self.route, route.clone());
                    self.history.push(current);
                    self.go_to(route);
                }
            }
            Action::Refresh => {
                let route = self.route.clone();
                self.request_page(&route, true);
                if self.signed_in {
                    self.backend.send(Request::LibraryPlaylists);
                }
            }
            Action::Open(target) => self.open(target),
            Action::PlayTracks {
                tracks,
                start,
                source,
            } => {
                if tracks.is_empty() {
                    return;
                }
                self.pending_queue = None;
                self.radio_dry = false;
                self.queue.replace(tracks, start, source);
                self.start_current(0.0);
            }
            Action::PlayTrack(track) => {
                self.pending_queue = None;
                self.radio_dry = false;
                let video_id = track.video_id.clone();
                self.queue.replace(
                    vec![track],
                    0,
                    Source {
                        playlist_id: Some(format!("RDAMVM{video_id}")),
                        watch: true,
                        ..Source::default()
                    },
                );
                self.start_current(0.0);
                if let Some(uid) = self.queue.current().map(|e| e.uid) {
                    let seq = self.next_seq();
                    self.watch_purposes.insert(seq, WatchPurpose::Radio { after_uid: uid });
                    self.backend.send(Request::Watch {
                        seq,
                        video_id: Some(video_id),
                        playlist_id: None,
                        params: None,
                    });
                }
            }
            Action::StartRadio(track) => {
                self.actions.push(Action::PlayWatch {
                    video_id: Some(track.video_id.clone()),
                    playlist_id: Some(format!("RDAMVM{}", track.video_id)),
                    params: Some("wAEB".into()),
                    title: Some(format!("{} radio", track.title)),
                });
            }
            Action::PlayWatch {
                video_id,
                playlist_id,
                params,
                title,
            } => {
                let seq = self.next_seq();
                self.pending_queue = Some(seq);
                self.watch_purposes.insert(
                    seq,
                    WatchPurpose::Start {
                        video_id: video_id.clone(),
                        title,
                    },
                );
                self.backend.send(Request::Watch {
                    seq,
                    video_id,
                    playlist_id,
                    params,
                });
            }
            Action::PlayPlaylist {
                playlist_id,
                shuffle,
                title,
            } => {
                if shuffle != self.queue.shuffle {
                    self.queue.set_shuffle(shuffle);
                    self.settings.shuffle = shuffle;
                }
                let seq = self.next_seq();
                self.pending_queue = Some(seq);
                self.watch_purposes.insert(
                    seq,
                    WatchPurpose::Start {
                        video_id: None,
                        title,
                    },
                );
                self.backend.send(Request::PlaylistTracks { seq, playlist_id });
            }
            Action::PlayNext(tracks) => {
                let n = tracks.len();
                let empty = self.queue.current.is_none();
                self.queue.play_next(tracks);
                self.toast(
                    if n == 1 { "Song will play next".to_owned() } else { format!("{n} songs will play next") },
                    ToastKind::Info,
                );
                if empty {
                    self.start_current(0.0);
                } else {
                    self.queue_changed();
                }
            }
            Action::AddToQueue(tracks) => {
                let n = tracks.len();
                let empty = self.queue.current.is_none();
                self.queue.append(tracks);
                self.toast(
                    if n == 1 { "Added to queue".to_owned() } else { format!("{n} songs added to queue") },
                    ToastKind::Info,
                );
                if empty {
                    self.start_current(0.0);
                } else {
                    self.queue_changed();
                }
            }
            Action::PlayQueueEntry(uid) => {
                if let Some(index) = self.queue.index_of(uid) {
                    self.queue.current = Some(index);
                    self.start_current(0.0);
                }
            }
            Action::RemoveFromQueue(uid) => {
                let was_current = self.queue.remove(uid);
                if was_current {
                    if self.queue.current.is_some() {
                        self.start_current(0.0);
                    } else {
                        self.player.stop();
                    }
                } else {
                    self.queue_changed();
                }
            }
            Action::MoveInQueue(from, to) => {
                self.queue.move_entry(from, to);
                self.queue_changed();
            }
            Action::ClearQueue => {
                self.queue.clear_upcoming();
                self.queue_changed();
                self.radio_dry = true;
            }
            Action::TogglePlay => self.toggle_play(),
            Action::Next => {
                if self.queue.next_index(self.settings.repeat, true).is_none() && self.settings.autoplay {
                    self.ended_waiting = true;
                    self.radio_dry = false;
                    self.extend_queue();
                } else {
                    self.advance(true);
                }
            }
            Action::Previous => {
                if self.position() > 3.0 {
                    self.actions.push(Action::Seek(0.0));
                } else if let Some(index) = self.queue.previous_index(self.settings.repeat) {
                    self.queue.current = Some(index);
                    self.start_current(0.0);
                } else {
                    self.actions.push(Action::Seek(0.0));
                }
            }
            Action::Seek(secs) => {
                let status = self.player.status();
                if status.token == self.current_token && status.token != 0 && status.state != PlayState::Idle {
                    self.player.seek(Duration::from_secs_f64(secs.max(0.0)));
                } else {
                    self.start_at = secs.max(0.0);
                }
            }
            Action::SeekBy(delta) => {
                let to = (self.position() + delta).max(0.0);
                let to = self.duration().map_or(to, |d| to.min((d - 0.5).max(0.0)));
                self.actions.push(Action::Seek(to));
            }
            Action::SetVolume(volume) => {
                self.settings.volume = volume.clamp(0.0, 1.0);
                self.settings.muted = false;
                self.player.set_volume(self.settings.volume);
            }
            Action::ToggleMute => {
                self.settings.muted = !self.settings.muted;
                self.player
                    .set_volume(if self.settings.muted { 0.0 } else { self.settings.volume });
                self.save_settings();
            }
            Action::ToggleShuffle => {
                let on = !self.queue.shuffle;
                self.queue.set_shuffle(on);
                self.settings.shuffle = on;
                self.queue_changed();
                self.save_settings();
            }
            Action::SetShuffle(on) => {
                if on != self.queue.shuffle {
                    self.queue.set_shuffle(on);
                    self.settings.shuffle = on;
                    self.queue_changed();
                    self.save_settings();
                }
            }
            Action::CycleRepeat => {
                self.settings.repeat = self.settings.repeat.next();
                self.queue_changed();
                self.save_settings();
            }
            Action::Rate(video_id, rating) => {
                if !self.require_sign_in() {
                    return;
                }
                if self.now.video_id == video_id {
                    self.now.like = Some(match rating {
                        Rating::Like => Like::Liked,
                        Rating::Dislike => Like::Disliked,
                        Rating::None => Like::Indifferent,
                    });
                }
                let skip = rating == Rating::Dislike
                    && self.current().is_some_and(|t| t.video_id == video_id);
                self.backend.send(Request::Rate { video_id, rating });
                if skip {
                    self.advance(true);
                }
            }
            Action::SavePlaylist(id, save) => {
                if !self.require_sign_in() {
                    return;
                }
                if let Some(state) = self.pages.get_mut(&self.route.key())
                    && let Some(page) = state.data.ready_mut()
                    && let Some(header) = &mut page.header
                {
                    header.saved = Some(save);
                }
                self.backend.send(Request::SavePlaylist {
                    playlist_id: id,
                    save,
                });
                self.backend.send(Request::LibraryPlaylists);
            }
            Action::Subscribe(channel_id, subscribe) => {
                if !self.require_sign_in() {
                    return;
                }
                if let Some(state) = self.pages.get_mut(&self.route.key())
                    && let Some(page) = state.data.ready_mut()
                    && let Some(header) = &mut page.header
                {
                    header.subscribed = Some(subscribe);
                }
                self.backend.send(Request::Subscribe {
                    channel_id,
                    subscribe,
                });
            }
            Action::PickPlaylist(tracks) => {
                if !self.require_sign_in() {
                    return;
                }
                if self.playlists.is_empty() {
                    self.backend.send(Request::LibraryPlaylists);
                }
                self.picker = Some(tracks);
            }
            Action::AddToPlaylist {
                playlist_id,
                title,
                tracks,
            } => {
                self.picker = None;
                let id = playlist_id.trim_start_matches("VL").to_owned();
                self.backend.send(Request::AddToPlaylist {
                    playlist_id: id,
                    title,
                    video_ids: tracks.into_iter().map(|t| t.video_id).collect(),
                });
            }
            Action::RemoveFromPlaylist { playlist_id, track } => {
                let Some(set_video_id) = track.set_video_id.clone() else {
                    return;
                };
                let key = self.route.key();
                if let Some(state) = self.pages.get_mut(&key)
                    && let Some(page) = state.data.ready_mut()
                {
                    page.tracks
                        .retain(|t| t.set_video_id.as_deref() != Some(&set_video_id));
                }
                self.backend.send(Request::RemoveFromPlaylist {
                    key,
                    playlist_id,
                    video_id: track.video_id,
                    set_video_id,
                });
            }
            Action::CreatePlaylist { title, tracks } => {
                self.new_playlist = None;
                self.picker = None;
                if title.trim().is_empty() {
                    return;
                }
                self.backend.send(Request::CreatePlaylist {
                    title: title.trim().to_owned(),
                    video_ids: tracks.into_iter().map(|t| t.video_id).collect(),
                });
            }
            Action::LoadMore(key) => {
                let Some(state) = self.pages.get_mut(&key) else {
                    return;
                };
                if state.loading_more {
                    return;
                }
                let Some(page) = state.data.ready() else {
                    return;
                };
                let token = page
                    .continuation
                    .clone()
                    .or_else(|| page.sections.last().and_then(|s| s.continuation.clone()));
                if let Some(token) = token {
                    state.loading_more = true;
                    self.backend.send(Request::More {
                        key,
                        token,
                        tracks: false,
                    });
                }
            }
            Action::LoadMoreTracks(key) => {
                let Some(state) = self.pages.get_mut(&key) else {
                    return;
                };
                if state.loading_tracks {
                    return;
                }
                if let Some(token) = state.data.ready().and_then(|p| p.tracks_continuation.clone()) {
                    state.loading_tracks = true;
                    self.backend.send(Request::More {
                        key,
                        token,
                        tracks: true,
                    });
                }
            }
            Action::SearchMore => {
                if self.search.loading_more {
                    return;
                }
                if let Some(token) = self.search.continuation.clone() {
                    self.search.loading_more = true;
                    self.backend.send(Request::SearchMore {
                        seq: self.search.seq,
                        token,
                    });
                }
            }
            Action::Search(query) => {
                let query = query.trim().to_owned();
                if query.is_empty() {
                    return;
                }
                self.search.suggestions = Suggestions::default();
                self.search.suggest_for.clear();
                let filter = if matches!(self.route, Route::Search { .. }) {
                    self.search.filter
                } else {
                    Filter::All
                };
                self.navigate(Route::Search { query, filter });
            }
            Action::SetSearchFilter(filter) => {
                let query = self.search.query.clone();
                if !query.is_empty() {
                    let route = Route::Search { query, filter };
                    self.route = route.clone();
                    self.request_page(&route, true);
                }
            }
            Action::Suggest(input) => {
                if input == self.search.suggest_for {
                    return;
                }
                self.search.suggest_for = input.clone();
                if input.trim().is_empty() {
                    self.search.suggestions = Suggestions::default();
                    return;
                }
                let seq = self.next_seq();
                self.search.suggest_seq = seq;
                self.backend.send(Request::Suggest { seq, input });
            }
            Action::ShowSidePanel(tab) => {
                if self.full_view {
                    self.full_tab = tab;
                } else {
                    self.settings.side_panel = true;
                    self.side_tab = tab;
                    self.save_settings();
                }
                if tab == SideTab::Lyrics {
                    self.request_lyrics();
                }
            }
            Action::ToggleFullView => {
                self.full_view = !self.full_view;
                if self.full_view && self.full_tab == SideTab::Lyrics {
                    self.request_lyrics();
                }
            }
            Action::LastFmConnect => {
                let (key, secret) = self.lastfm_form.clone();
                if key.trim().is_empty() || secret.trim().is_empty() {
                    self.toast("Enter the API key and the shared secret first", ToastKind::Info);
                } else {
                    self.lastfm.connect(&key, &secret);
                    self.lastfm_form = (String::new(), String::new());
                }
            }
            Action::LastFmDisconnect => {
                self.lastfm.disconnect();
                self.toast("Disconnected from Last.fm", ToastKind::Info);
            }
            Action::ToggleSidePanel => {
                self.settings.side_panel = !self.settings.side_panel;
                if self.settings.side_panel && self.side_tab == SideTab::Lyrics {
                    self.request_lyrics();
                }
                self.save_settings();
            }
            Action::ToggleSidebar => {
                self.settings.sidebar = !self.settings.sidebar;
                self.save_settings();
            }
            Action::OpenSignIn => {
                self.sign_in = Some(SignInDialog {
                    text: String::new(),
                    error: None,
                    busy: false,
                });
            }
            Action::SignIn(text) => {
                let cookies = parse_cookies(&text);
                let auth = Auth {
                    cookies,
                    auth_user: 0,
                };
                let Some(dialog) = &mut self.sign_in else {
                    return;
                };
                if !auth.is_usable() {
                    dialog.error = Some(
                        "That does not look like a signed-in YouTube Music cookie: it has no SAPISID. Copy the whole cookie header from a request to music.youtube.com.".into(),
                    );
                    return;
                }
                dialog.text = text;
                dialog.busy = true;
                dialog.error = None;
                self.backend.send(Request::SetAuth(Some(auth)));
                self.signed_in = true;
                self.backend.send(Request::Account);
            }
            Action::SignOut => {
                self.backend.send(Request::SetAuth(None));
                crate::platform::forget_cookie(&self.paths.account());
                self.signed_in = false;
                self.account = None;
                self.playlists.clear();
                self.pages.clear();
                self.toast("Signed out", ToastKind::Info);
                let route = Route::Home;
                self.route = route.clone();
                self.history.clear();
                self.forward.clear();
                self.request_page(&route, true);
            }
            Action::ApplySettings(new) => {
                let new = *new;
                if new.language != self.settings.language || new.region != self.settings.region {
                    self.backend.send(Request::SetLanguage {
                        language: new.language.clone(),
                        region: (!new.region.trim().is_empty()).then(|| new.region.trim().to_owned()),
                    });
                    self.pages.clear();
                    self.request_page(&Route::Home, true);
                }
                if new.use_ytdlp != self.settings.use_ytdlp || new.ytdlp_path != self.settings.ytdlp_path {
                    self.backend.send(Request::SetYtDlp {
                        enabled: new.use_ytdlp,
                        path: (!new.ytdlp_path.trim().is_empty()).then(|| new.ytdlp_path.trim().into()),
                    });
                }
                if new.premium_quality != self.settings.premium_quality {
                    self.backend.send(Request::SetPremium(new.premium_quality));
                    // Streams found at the other quality are found again.
                    self.streams.clear();
                }
                if new.normalize != self.settings.normalize {
                    self.player.set_normalize(new.normalize);
                }
                if new.discord != self.settings.discord || new.discord_client_id != self.settings.discord_client_id {
                    // Dropping it clears the activity and ends its thread.
                    self.presence = None;
                    if new.discord {
                        let id = client_id_or_default(&new.discord_client_id);
                        self.presence = Some(discord::Presence::spawn(&id));
                    }
                }
                if new.synced_lyrics != self.settings.synced_lyrics {
                    self.lyrics.clear();
                }
                if (new.zoom - self.settings.zoom).abs() > f32::EPSILON {
                    self.ctx.set_zoom_factor(new.zoom.clamp(0.7, 2.0));
                }
                if !new.restore_session {
                    let _ = std::fs::remove_file(self.paths.session());
                }
                self.settings = new;
                self.save_settings();
            }
            Action::CopyLink(url) => {
                self.ctx.copy_text(url);
                self.toast("Link copied", ToastKind::Info);
            }
            Action::OpenUrl(url) => {
                self.ctx.open_url(egui::OpenUrl::new_tab(url));
            }
            Action::ClearImageCache => {
                let freed = crate::images::clear_cache(&self.paths.images());
                self.toast(format!("Freed {:.1} MB", freed as f64 / 1_048_576.0), ToastKind::Info);
            }
            Action::ShowWindow => self.wants_show = true,
            Action::Quit => {
                self.quit = true;
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn require_sign_in(&mut self) -> bool {
        if self.signed_in {
            return true;
        }
        self.toast("Sign in to do that", ToastKind::Info);
        self.actions.push(Action::OpenSignIn);
        false
    }
}

/// How long a resolved stream address is used before it is resolved again;
/// YouTube's last about six hours.
const STREAM_FRESH: Duration = Duration::from_secs(3600);

fn client_id_or_default(id: &str) -> String {
    let id = id.trim();
    if id.is_empty() { discord::DEFAULT_CLIENT_ID.to_owned() } else { id.to_owned() }
}

fn route_filter(route: &Route) -> Filter {
    match route {
        Route::Search { filter, .. } => *filter,
        _ => Filter::All,
    }
}

// ---- The desktop: media keys, the overlay, the tray ---------------------------

impl App {
    fn media_controls(&mut self) {
        use fastframe_now_playing::{Command, Controls, Playback, State, Track as NpTrack};
        let Some(controls) = &self.now_playing else {
            return;
        };
        for command in controls.commands() {
            match command {
                Command::Play => {
                    if !self.playing() {
                        self.toggle_play();
                    }
                }
                Command::Pause | Command::Stop => {
                    if self.playing() {
                        self.player.pause();
                    }
                }
                Command::PlayPause => self.toggle_play(),
                Command::Next => self.actions.push(Action::Next),
                Command::Previous => self.actions.push(Action::Previous),
                Command::SeekBy(ms) => self.actions.push(Action::SeekBy(ms as f64 / 1000.0)),
                Command::SetPosition { position, .. } => {
                    self.actions.push(Action::Seek(position.as_secs_f64()));
                }
                Command::SetVolume(v) => self.actions.push(Action::SetVolume(v as f32)),
                Command::SetShuffle(on) => {
                    if on != self.queue.shuffle {
                        self.actions.push(Action::ToggleShuffle);
                    }
                }
                Command::SetRepeat(_) => self.actions.push(Action::CycleRepeat),
                Command::Raise => self.wants_show = true,
                Command::Quit => self.actions.push(Action::Quit),
                Command::OpenUri(_) => {}
            }
        }
        let playback = if self.current().is_none() {
            Playback::Stopped
        } else if self.playing() {
            Playback::Playing
        } else {
            Playback::Paused
        };
        let track = self.current().cloned();
        let art_file = track
            .as_ref()
            .and_then(|t| t.thumbnail.clone())
            .and_then(|url| self.images.file(&url, 512));
        let uid = self.current_uid.unwrap_or(0);
        let duration = self.duration().map(Duration::from_secs_f64);
        let state = State {
            playback,
            track: track.map(|t| NpTrack {
                id: format!("{uid}"),
                title: t.title.clone(),
                artists: t.artists.iter().map(|a| a.name.clone()).collect(),
                album: t.album.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
                duration,
                art_file,
                art_url: t.thumbnail.clone(),
                url: Some(format!("https://music.youtube.com/watch?v={}", t.video_id)),
                ..NpTrack::default()
            }),
            position: Duration::from_secs_f64(self.position().max(0.0)),
            volume: Some(f64::from(self.settings.volume)),
            shuffle: Some(self.queue.shuffle),
            repeat: Some(match self.settings.repeat {
                Repeat::Off => fastframe_now_playing::Repeat::Off,
                Repeat::All => fastframe_now_playing::Repeat::Playlist,
                Repeat::One => fastframe_now_playing::Repeat::Track,
            }),
            controls: Controls {
                seek: self.current().is_some(),
                ..Controls::default()
            },
        };
        if let Some(controls) = &mut self.now_playing {
            controls.update(state);
        }
    }

    fn discord_client_id(&self) -> String {
        client_id_or_default(&self.settings.discord_client_id)
    }

    /// Tells Discord what plays; nothing while paused.
    fn presence_tick(&mut self) {
        if self.presence.is_none() {
            return;
        }
        let activity = self.current().filter(|_| self.playing()).map(|track| {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            let start_ms = now_ms - (self.position() * 1000.0) as i64;
            let end_ms = self.duration().map(|d| start_ms + (d * 1000.0) as i64);
            discord::Activity {
                title: track.title.clone(),
                artist: track.artist_line(),
                album: track.album.as_ref().map(|a| a.name.clone()),
                art_url: track
                    .thumbnail
                    .as_deref()
                    .map(|url| sized_thumbnail(url, 512))
                    .filter(|url| url.starts_with("https://") && url.len() <= 256),
                url: format!("https://music.youtube.com/watch?v={}", track.video_id),
                artist_url: track
                    .artists
                    .first()
                    .and_then(|a| a.id.as_ref())
                    .map(|id| format!("https://music.youtube.com/channel/{id}")),
                start_ms,
                end_ms,
            }
        });
        if let Some(presence) = &mut self.presence {
            presence.set(activity);
        }
    }

    fn scrobble_tick(&mut self) {
        let playing = self.playing();
        let position = self.position();
        let duration = self.duration();
        let uid = self.current_uid.unwrap_or(0);
        let track = self.queue.current_track().cloned();
        let current = track.as_ref().filter(|_| self.current_token != 0).map(|track| lastfm::Current {
            uid,
            track,
            position,
            duration,
            playing,
        });
        for notice in self.lastfm.tick(current, self.settings.scrobble) {
            match notice {
                lastfm::Notice::OpenUrl(url) => self.ctx.open_url(egui::OpenUrl::new_tab(url)),
                lastfm::Notice::Toast { text, error } => {
                    self.toast(text, if error { ToastKind::Error } else { ToastKind::Info })
                }
            }
        }
    }

    fn tray_events(&mut self) {
        let playing = self.playing();
        let title = self
            .current()
            .map(|t| format!("{} \u{2014} {}", t.title, t.artist_line()));
        let Some(tray) = &mut self.tray else {
            return;
        };
        for event in tray.events() {
            match event {
                fastframe_tray::Event::Toggle | fastframe_tray::Event::Show => self.wants_show = true,
                fastframe_tray::Event::Menu(id) => match id {
                    TRAY_SHOW => self.wants_show = true,
                    TRAY_PLAY => self.actions.push(Action::TogglePlay),
                    TRAY_NEXT => self.actions.push(Action::Next),
                    TRAY_PREVIOUS => self.actions.push(Action::Previous),
                    TRAY_QUIT => self.actions.push(Action::Quit),
                    _ => {}
                },
            }
        }
        if self.tray_playing != Some(playing) {
            self.tray_playing = Some(playing);
            tray.set_label(TRAY_PLAY, if playing { "Pause" } else { "Play" });
            let tooltip = match &title {
                Some(t) => format!("Native YTM\n{t}"),
                None => "Native YTM".into(),
            };
            // The tooltip is limited to 127 characters on Windows.
            tray.set_tooltip(tooltip.chars().take(120).collect::<String>());
        }
    }

    pub fn tray_shown(&self) -> bool {
        self.tray.as_ref().is_some_and(fastframe_tray::Tray::is_shown)
    }

    /// The window's close button: hide to the tray while music may play, or
    /// quit.
    pub fn on_close_requested(&mut self) {
        if !self.quit && self.settings.close_to_tray && self.tray_shown() {
            self.hide = true;
        } else {
            self.quit = true;
        }
    }

    pub fn shutdown(&mut self) {
        self.save_session();
        self.save_settings();
        self.player.stop();
    }
}

impl fastframe_shell::Resident for App {
    fn closed(&self) -> fastframe_shell::Closed {
        if !self.quit && self.hide {
            fastframe_shell::Closed::Hide
        } else {
            fastframe_shell::Closed::Quit
        }
    }

    fn window_gone(&mut self) {
        self.hide = false;
        self.wants_show = false;
        self.save_session();
    }

    fn headless_frame(&mut self, _ctx: &egui::Context) -> fastframe_shell::Headless {
        self.background();
        if self.quit {
            fastframe_shell::Headless::Quit
        } else if self.wants_show {
            self.wants_show = false;
            fastframe_shell::Headless::Show
        } else {
            fastframe_shell::Headless::Wait
        }
    }

    fn shutdown(&mut self) {
        App::shutdown(self);
    }
}

fn tray_config() -> fastframe_tray::Config {
    use fastframe_tray::MenuItem;
    fastframe_tray::Config {
        id: "native-ytm",
        title: "Native YTM".into(),
        icon: crate::theme::app_icon_rgba,
        template_icon: None,
        themed_icon: false,
        menu_on_click: false,
        menu: vec![
            MenuItem::action(TRAY_SHOW, "Show Native YTM"),
            MenuItem::Separator,
            MenuItem::action(TRAY_PLAY, "Play"),
            MenuItem::action(TRAY_NEXT, "Next"),
            MenuItem::action(TRAY_PREVIOUS, "Previous"),
            MenuItem::Separator,
            MenuItem::action(TRAY_QUIT, "Quit"),
        ],
    }
}
