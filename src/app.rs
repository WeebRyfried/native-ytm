//! The application: its state, the frame loop, and what every action does.
//!
//! Views draw from `App` and push `Action`s; `apply` runs them after the
//! frame. Network answers and player events come in at the start of each
//! frame. The interface is optimistic: a clicked song is the playing song at
//! once, and the backend catches up behind it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::api::client::{Auth, parse_cookies};
use crate::api::model::*;
use crate::api::music::{Filter, Rating};
use crate::backend::{Backend, How, Purpose, Request, Response};
use crate::cache::Mark;
use crate::images::Images;
use crate::model::*;
use crate::player::{self, Player, State as PlayState, StreamInfo};
use crate::queue::{Queue, Source};
use crate::settings::{self, Paths, Repeat, Session, Settings};
use crate::{discord, lastfm, local, net, update};

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
    /// The radio of a song found on YouTube Music for the last song on
    /// this PC: everything but that song itself, when it is the same song.
    RadioAfter { skip: Option<String> },
    /// Lyrics and rating for the song now playing.
    Info { video_id: String },
    /// Where the next song's lyrics are, to look them up before it plays.
    Lyrics { track: Track },
    /// A whole playlist's songs, to download.
    Offline { title: String },
}

/// Songs being downloaded for offline listening, one at a time.
#[derive(Default)]
pub struct Downloads {
    /// Waiting their turn.
    pub waiting: VecDeque<Track>,
    active: Option<Download>,
    /// This round: downloaded, and failed, for the line on the Cached page
    /// and the note at the end.
    pub done: usize,
    pub failed: usize,
}

struct Download {
    track: Track,
    /// The stream's resolve, by its token.
    token: u64,
    /// The file coming down, once its stream is found.
    source: Option<(crate::player::source::RemoteSource, Arc<crate::player::source::Shared>)>,
    progress: f32,
}

impl Downloads {
    pub fn busy(&self) -> bool {
        self.active.is_some() || !self.waiting.is_empty()
    }

    /// The song downloading, and how far it is.
    pub fn active(&self) -> Option<(&Track, f32)> {
        self.active.as_ref().map(|d| (&d.track, d.progress))
    }

    fn has(&self, video_id: &str) -> bool {
        self.active.as_ref().is_some_and(|d| d.track.video_id == video_id)
            || self.waiting.iter().any(|t| t.video_id == video_id)
    }
}

#[derive(Default)]
pub struct NowInfo {
    pub video_id: String,
    pub lyrics_id: Option<String>,
    pub like: Option<Like>,
    /// The year it came out, from YouTube Music's queue for it.
    pub year: Option<u16>,
    /// The song's info arrived (or failed): whether YouTube has lyrics for
    /// it is known.
    pub info_ready: bool,
}

/// An artist's page: what Last.fm says of them (without a Last.fm account,
/// nothing), and their card on YouTube Music.
#[derive(Clone, Debug, Default)]
pub struct ArtistPage {
    pub lastfm: Option<Result<lastfm::ArtistInfo, String>>,
    pub youtube: Option<Item>,
}

/// Where a quick start's switch to the better stream is, for the full
/// view to show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UpgradePhase {
    /// Playing the quick stream; yt-dlp is finding the better one.
    Finding,
    /// The better stream is decoding; it is heard from this point on.
    Switching(f64),
    /// Heard now, since this moment.
    Done(Instant),
    /// There is none better (or it could not be had); the quick one plays on.
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct Upgrade {
    pub token: u64,
    pub phase: UpgradePhase,
    /// The stream heard before the switch, and the one after.
    pub from: Option<String>,
    pub to: Option<String>,
    /// The quick stream failed (a Premium members' song): the better one
    /// is all there is.
    quick_failed: bool,
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

/// The stats page: which stats it shows, and those loaded, with when and,
/// for the app's own, as of how many plays written.
pub struct StatsView {
    pub source: crate::stats::Source,
    pub period: crate::stats::Period,
    /// The source has been chosen (Last.fm, when connected, the first time).
    chosen: bool,
    pub summaries: HashMap<(crate::stats::Source, crate::stats::Period), (Loadable<crate::stats::Summary>, Instant, u64)>,
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
    /// Streams found while the pointer rested on a song: the quick ones, to
    /// start on. Apart from `streams`, so the account's better one is still
    /// found when the song plays.
    warm: HashMap<String, (StreamInfo, Instant)>,
    /// Songs being resolved ahead of time, so a slow resolve (yt-dlp) is
    /// done before they are wanted.
    prefetching: HashSet<String>,
    /// The song being found through yt-dlp because it was pointed at.
    account_prewarm: Option<String>,
    /// A quick start, switching to the better stream when it comes.
    pub upgrade: Option<Upgrade>,
    /// The stream the player has for the current token: its quality and
    /// container.
    loaded: Option<(u64, StreamInfo)>,
    /// Songs kept on disk.
    pub audio_cache: Arc<crate::cache::AudioCache>,
    /// Lyrics found before, on disk.
    pub lyrics_cache: crate::lyrics::LyricsCache,
    /// Home, Explore and the library's pages as they last loaded.
    kept_pages: crate::kept_pages::KeptPages,
    /// Songs whose lyrics came back incomplete, and when to ask again.
    lyrics_retry: HashMap<String, Instant>,
    /// Songs whose lyrics were sent to be translated, and when.
    lyrics_translating: HashMap<String, Instant>,
    /// How each song on disk stands, for the marks beside their names.
    pub offline: HashMap<String, Mark>,
    offline_gen: u64,
    marks_dirty: bool,
    pub downloads: Downloads,
    download_done: (std::sync::mpsc::Sender<(String, bool)>, Receiver<(String, bool)>),
    /// Whether the internet is there.
    network: net::Network,
    was_online: bool,
    /// Playback stopped for want of the network; it carries on when it
    /// is back, from `start_at`.
    stalled: bool,
    /// The token last added to the account's history.
    history_sent: u64,
    /// Found once, for Settings: whether yt-dlp has a JavaScript runtime.
    js_runtime: Option<bool>,
    /// What is kept on disk, by kind, as last measured.
    storage: Arc<parking_lot::Mutex<Measured>>,
    /// The Cached page's lists (downloaded, played lately), as of a
    /// generation of the cache.
    cached_lists: Option<(u64, Arc<Vec<Track>>, Arc<Vec<Track>>)>,
    /// A queue answer is on its way; the bar shows it loading.
    pub pending_queue: Option<u64>,
    extending: bool,
    radio_dry: bool,
    /// The queue ran out and more is being fetched to carry on.
    ended_waiting: bool,
    pub now: NowInfo,
    pub lyrics: HashMap<String, Loadable<Option<Arc<Lyrics>>>>,

    pub signed_in: bool,
    pub account: Option<Account>,
    pub playlists: Vec<Item>,
    pub sign_in: Option<SignInDialog>,
    pub picker: Option<Vec<Track>>,
    pub new_playlist: Option<(String, Vec<Track>)>,
    /// A playlist being renamed (its id, the name being typed), or about
    /// to be deleted (its id and name).
    pub rename_playlist: Option<(String, String)>,
    pub delete_playlist: Option<(String, String)>,
    pub toasts: Vec<Toast>,
    pub side_tab: SideTab,
    /// The full-window player: the artwork, with the queue or the lyrics.
    pub full_view: bool,
    pub full_tab: SideTab,
    /// The window fills the screen with the full view, the player bar
    /// hiding while the mouse rests; and whether the full view was open
    /// before, to go back to.
    pub fullscreen: bool,
    full_view_before: bool,
    /// When the mouse last moved (or a key went down), in egui's time.
    pub active_at: f64,
    /// Songs in the person's own folders.
    pub local: local::LocalLibrary,
    /// A search's songs from those folders, for its query, as of a
    /// generation of the library.
    local_hits: Option<(String, u64, Arc<Vec<Track>>)>,
    /// Artists' pages, by name in lower case.
    pub artists: HashMap<String, Loadable<ArtistPage>>,
    /// An artist's songs on this PC, as of a generation of the library.
    artist_songs: Option<(String, u64, Arc<Vec<Track>>)>,
    /// Songs on this PC still without a picture, to look for online two at
    /// a time; how many are being looked for; and the library's generation
    /// last looked at.
    art_queue: VecDeque<local::ArtQuery>,
    art_flight: usize,
    art_generation: u64,
    /// A song being found on YouTube Music to play.
    song_search: Option<u64>,
    /// The last song on this PC being found on YouTube Music, for the
    /// radio to carry on with.
    radio_seed: Option<u64>,
    /// The folder picker is open; what it picks comes here.
    pub picking_folder: bool,
    folder_picked: (std::sync::mpsc::Sender<Option<std::path::PathBuf>>, Receiver<Option<std::path::PathBuf>>),
    pub updater: update::Updater,
    /// When to look for an update next on its own.
    update_due: Option<Instant>,
    /// Write the session whatever the settings say (an update's restart).
    keep_session: bool,
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
    /// Settings changed and not yet written (a slider being dragged writes
    /// once it rests).
    settings_dirty: Option<Instant>,
    /// Windows telling when the folders of music change, and whether they
    /// have since they were last looked through.
    folder_watch: Option<crate::watch::FolderWatch>,
    folders_changed: Arc<AtomicBool>,
    /// Windows starts the app when this user signs in.
    pub autostart: bool,
    /// The plays the app hears, kept for its stats, and the stats page.
    pub play_log: crate::stats::PlayLog,
    pub stats: StatsView,
    stats_done: (std::sync::mpsc::Sender<(crate::stats::Period, crate::stats::Summary)>, Receiver<(crate::stats::Period, crate::stats::Summary)>),
    /// Recent searches and lyric offsets, and when they last changed.
    pub personal: crate::personal::Personal,
    personal_dirty: Option<Instant>,
    pub window_styled: bool,
    /// The visualiser's spectrum, from frame to frame.
    pub analyser: crate::visualiser::Analyser,
    /// Home's parts still to fetch at once while Quick picks is not in.
    home_quick_left: u8,
    pub drag_from: Option<usize>,
    /// A scroll position to jump the page to once (screenshots).
    pub scroll_to: Option<f32>,
}

impl App {
    pub fn new(waker: fastframe_shell::Waker) -> Self {
        let paths = Paths::new();
        let settings: Settings = settings::load(&paths.settings());
        crate::theme::set(settings.theme);
        let wake: Arc<dyn Fn() + Send + Sync> = {
            let waker = waker.clone();
            Arc::new(move || waker.wake())
        };
        let ytdlp = (!settings.ytdlp_path.trim().is_empty())
            .then(|| std::path::PathBuf::from(settings.ytdlp_path.trim()));
        crate::stream::clean_scratch(&paths.scratch());
        // Better Lyrics' key and its check's WebView2 profile.
        crate::api::betterlyrics::init(paths.cache.join("lyrics-check"));
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
        let audio_cache = crate::cache::AudioCache::open(paths.songs(), settings.cached_songs as usize);
        let mut player = Player::spawn(
            backend.runtime.handle().clone(),
            backend.http.clone(),
            events_tx,
            wake,
            settings.normalize,
            audio_cache.clone(),
        );
        player.set_volume(if settings.muted { 0.0 } else { settings.volume });
        player.set_eq(settings.eq, &settings.eq_gains);
        let (crossfade, gap) = settings.transitions();
        player.set_transitions(crossfade, gap);
        let ctx = egui::Context::default();
        let images = Images::new(
            ctx.clone(),
            backend.runtime.handle().clone(),
            backend.http.clone(),
            paths.images(),
        );
        let mut queue = Queue::default();
        queue.shuffle = settings.shuffle;
        let lastfm = lastfm::LastFm::new(&paths.config, backend.runtime.handle().clone(), backend.http.clone(), {
            let waker = waker.clone();
            Arc::new(move || waker.wake())
        });
        let network = net::Network::start({
            let waker = waker.clone();
            Arc::new(move || waker.wake())
        });
        let wake_with = |waker: &fastframe_shell::Waker| -> Arc<dyn Fn() + Send + Sync> {
            let waker = waker.clone();
            Arc::new(move || waker.wake())
        };
        let local = local::LocalLibrary::new(paths.cache.join("local"), wake_with(&waker));
        let updater = update::Updater::new(backend.runtime.handle().clone(), backend.http.clone(), wake_with(&waker));
        // A first look for an update a little after starting, when the
        // window is up and the music has started.
        let update_due = settings.check_updates.then(|| Instant::now() + Duration::from_secs(8));
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
            warm: HashMap::new(),
            prefetching: HashSet::new(),
            account_prewarm: None,
            upgrade: None,
            loaded: None,
            audio_cache,
            lyrics_cache: crate::lyrics::LyricsCache::new(paths.lyrics()),
            kept_pages: crate::kept_pages::KeptPages::new(paths.pages()),
            lyrics_retry: HashMap::new(),
            lyrics_translating: HashMap::new(),
            offline: HashMap::new(),
            offline_gen: 0,
            marks_dirty: true,
            downloads: Downloads::default(),
            download_done: std::sync::mpsc::channel(),
            network,
            was_online: true,
            stalled: false,
            history_sent: 0,
            js_runtime: None,
            storage: Arc::default(),
            cached_lists: None,
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
            rename_playlist: None,
            delete_playlist: None,
            toasts: Vec::new(),
            side_tab: SideTab::UpNext,
            full_view: false,
            full_tab: SideTab::UpNext,
            fullscreen: false,
            full_view_before: false,
            active_at: 0.0,
            local,
            local_hits: None,
            artists: HashMap::new(),
            artist_songs: None,
            art_queue: VecDeque::new(),
            art_flight: 0,
            art_generation: 0,
            song_search: None,
            radio_seed: None,
            picking_folder: false,
            folder_picked: std::sync::mpsc::channel(),
            updater,
            update_due,
            keep_session: false,
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
            settings_dirty: None,
            folder_watch: None,
            autostart: false,
            play_log: crate::stats::PlayLog::new(paths.plays()),
            stats: StatsView {
                source: crate::stats::Source::App,
                period: crate::stats::Period::Month,
                chosen: false,
                summaries: HashMap::new(),
            },
            stats_done: std::sync::mpsc::channel(),
            folders_changed: Arc::new(AtomicBool::new(false)),
            personal: settings::load(&paths.personal()),
            personal_dirty: None,
            window_styled: false,
            analyser: crate::visualiser::Analyser::default(),
            home_quick_left: 0,
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
        app.check_autostart();
        let folders = app.settings.local_folders.clone();
        app.local.scan(&folders);
        app.watch_folders();
        if app.settings.restore_session {
            app.restore_session();
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

    /// Home as it came: with Quick picks in it, that moves up to second.
    /// Without (YouTube Music sends it further down, in a later part),
    /// the one from last time stands in near the top, and the token of the
    /// next part comes back, to be fetched at once until it turns up.
    fn home_quick_picks(&mut self, page: &mut Page) -> Option<String> {
        self.home_quick_left = 0;
        if lift_quick_picks(page) {
            return None;
        }
        let last = self
            .pages
            .get("home")
            .and_then(|s| s.data.ready())
            .and_then(|p| p.sections.iter().find(|s| is_quick_picks(s)).cloned());
        if let Some(last) = last {
            page.sections.insert(1.min(page.sections.len()), last);
        }
        let token = page.continuation.clone()?;
        self.home_quick_left = 3;
        Some(token)
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
            born: Instant::now(),
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
        while let Ok((period, summary)) = self.stats_done.1.try_recv() {
            let written = self.play_log.written;
            self.stats.summaries.insert((crate::stats::Source::App, period), (Loadable::Ready(summary), Instant::now(), written));
        }
        if self.folders_changed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            log::info!("the folders of music changed: looking through them again");
            let folders = self.settings.local_folders.clone();
            self.local.scan(&folders);
        }
        if self.local.poll() {
            self.local_hits = None;
        }
        self.art_tick();
        while let Ok(picked) = self.folder_picked.1.try_recv() {
            self.picking_folder = false;
            if let Some(folder) = picked {
                self.add_local_folder(folder);
            }
        }
        self.update_tick();
        self.media_controls();
        self.tray_events();
        self.connectivity();
        self.download_tick();
        self.refresh_marks();
        self.maybe_preload();
        self.retry_lyrics();
        self.maybe_extend();
        self.upgrade_tick();
        self.presence_tick();
        self.scrobble_tick();
        self.ticking
            .store(self.playing(), std::sync::atomic::Ordering::Relaxed);
        self.toasts.retain(|t| t.until > Instant::now());
        if self.last_saved.elapsed() > Duration::from_secs(15) {
            self.save_session();
        }
        if self.settings_dirty.is_some_and(|at| at.elapsed() > Duration::from_millis(600)) {
            self.settings_dirty = None;
            self.save_settings();
        } else if self.settings_dirty.is_some() {
            crate::theme::repaint_in(&self.ctx, Duration::from_millis(650));
        }
        if self.personal_dirty.is_some_and(|at| at.elapsed() > Duration::from_secs(2)) {
            self.personal_dirty = None;
            settings::save(&self.paths.personal(), &self.personal);
        } else if self.personal_dirty.is_some() {
            crate::theme::repaint_in(&self.ctx, Duration::from_millis(2100));
        }
        let actions = std::mem::take(&mut self.actions);
        for action in actions {
            self.apply(action);
        }
    }

    /// A song's title as the settings would have it shown.
    pub fn shown_title<'t>(&self, track: &'t Track) -> &'t str {
        if self.settings.romanized_titles { track.romanized_title() } else { &track.title }
    }

    /// Whether the internet is there, as far as the app knows.
    pub fn online(&self) -> bool {
        self.network.online()
    }

    /// Whether yt-dlp will find a JavaScript runtime (Deno or Node.js).
    pub fn js_runtime_found(&mut self) -> bool {
        *self.js_runtime.get_or_insert_with(|| {
            ["deno.exe", "node.exe"].iter().any(|name| crate::stream::find_on_path(name).is_some())
        })
    }

    /// The downloaded songs and those kept from plays, for the Cached page:
    /// made again only when what is on disk changes.
    pub fn cached_lists(&mut self) -> (Arc<Vec<Track>>, Arc<Vec<Track>>) {
        let generation = self.audio_cache.generation();
        match &self.cached_lists {
            Some((at, saved, kept)) if *at == generation => (saved.clone(), kept.clone()),
            _ => {
                let saved = Arc::new(self.audio_cache.saved_tracks());
                let kept = Arc::new(self.audio_cache.kept_tracks());
                self.cached_lists = Some((generation, saved.clone(), kept.clone()));
                (saved, kept)
            }
        }
    }

    /// The stats chosen, unless loaded lately: the app's own on a thread
    /// (reading every play kept), Last.fm's from Last.fm.
    fn load_stats(&mut self, force: bool) {
        use crate::stats::Source;
        let (source, period) = (self.stats.source, self.stats.period);
        let fresh = self.stats.summaries.get(&(source, period)).is_some_and(|(loaded, at, written)| match (source, loaded) {
            (_, Loadable::Loading) => true,
            (Source::App, Loadable::Ready(_)) => *written == self.play_log.written && at.elapsed() < Duration::from_secs(600),
            (Source::LastFm, Loadable::Ready(_)) => at.elapsed() < Duration::from_secs(600),
            (_, Loadable::Failed(_)) => at.elapsed() < Duration::from_secs(15),
        });
        if fresh && !force {
            return;
        }
        match source {
            Source::App => {
                self.stats.summaries.insert((source, period), (Loadable::Loading, Instant::now(), self.play_log.written));
                let path = self.play_log.path().to_path_buf();
                let tx = self.stats_done.0.clone();
                let waker = self.waker.clone();
                let _ = std::thread::Builder::new().name("stats".into()).spawn(move || {
                    let plays = crate::stats::load(&path);
                    let summary = crate::stats::summarize(&plays, period, crate::stats::unix_now());
                    let _ = tx.send((period, summary));
                    waker.wake();
                });
            }
            Source::LastFm => match self.lastfm.reader().filter(|r| r.user().is_some()) {
                Some(reader) => {
                    self.stats.summaries.insert((source, period), (Loadable::Loading, Instant::now(), 0));
                    self.backend.send(Request::LastFmStats { period, reader });
                }
                None => {
                    let error = "Connect Last.fm in Settings, under Sharing, to see your stats from it.".to_owned();
                    self.stats.summaries.insert((source, period), (Loadable::Failed(error), Instant::now(), 0));
                }
            },
        }
    }

    /// Whether Windows starts the app at sign-in; when it does, that it
    /// starts this copy (the program was moved, say). Test profiles leave
    /// it alone.
    fn check_autostart(&mut self) {
        if settings::profile().is_some() {
            return;
        }
        let Some(command) = crate::platform::autostart() else {
            return;
        };
        self.autostart = true;
        if crate::platform::autostart_command().is_some_and(|ours| ours != command) {
            log::info!("starting with Windows: pointing at this copy of the app");
            crate::platform::set_autostart(true);
        }
    }

    /// Asks Windows to say when anything in the folders of music changes,
    /// in place of any watch before.
    fn watch_folders(&mut self) {
        self.folder_watch = None;
        let changed = self.folders_changed.clone();
        let waker = self.waker.clone();
        self.folder_watch = crate::watch::FolderWatch::start(
            &self.settings.local_folders,
            Arc::new(move || {
                changed.store(true, std::sync::atomic::Ordering::SeqCst);
                waker.wake();
            }),
        );
    }

    /// What a page depends on besides its address: the account (signed in
    /// or not), the language and the region.
    fn page_context(&self) -> String {
        format!("{}|{}|{}", if self.signed_in { "in" } else { "out" }, self.settings.language, self.settings.region.trim())
    }

    /// What the app keeps on disk, by kind: measured on a thread of its
    /// own at most every ten seconds, so None until first measured.
    pub fn storage(&mut self) -> Option<Storage> {
        let usage = self.audio_cache.usage();
        let mut measured = self.storage.lock();
        if measured.at.is_none_or(|at| at.elapsed() > Duration::from_secs(10)) && !measured.busy {
            measured.busy = true;
            let shared = self.storage.clone();
            let ctx = self.ctx.clone();
            let (images, local, lyrics, cache) = (self.paths.images(), self.paths.cache.join("local"), self.paths.lyrics(), self.paths.cache.clone());
            let other = [self.paths.logs(), self.paths.scratch(), self.paths.pages()];
            let spawned = std::thread::Builder::new().name("storage".into()).spawn(move || {
                let sizes = Storage {
                    artwork: crate::cache::dir_size(&images),
                    local: crate::cache::dir_size(&local),
                    lyrics: crate::cache::dir_size(&lyrics),
                    other: other.iter().map(|d| crate::cache::dir_size(d)).sum(),
                    free: crate::platform::free_space(&cache),
                    ..Storage::default()
                };
                let mut measured = shared.lock();
                *measured = Measured { sizes: Some(sizes), at: Some(Instant::now()), busy: false };
                ctx.request_repaint();
            });
            if spawned.is_err() {
                measured.busy = false;
            }
        }
        // The music on this PC: its files, and what the app keeps of them
        // (their pictures and the index).
        let music = self.local.bytes();
        measured.sizes.map(|sizes| Storage {
            downloads: usage.saved_bytes,
            recent: usage.kept_bytes,
            local: sizes.local + music,
            music,
            ..sizes
        })
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
        !self.stalled
            && self.current().is_some()
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
            Response::Artist { name, lastfm, youtube } => {
                if let Some(Err(error)) = &lastfm {
                    log::info!("Last.fm on {name}: {error}");
                }
                self.artists.insert(name.to_lowercase(), Loadable::Ready(ArtistPage { lastfm, youtube }));
            }
            Response::ArtFound { key, url } => {
                self.art_flight = self.art_flight.saturating_sub(1);
                // The queue's copies of the album's songs too.
                if let Some(url) = &url {
                    for entry in self.queue.entries.iter_mut() {
                        if entry.track.is_local() && entry.track.thumbnail.is_none() && local::art_key(&entry.track) == key {
                            entry.track.thumbnail = Some(url.clone());
                        }
                    }
                }
                self.local.art_found(&key, url);
                if self.art_flight == 0 && self.art_queue.is_empty() {
                    self.local.save_found();
                }
            }
            Response::SongFound { seq, track } if self.radio_seed == Some(seq) => {
                self.radio_seed = None;
                match track {
                    Some(seed) => {
                        log::info!("after the songs on this PC: the radio of {} by {} ({})", seed.title, seed.artist_line(), seed.video_id);
                        let seq = self.next_seq();
                        // The song just heard, again from YouTube Music, is
                        // left out; one like it (another artist's) stays in.
                        let heard = self.queue.entries.last().map(|e| e.track.title.clone()).unwrap_or_default();
                        let same = crate::local::loose(&seed.title).contains(&crate::local::loose(&heard));
                        let video_id = seed.video_id;
                        self.watch_purposes.insert(seq, WatchPurpose::RadioAfter { skip: same.then(|| video_id.clone()) });
                        self.queue.source.watch = true;
                        self.queue.source.playlist_id = Some(format!("RDAMVM{video_id}"));
                        self.backend.send(Request::Watch {
                            seq,
                            video_id: Some(video_id),
                            playlist_id: None,
                            params: None,
                        });
                    }
                    None => {
                        self.extending = false;
                        self.radio_dry = true;
                        if std::mem::take(&mut self.ended_waiting) {
                            self.player.stop();
                            self.want_play = false;
                            self.start_at = 0.0;
                        }
                    }
                }
            }
            Response::SongFound { seq, track } => {
                // Unless something else was played meanwhile.
                if self.song_search == Some(seq) && self.pending_queue == Some(seq) {
                    self.song_search = None;
                    self.pending_queue = None;
                    match track {
                        Some(track) => self.actions.push(Action::PlayTrack(track)),
                        None => self.toast("That song isn't on YouTube Music", ToastKind::Info),
                    }
                }
            }
            Response::Page { key, mut result } => {
                if let Err(error) = &result
                    && net::unreachable(error)
                {
                    self.network.check_now();
                }
                let home = key == "home";
                let mut find_quick = None;
                if home && let Ok(page) = &mut result {
                    find_quick = self.home_quick_picks(page);
                }
                if let Ok(page) = &result {
                    self.kept_pages.put(&key, &self.page_context(), page);
                }
                let state = self.pages.entry(key.clone()).or_insert_with(PageState::loading);
                // A page kept from last time stays when the fresh one fails.
                match (result, &state.data) {
                    (Ok(page), _) => state.data = Loadable::Ready(page),
                    (Err(_), Loadable::Ready(_)) if state.loading_more => {}
                    (Err(error), _) => state.data = Loadable::Failed(error),
                }
                state.loading_more = false;
                state.loading_tracks = false;
                if let Some(token) = find_quick {
                    state.loading_more = true;
                    self.backend.send(Request::More { key, token, tracks: false });
                }
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
                    let home = key == "home";
                    let mut found = false;
                    for section in sections {
                        // Quick picks, come at last: it takes the place of
                        // the one kept from last time, near the top.
                        match page.sections.iter().position(is_quick_picks).filter(|_| is_quick_picks(&section)) {
                            Some(at) if home => {
                                page.sections[at] = section;
                                found = true;
                            }
                            _ => {
                                found |= home && is_quick_picks(&section);
                                page.sections.push(section);
                            }
                        }
                    }
                    page.continuation = token;
                    if home {
                        lift_quick_picks(page);
                        if found {
                            self.home_quick_left = 0;
                            let page = page.clone();
                            self.kept_pages.put(&key, &self.page_context(), &page);
                        } else if self.home_quick_left > 0
                            && let Some(token) = page.continuation.clone()
                        {
                            // Not yet: the next part at once, a few times.
                            self.home_quick_left -= 1;
                            if let Some(state) = self.pages.get_mut(&key) {
                                state.loading_more = true;
                            }
                            self.backend.send(Request::More { key, token, tracks: false });
                        }
                    }
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
                        if net::unreachable(&error) {
                            self.network.check_now();
                        }
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
                let title = match self.watch_purposes.remove(&seq) {
                    Some(WatchPurpose::Start { title, .. }) => title,
                    Some(WatchPurpose::Offline { title }) => {
                        match result {
                            Ok(tracks) if !tracks.is_empty() => self.save_offline(tracks),
                            Ok(_) => self.toast(format!("{title} has no songs to download"), ToastKind::Info),
                            Err(error) => {
                                self.toast(format!("Could not get the songs of {title}: {error}"), ToastKind::Error)
                            }
                        }
                        return;
                    }
                    _ => return,
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
            Response::LastFmStats { period, result } => {
                let loaded = match result {
                    Ok(summary) => Loadable::Ready(summary),
                    Err(error) => Loadable::Failed(error),
                };
                self.stats.summaries.insert((crate::stats::Source::LastFm, period), (loaded, Instant::now(), 0));
            }
            Response::LyricsTranslated { video_id, lines, result } => {
                let Ok(done) = result else {
                    return;
                };
                self.lyrics_translating.remove(&video_id);
                if let Some(Loadable::Ready(Some(lyrics))) = self.lyrics.get_mut(&video_id) {
                    let lyrics = Arc::make_mut(lyrics);
                    if done.from.is_some() {
                        lyrics.translated.from = done.from;
                    }
                    // Every line asked is answered, if only with nothing
                    // (one in English already), so it is not asked again.
                    for (line, said) in lines.into_iter().zip(done.lines) {
                        lyrics.translated.lines.insert(line, said.unwrap_or_default());
                    }
                    self.lyrics_cache.update(&video_id, self.settings.synced_lyrics, lyrics);
                }
            }
            Response::Lyrics { video_id, partial: true, result, .. } => {
                // Shown while the rest is waited for: kept only once it comes.
                if let (Some(Loadable::Loading), Ok(lyrics)) = (self.lyrics.get(&video_id), result) {
                    self.lyrics.insert(video_id, Loadable::Ready(lyrics.map(Arc::new)));
                }
            }
            Response::Lyrics { video_id, result, complete, .. } => {
                // Kept together with what was found before: a look that
                // finds less (a source down) takes nothing away. A stopgap
                // is kept as provisional, so it shows at once next time too.
                let synced = self.settings.synced_lyrics;
                let kept = match &result {
                    Ok(found) => self.lyrics_cache.put(&video_id, synced, found, complete),
                    Err(_) => self.lyrics_cache.get(&video_id, synced).and_then(|k| k.lyrics),
                };
                let complete = complete && result.is_ok();
                if complete {
                    self.lyrics_retry.remove(&video_id);
                } else {
                    // Nothing to show meanwhile: asked again soon.
                    let wait = if kept.is_some() { LYRICS_RETRY } else { LYRICS_RETRY_SOON };
                    self.lyrics_retry.insert(video_id.clone(), Instant::now() + wait);
                }
                let shown = match (kept, result) {
                    (Some(lyrics), _) => Loadable::Ready(Some(Arc::new(lyrics))),
                    (None, Err(error)) => Loadable::Failed(error),
                    (None, Ok(_)) if !complete => Loadable::Failed("not every source answered".into()),
                    (None, Ok(_)) => Loadable::Ready(None),
                };
                self.lyrics.insert(video_id, shown);
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
            WatchPurpose::Extend | WatchPurpose::RadioAfter { .. } => {
                self.extending = false;
                let (after_local, skip) = match &purpose {
                    WatchPurpose::RadioAfter { skip } => (true, skip.clone()),
                    _ => (false, None),
                };
                // The song found for the one on this PC is that song again.
                let result = result.map(|mut watch| {
                    watch.tracks.retain(|t| Some(&t.video_id) != skip.as_ref());
                    watch
                });
                match result {
                    Ok(watch) => {
                        if after_local {
                            log::info!("radio: {} songs after the songs on this PC", watch.tracks.len());
                        }
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
            WatchPurpose::Offline { .. } => {}
            WatchPurpose::Lyrics { track } => {
                let browse_id = result.ok().and_then(|watch| watch.lyrics_id);
                let query = self.lyrics_query(&track);
                if query.is_none() && browse_id.is_none() {
                    self.lyrics.insert(track.video_id, Loadable::Ready(None));
                    return;
                }
                self.backend.send(Request::Lyrics {
                    video_id: track.video_id,
                    browse_id,
                    query,
                });
            }
            WatchPurpose::Info { video_id } => {
                if self.now.video_id != video_id {
                    return;
                }
                self.now.info_ready = true;
                if let Ok(watch) = result {
                    self.now.year = watch.tracks.iter().find(|t| t.video_id == video_id).and_then(|t| t.year);
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
        self.upgrade = None;
        self.loaded = None;
        self.stalled = false;
        // A song on this PC plays from its file, online or not.
        if entry.track.is_local() {
            match self.local.stream_for(&entry.track) {
                Some(stream) => {
                    self.loaded = Some((token, stream.clone()));
                    self.player.load(token, stream, Duration::from_secs_f64(start), true);
                    self.track_info(&video_id);
                }
                None => self.cannot_play(&video_id, "its file is no longer there"),
            }
            return;
        }
        // Its names go with the file, should it be kept.
        self.audio_cache.note(&entry.track);
        if !self.online() && !self.audio_cache.has_at_least(&video_id, 1) && !self.network.check_now() {
            self.track_info(&video_id);
            let title = self.shown_title(&entry.track).to_owned();
            self.offline_skip(&title);
            return;
        }
        if let Some(stream) = self.best_stream(&video_id) {
            self.loaded = Some((token, stream.clone()));
            self.player.load(token, stream, Duration::from_secs_f64(start), true);
        } else if let Some(stream) = self.audio_cache.lookup(&video_id) {
            // Kept at standard quality: it plays at once from disk, and the
            // Premium stream takes over when it comes, as on a quick start.
            self.loaded = Some((token, stream.clone()));
            self.player.load(token, stream.clone(), Duration::from_secs_f64(start), true);
            if self.account_streams() {
                self.upgrade = Some(Upgrade {
                    token,
                    phase: UpgradePhase::Finding,
                    from: Some(stream.quality.clone()),
                    to: None,
                    quick_failed: false,
                });
                self.backend.send(Request::Resolve {
                    token,
                    video_id: video_id.clone(),
                    purpose: Purpose::Upgrade,
                    start,
                    how: How::Account,
                });
            }
        } else if self.account_streams()
            && let Some(stream) = self.quick_stream(&video_id)
        {
            // Found a moment ago, when it was pointed at: it starts at once,
            // and the account's stream takes over when it comes.
            self.loaded = Some((token, stream.clone()));
            self.player.load(token, stream.clone(), Duration::from_secs_f64(start), true);
            self.upgrade = Some(Upgrade {
                token,
                phase: UpgradePhase::Finding,
                from: Some(stream.quality.clone()),
                to: None,
                quick_failed: false,
            });
            self.backend.send(Request::Resolve {
                token,
                video_id: video_id.clone(),
                purpose: Purpose::Upgrade,
                start,
                how: How::Account,
            });
        } else if self.account_streams() {
            // The account's stream takes seconds through yt-dlp: start on
            // the quick one and switch when the better one arrives.
            self.upgrade = Some(Upgrade {
                token,
                phase: UpgradePhase::Finding,
                from: None,
                to: None,
                quick_failed: false,
            });
            for (purpose, how) in [(Purpose::Load, How::Native), (Purpose::Upgrade, How::Account)] {
                self.backend.send(Request::Resolve {
                    token,
                    video_id: video_id.clone(),
                    purpose,
                    start,
                    how,
                });
            }
        } else {
            self.backend.send(Request::Resolve {
                token,
                video_id: video_id.clone(),
                purpose: Purpose::Load,
                start,
                how: How::Full,
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
        // YouTube knows nothing of a song on this PC; its lyrics come from
        // LRCLIB alone.
        if local::is_local_id(video_id) {
            self.now.info_ready = true;
            if self.lyrics_shown() {
                self.request_lyrics();
            }
            return;
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
    /// YouTube has them: timed ones from Better Lyrics' sources and LRCLIB
    /// first, then YouTube's.
    pub fn request_lyrics(&mut self) {
        self.ask_lyrics(false);
    }

    /// `again`: the sources asked once more, whatever is on disk or on
    /// screen (which stays until the answer comes).
    fn ask_lyrics(&mut self, again: bool) {
        let Some(track) = self.current().cloned() else {
            return;
        };
        // Found before: from disk, at once and offline too.
        if !self.lyrics.contains_key(&track.video_id)
            && let Some(kept) = self.lyrics_cache.get(&track.video_id, self.settings.synced_lyrics)
        {
            // Provisional ones show, and the sources are asked again meanwhile.
            if kept.recheck {
                self.lyrics_retry.insert(track.video_id.clone(), Instant::now());
            }
            self.lyrics.insert(track.video_id.clone(), Loadable::Ready(kept.lyrics.map(Arc::new)));
        }
        // A restored song that has not played yet has no info yet.
        if self.now.video_id != track.video_id {
            self.track_info(&track.video_id);
            return;
        }
        let video_id = track.video_id.clone();
        let asked = match self.lyrics.get(&video_id) {
            None => false,
            Some(Loadable::Loading) => true,
            Some(_) => !again,
        };
        if asked || !self.now.info_ready {
            return;
        }
        let query = self.lyrics_query(&track);
        let browse_id = self.now.lyrics_id.clone();
        if query.is_none() && browse_id.is_none() {
            self.lyrics.insert(video_id, Loadable::Ready(None));
            return;
        }
        if !matches!(self.lyrics.get(&video_id), Some(Loadable::Ready(Some(_)))) {
            self.lyrics.insert(video_id.clone(), Loadable::Loading);
        }
        self.backend.send(Request::Lyrics {
            video_id,
            browse_id,
            query,
        });
    }

    /// What LRCLIB is asked for a song's synced lyrics, when they are wanted.
    fn lyrics_query(&self, track: &Track) -> Option<crate::api::lrclib::Query> {
        track
            .artists
            .first()
            .filter(|_| self.settings.synced_lyrics)
            .map(|artist| crate::api::lrclib::Query {
                title: track.title.clone(),
                artist: artist.name.clone(),
                album: track.album.as_ref().map(|a| a.name.clone()),
                duration: track.duration,
            })
    }

    /// The next song's lyrics, looked up while this one plays, so they are
    /// there the moment it starts (LRCLIB can be slow to answer when it is
    /// busy). Only while lyrics are on screen.
    fn prefetch_lyrics(&mut self, track: &Track) {
        if !self.lyrics_shown() || !self.online() || self.lyrics.contains_key(&track.video_id) {
            return;
        }
        if let Some(kept) = self.lyrics_cache.get(&track.video_id, self.settings.synced_lyrics) {
            if kept.recheck {
                self.lyrics_retry.insert(track.video_id.clone(), Instant::now());
            }
            self.lyrics.insert(track.video_id.clone(), Loadable::Ready(kept.lyrics.map(Arc::new)));
            return;
        }
        self.lyrics.insert(track.video_id.clone(), Loadable::Loading);
        log::info!("looking up the lyrics of the next song, {}, ahead of it", track.title);
        if track.is_local() {
            // YouTube knows nothing of a song on this PC: LRCLIB alone.
            match self.lyrics_query(track) {
                Some(query) => self.backend.send(Request::Lyrics {
                    video_id: track.video_id.clone(),
                    browse_id: None,
                    query: Some(query),
                }),
                None => {
                    self.lyrics.insert(track.video_id.clone(), Loadable::Ready(None));
                }
            }
            return;
        }
        // YouTube's own lyrics are named in the song's info: that first.
        let seq = self.next_seq();
        self.watch_purposes.insert(seq, WatchPurpose::Lyrics { track: track.clone() });
        self.backend.send(Request::Watch {
            seq,
            video_id: Some(track.video_id.clone()),
            playlist_id: None,
            params: None,
        });
    }

    /// The lyrics on screen came back incomplete a while ago (LRCLIB out of
    /// reach, or limiting requests): ask again.
    fn retry_lyrics(&mut self) {
        let Some(video_id) = self.current().map(|t| t.video_id.clone()) else {
            return;
        };
        if self.lyrics_retry.get(&video_id).is_some_and(|at| Instant::now() >= *at) && self.lyrics_shown() && self.online() {
            self.lyrics_retry.remove(&video_id);
            // What is shown stays until the new answer comes.
            self.ask_lyrics(true);
        }
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
        if let Some((token, stream)) = &self.loaded
            && *token == self.current_token
        {
            return Some(stream.quality.as_str());
        }
        let track = self.current()?;
        self.streams
            .get(&track.video_id)
            .map(|(stream, _)| stream.quality.as_str())
    }

    pub fn current_token(&self) -> u64 {
        self.current_token
    }

    /// The entry that plays after this one, by its uid, with the token it
    /// was preloaded under once it has been: the card waiting behind the
    /// artwork in the full player.
    pub fn coming_up(&self) -> Option<(u64, Option<u64>, &Track)> {
        if let Some((token, uid)) = self.preload
            && let Some(index) = self.queue.index_of(uid)
        {
            return Some((uid, Some(token), &self.queue.entries[index].track));
        }
        let next = self.queue.next_index(self.settings.repeat, false).filter(|&i| Some(i) != self.queue.current)?;
        let entry = self.queue.entries.get(next)?;
        Some((entry.uid, None, &entry.track))
    }

    /// A switch to the better stream is heard once playback reaches it.
    fn upgrade_tick(&mut self) {
        let position = self.position();
        if let Some(upgrade) = &mut self.upgrade
            && let UpgradePhase::Switching(at) = upgrade.phase
            && upgrade.token == self.current_token
            && position >= at
        {
            upgrade.phase = UpgradePhase::Done(Instant::now());
        }
    }

    /// Any stream of the song found within the hour, whatever its quality:
    /// one to start on while a better one comes.
    fn quick_stream(&self, video_id: &str) -> Option<StreamInfo> {
        self.streams
            .get(video_id)
            .or_else(|| self.warm.get(video_id))
            .filter(|(_, at)| at.elapsed() < STREAM_FRESH)
            .map(|(stream, _)| stream.clone())
    }

    /// Finds a song's stream while the pointer rests on it, so a click
    /// starts it at once: the quick, signed-out stream, in a fraction of a
    /// second. A song only Premium members can play has none of those, and
    /// comes through yt-dlp, in seconds: that starts now too, one at a time.
    fn prewarm(&mut self, track: &Track) {
        let video_id = &track.video_id;
        if track.is_local()
            || !self.online()
            || self.current().is_some_and(|t| &t.video_id == video_id)
            || self.quick_stream(video_id).is_some()
            || self.audio_cache.has_at_least(video_id, 1)
            || self.prefetching.contains(video_id)
            || self.prefetching.len() >= 4
        {
            return;
        }
        self.prefetching.insert(video_id.clone());
        self.backend.send(Request::Resolve {
            token: 0,
            video_id: video_id.clone(),
            purpose: Purpose::Prewarm,
            start: 0.0,
            how: How::Native,
        });
    }

    /// The song's stream at the quality wanted, if it is at hand: kept on
    /// disk, or resolved within the hour.
    fn best_stream(&mut self, video_id: &str) -> Option<StreamInfo> {
        let account = self.account_streams();
        let wanted = if account { PREMIUM_KBPS } else { 1 };
        if self.audio_cache.has_at_least(video_id, wanted) {
            return self.audio_cache.lookup(video_id);
        }
        // One found on pointing is the best there is only without the
        // account's streams.
        self.streams
            .get(video_id)
            .or_else(|| if account { None } else { self.warm.get(video_id) })
            .filter(|(_, at)| at.elapsed() < STREAM_FRESH)
            .map(|(stream, _)| stream.clone())
    }

    /// Streams come from the account (through yt-dlp, in seconds).
    fn account_streams(&self) -> bool {
        self.settings.premium_quality && self.signed_in && self.backend.ytdlp_found().is_some()
    }

    /// Resolves the song after this one ahead of time, when resolving is
    /// slow (the account's streams come through yt-dlp, in seconds).
    fn prefetch_next(&mut self) {
        if !(self.settings.premium_quality && self.signed_in) {
            return;
        }
        // One yt-dlp at a time: the song playing gets its better stream
        // first, then the next is fetched (two at once slow both).
        if self.upgrade.as_ref().is_some_and(|u| u.phase == UpgradePhase::Finding) {
            return;
        }
        let Some(entry) = self
            .queue
            .next_index(self.settings.repeat, false)
            .and_then(|i| self.queue.entries.get(i))
            .filter(|e| !e.track.is_local())
        else {
            return;
        };
        let video_id = entry.track.video_id.clone();
        let fresh = self
            .streams
            .get(&video_id)
            .is_some_and(|(_, at)| at.elapsed() < STREAM_FRESH)
            || self.audio_cache.has_at_least(&video_id, PREMIUM_KBPS);
        if fresh || self.prefetching.contains(&video_id) || Some(video_id.as_str()) == self.current().map(|t| t.video_id.as_str()) {
            return;
        }
        self.prefetching.insert(video_id.clone());
        self.backend.send(Request::Resolve {
            token: 0,
            video_id,
            purpose: Purpose::Prefetch,
            start: 0.0,
            how: How::Full,
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
        // Tagged with its song: once wholly downloaded, the file is kept.
        let result = result.map(|(mut stream, via)| {
            stream.cache_key = Some(video_id.clone());
            (stream, via)
        });
        match (purpose, result) {
            (Purpose::Prefetch, Ok((stream, _))) => {
                self.streams.insert(video_id, (stream, Instant::now()));
            }
            (Purpose::Prefetch, Err(error)) => log::info!("prefetch of {video_id} failed: {error}"),
            (Purpose::Prewarm, Ok((stream, _))) => {
                self.prefetching.remove(&video_id);
                self.warm.retain(|_, (_, at)| at.elapsed() < STREAM_FRESH);
                self.warm.insert(video_id, (stream, Instant::now()));
            }
            (Purpose::Prewarm, Err(error)) => {
                self.prefetching.remove(&video_id);
                // For Premium members only: through yt-dlp with the account,
                // which takes seconds, so it starts now. One at a time.
                let busy = self.prefetching.iter().any(|id| self.account_prewarm.as_deref() == Some(id.as_str()));
                if error.contains("Premium") && self.signed_in && self.backend.ytdlp_found().is_some() && !busy {
                    log::info!("{video_id} is for Premium members: finding it through yt-dlp ahead of a click");
                    self.account_prewarm = Some(video_id.clone());
                    self.prefetching.insert(video_id.clone());
                    self.backend.send(Request::Resolve {
                        token: 0,
                        video_id,
                        purpose: Purpose::Prefetch,
                        start: 0.0,
                        how: How::Account,
                    });
                }
            }
            (Purpose::Load, Ok((stream, _))) => {
                let quick = self.upgrade.as_ref().is_some_and(|u| u.token == token);
                // A quick start's stream is not kept: played again, the song
                // should get the better one.
                if !quick {
                    self.streams.insert(video_id, (stream.clone(), Instant::now()));
                }
                if token == self.current_token && self.loaded.as_ref().is_none_or(|(t, _)| *t != token) {
                    if let Some(upgrade) = &mut self.upgrade
                        && upgrade.token == token
                    {
                        upgrade.from = Some(stream.quality.clone());
                    }
                    self.loaded = Some((token, stream.clone()));
                    self.player.load(
                        token,
                        stream,
                        Duration::from_secs_f64(start),
                        self.want_play,
                    );
                }
            }
            (Purpose::Upgrade, Ok((stream, _))) => {
                self.streams.insert(video_id, (stream.clone(), Instant::now()));
                if token != self.current_token {
                    return;
                }
                let Some(upgrade) = self.upgrade.as_mut().filter(|u| u.token == token) else {
                    return;
                };
                upgrade.to = Some(stream.quality.clone());
                // No longer finding: the next song may be fetched now.
                upgrade.phase = UpgradePhase::Switching(f64::INFINITY);
                match &self.loaded {
                    // Playing the quick stream: switch, if the two are
                    // alike enough to join without a jump.
                    Some((t, quick)) if *t == token => {
                        if quick.mime == stream.mime && quick.quality != stream.quality {
                            self.player.upgrade(token, stream);
                        } else {
                            upgrade.phase = UpgradePhase::Unavailable;
                        }
                    }
                    // Nothing playing yet: this one starts it.
                    _ => {
                        upgrade.phase = UpgradePhase::Done(Instant::now());
                        upgrade.from = None;
                        let at = Duration::from_secs_f64(self.start_at);
                        self.loaded = Some((token, stream.clone()));
                        self.player.load(token, stream, at, self.want_play);
                    }
                }
                self.prefetch_next();
            }
            (Purpose::Upgrade, Err(error)) => {
                log::info!("no better stream for {video_id}: {error}");
                let Some(upgrade) = self.upgrade.as_mut().filter(|u| u.token == token) else {
                    return;
                };
                upgrade.phase = UpgradePhase::Unavailable;
                if upgrade.quick_failed && token == self.current_token {
                    self.cannot_play(&video_id, &error);
                } else {
                    self.prefetch_next();
                }
            }
            (Purpose::Preload, Ok((stream, _))) => {
                self.streams
                    .insert(video_id, (stream.clone(), Instant::now()));
                if let Some((t, uid)) = self.preload
                    && t == token
                {
                    let crossfade = self.queue.index_of(uid).is_some_and(|i| self.crossfade_into(&self.queue.entries[i].track));
                    self.player.preload(token, stream, crossfade);
                }
            }
            (Purpose::Load, Err(error)) => {
                if token != self.current_token {
                    return;
                }
                // The quick stream failed; the account's may still come.
                if let Some(upgrade) = self.upgrade.as_mut().filter(|u| u.token == token)
                    && upgrade.phase == UpgradePhase::Finding
                {
                    upgrade.quick_failed = true;
                    return;
                }
                self.cannot_play(&video_id, &error);
            }
            (Purpose::Preload, Err(error)) => {
                log::info!("preload of {video_id} failed: {error}");
                if self.preload.is_some_and(|(t, _)| t == token) {
                    self.preload = None;
                }
            }
            (Purpose::Download, Ok((stream, _))) => {
                let Some(active) = self.downloads.active.as_mut().filter(|d| d.token == token) else {
                    return;
                };
                let (cache, track, stream_kept) = (self.audio_cache.clone(), active.track.clone(), stream.clone());
                let (tx, waker) = (self.download_done.0.clone(), self.waker.clone());
                let complete: crate::player::source::Complete = Box::new(move |bytes| {
                    let id = track.video_id.clone();
                    cache.save_offline(track, stream_kept, bytes, move |ok| {
                        let _ = tx.send((id, ok));
                        waker.wake();
                    });
                });
                active.source = Some(crate::player::source::RemoteSource::open(
                    self.backend.runtime.handle(),
                    self.backend.http.clone(),
                    stream.url.clone(),
                    stream.header_map(),
                    stream.content_length,
                    Some(complete),
                ));
            }
            (Purpose::Download, Err(error)) => {
                if self.downloads.active.as_ref().is_some_and(|d| d.token == token) {
                    self.download_failed(&error);
                }
            }
        }
    }

    /// The song started: into the account's history, as on the web.
    fn report_played(&mut self) {
        if !(self.signed_in && self.settings.history) || self.history_sent == self.current_token {
            return;
        }
        self.history_sent = self.current_token;
        if let Some(video_id) = self.current().filter(|t| !t.is_local()).map(|t| t.video_id.clone()) {
            self.backend.send(Request::History { video_id });
        }
    }

    // ---- Offline ------------------------------------------------------------

    /// Notices the connection going and coming back.
    fn connectivity(&mut self) {
        let online = self.online();
        if online == self.was_online {
            return;
        }
        self.was_online = online;
        if !online {
            self.toast("You're offline. Downloaded songs still play.", ToastKind::Info);
            return;
        }
        self.toast("Back online", ToastKind::Info);
        self.lastfm.forget_unanswered();
        // What could not load, now can.
        let route = self.route.clone();
        let failed = match &route {
            Route::Search { .. } => matches!(self.search.results, Loadable::Failed(_)),
            _ => self.pages.get(&route.key()).is_some_and(|p| matches!(p.data, Loadable::Failed(_))),
        };
        if failed {
            self.request_page(&route, true);
        }
        if self.signed_in && self.account.is_none() {
            self.backend.send(Request::Account);
            self.backend.send(Request::LibraryPlaylists);
        }
        if std::mem::take(&mut self.stalled) && self.current().is_some() {
            let at = self.start_at;
            self.start_current(at);
        }
    }

    /// The quality a download should have: Premium's when the account
    /// streams it, else any.
    fn wanted_kbps(&self) -> u32 {
        if self.account_streams() { PREMIUM_KBPS } else { 1 }
    }

    fn save_offline(&mut self, tracks: Vec<Track>) {
        let wanted = self.wanted_kbps();
        let (mut queued, mut at_once) = (0, 0);
        // Songs on this PC are here already.
        for track in tracks.into_iter().filter(|t| !t.is_local()) {
            let id = track.video_id.clone();
            if self.offline.get(&id) == Some(&Mark::Saved) || self.downloads.has(&id) {
                continue;
            }
            // On disk already, as good as a download would be.
            if self.audio_cache.has_at_least(&id, wanted) && self.audio_cache.pin(&track) {
                at_once += 1;
                continue;
            }
            self.downloads.waiting.push_back(track);
            queued += 1;
        }
        self.marks_dirty = true;
        let total = queued + at_once;
        if total == 0 {
            self.toast("Already downloaded", ToastKind::Info);
        } else if queued == 0 {
            self.toast(
                if total == 1 { "Downloaded for offline".to_owned() } else { format!("{total} songs downloaded for offline") },
                ToastKind::Info,
            );
        } else if !self.online() {
            self.toast("You're offline: the downloads start when the connection is back", ToastKind::Info);
        } else {
            self.toast(
                if total == 1 { "Downloading for offline".to_owned() } else { format!("Downloading {total} songs for offline") },
                ToastKind::Info,
            );
        }
    }

    /// Takes in finished downloads, follows the one under way, and starts
    /// the next.
    fn download_tick(&mut self) {
        while let Ok((video_id, ok)) = self.download_done.1.try_recv() {
            if self.downloads.active.as_ref().is_some_and(|d| d.track.video_id == video_id) {
                self.downloads.active = None;
            }
            if ok {
                self.downloads.done += 1;
            } else {
                self.downloads.failed += 1;
            }
            self.download_round_end();
        }
        if let Some(active) = &mut self.downloads.active
            && let Some((_, source)) = &active.source
        {
            if let Some(error) = source.error() {
                let message = match error {
                    crate::player::source::SourceError::Forbidden => "the stream was refused".to_owned(),
                    crate::player::source::SourceError::Other(message) => message,
                };
                self.download_failed(&message);
            } else {
                let have: u64 = source.downloaded().iter().map(|span| span.end - span.start).sum();
                active.progress = (have as f32 / source.len().max(1) as f32).clamp(0.0, 1.0);
                crate::theme::repaint_in(&self.ctx, Duration::from_millis(250));
            }
        }
        // One yt-dlp at a time: the song playing gets its better stream first.
        let upgrading = self.upgrade.as_ref().is_some_and(|u| u.phase == UpgradePhase::Finding);
        if self.downloads.active.is_none() && self.online() && !upgrading {
            while let Some(track) = self.downloads.waiting.pop_front() {
                // Played meanwhile, and kept as good as a download would be.
                if self.audio_cache.has_at_least(&track.video_id, self.wanted_kbps()) && self.audio_cache.pin(&track) {
                    self.downloads.done += 1;
                    continue;
                }
                let token = self.next_seq();
                self.backend.send(Request::Resolve {
                    token,
                    video_id: track.video_id.clone(),
                    purpose: Purpose::Download,
                    start: 0.0,
                    how: How::Full,
                });
                self.downloads.active = Some(Download { track, token, source: None, progress: 0.0 });
                break;
            }
            self.download_round_end();
        }
    }

    fn download_failed(&mut self, error: &str) {
        let Some(active) = self.downloads.active.take() else {
            return;
        };
        if net::unreachable(error) && !self.network.check_now() {
            // Offline: it waits for the connection, first in line.
            self.downloads.waiting.push_front(active.track);
            return;
        }
        log::warn!("cannot download {}: {error}", active.track.video_id);
        self.downloads.failed += 1;
        self.download_round_end();
    }

    /// Once the last download of a round is done, says how it went.
    fn download_round_end(&mut self) {
        self.marks_dirty = true;
        if self.downloads.busy() {
            return;
        }
        let (done, failed) = (std::mem::take(&mut self.downloads.done), std::mem::take(&mut self.downloads.failed));
        let songs = |n: usize| if n == 1 { "1 song".to_owned() } else { format!("{n} songs") };
        match (done, failed) {
            (0, 0) => {}
            (done, 0) => self.toast(format!("Downloaded {} for offline", songs(done)), ToastKind::Info),
            (0, failed) => self.toast(format!("Could not download {}", songs(failed)), ToastKind::Error),
            (done, failed) => self.toast(
                format!("Downloaded {} for offline; {} could not be downloaded", songs(done), songs(failed)),
                ToastKind::Error,
            ),
        }
    }

    /// The marks beside songs' names: what is on disk, and what is
    /// downloading. Rebuilt only when either changes.
    fn refresh_marks(&mut self) {
        let generation = self.audio_cache.generation();
        let busy = self.downloads.busy();
        if generation == self.offline_gen && !busy && !self.marks_dirty {
            return;
        }
        self.offline_gen = generation;
        self.marks_dirty = busy;
        let mut marks = self.audio_cache.marks();
        for track in &self.downloads.waiting {
            marks.insert(track.video_id.clone(), Mark::Waiting);
        }
        if let Some(active) = &self.downloads.active {
            marks.insert(active.track.video_id.clone(), Mark::Saving(active.progress));
        }
        self.offline = marks;
    }

    /// Offline, and the song is not on this PC: on to the next song in the
    /// queue that is, or else wait for the connection.
    fn offline_skip(&mut self, title: &str) {
        let next = self.queue.current.and_then(|current| {
            (current + 1..self.queue.entries.len()).find(|&i| {
                let track = &self.queue.entries[i].track;
                track.is_local() || self.audio_cache.has_at_least(&track.video_id, 1)
            })
        });
        match next {
            Some(index) => {
                self.toast(
                    format!("You're offline, and \u{201c}{title}\u{201d} isn't downloaded: on to the next song on this PC"),
                    ToastKind::Info,
                );
                self.queue.current = Some(index);
                self.start_current(0.0);
            }
            None => {
                self.player.stop();
                self.stalled = true;
                self.toast(
                    format!("You're offline. \u{201c}{title}\u{201d} plays when the connection is back; downloaded songs play now (Library, Cached)"),
                    ToastKind::Info,
                );
            }
        }
    }

    /// The current song cannot play: say why, and go on to the next one
    /// (a few times at most).
    fn cannot_play(&mut self, video_id: &str, error: &str) {
        let title = self.current().map(|t| self.shown_title(t).to_owned()).unwrap_or_default();
        log::warn!("cannot play {video_id}: {error}");
        if net::unreachable(error) && !self.network.check_now() {
            self.offline_skip(&title);
            return;
        }
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

    fn on_player(&mut self, event: player::Event) {
        match event {
            player::Event::Audible(token) => {
                if token == self.current_token {
                    self.failures = 0;
                    self.retried = None;
                    self.start_at = 0.0;
                    self.report_played();
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
                    self.report_played();
                    self.prefetch_next();
                }
            }
            player::Event::Ended(token) => {
                if token == self.current_token {
                    self.advance(false);
                }
            }
            player::Event::Upgraded { token, at } => {
                if let Some(upgrade) = self.upgrade.as_mut().filter(|u| u.token == token) {
                    upgrade.phase = UpgradePhase::Switching(at.as_secs_f64());
                }
                // The better stream is the one cached for the song now.
                let better = self
                    .current()
                    .and_then(|track| self.streams.get(&track.video_id))
                    .map(|(stream, _)| stream.clone());
                if let (Some((t, loaded)), Some(stream)) = (self.loaded.as_mut(), better)
                    && *t == token
                {
                    *loaded = stream;
                }
            }
            player::Event::UpgradeFailed(token) => {
                if let Some(upgrade) = self.upgrade.as_mut().filter(|u| u.token == token) {
                    upgrade.phase = UpgradePhase::Unavailable;
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
                self.warm.remove(&video_id);
                // A kept file that would not play is dropped, and the song
                // found again.
                let from_disk = self
                    .loaded
                    .as_ref()
                    .is_some_and(|(t, s)| *t == token && crate::cache::file_of(&s.url).is_some());
                if from_disk {
                    self.audio_cache.remove(&video_id);
                }
                if (expired || from_disk) && self.retried != Some(token) {
                    // The address ran out; find it again and carry on from
                    // the same place.
                    self.retried = Some(token);
                    self.loaded = None;
                    self.upgrade = None;
                    self.backend.send(Request::Resolve {
                        token,
                        video_id,
                        purpose: Purpose::Load,
                        start: position.as_secs_f64(),
                        how: How::Full,
                    });
                    return;
                }
                if net::unreachable(&message) && !self.network.check_now() {
                    // The connection went mid-song: it carries on from here
                    // once it is back.
                    self.stalled = true;
                    self.start_at = position.as_secs_f64();
                    self.loaded = None;
                    self.upgrade = None;
                    self.toast("Connection lost. The song carries on when you're back online.", ToastKind::Info);
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
        // Early enough that the smart crossfade finds its start ready to
        // look at, beside the last seconds of this one.
        let ahead = Duration::from_secs(20) + if self.settings.crossfade { crate::player::smart::SCAN } else { Duration::ZERO };
        if left > ahead {
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
        self.prefetch_lyrics(&entry.track);
        let token = self.next_seq();
        self.preload = Some((token, entry.uid));
        let stream = if entry.track.is_local() {
            self.local.stream_for(&entry.track)
        } else {
            self.best_stream(&entry.track.video_id)
                .or_else(|| self.audio_cache.lookup(&entry.track.video_id))
        };
        if let Some(stream) = stream {
            let crossfade = self.crossfade_into(&entry.track);
            self.player.preload(token, stream, crossfade);
        } else if entry.track.is_local() {
            // Gone from disk: it fails in its turn, and is skipped then.
            self.preload = None;
        } else {
            self.backend.send(Request::Resolve {
                token,
                video_id: entry.track.video_id,
                purpose: Purpose::Preload,
                start: 0.0,
                how: How::Full,
            });
        }
    }

    /// Whether `next` may fade in over the song playing: not when they are
    /// one album's, which plays as one piece.
    fn crossfade_into(&self, next: &Track) -> bool {
        // A song on this PC has no album id: its album's name and folder
        // stand in for one.
        let album = |t: &Track| match &t.file {
            Some(file) => t.album.as_ref().map(|a| format!("{}|{}", a.name, std::path::Path::new(file).parent().map(|p| p.display().to_string()).unwrap_or_default())),
            None => t.album.as_ref().and_then(|a| a.id.clone()),
        };
        match (self.current().and_then(album), album(next)) {
            (Some(a), Some(b)) => a != b,
            _ => true,
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
                // The list ran out: carry on with the radio of its last song
                // from YouTube Music; for a song on this PC, of the same song
                // found there.
                let Some(last) = self.queue.entries.iter().rev().find(|e| !e.track.is_local()).map(|e| e.track.video_id.clone()) else {
                    let Some(local) = self.queue.entries.last().map(|e| e.track.clone()) else {
                        return;
                    };
                    let Some(artist) = local.artists.first().map(|a| a.name.clone()) else {
                        self.radio_dry = true;
                        return;
                    };
                    self.extending = true;
                    let seq = self.next_seq();
                    self.radio_seed = Some(seq);
                    self.backend.send(Request::RadioSeed {
                        seq,
                        artist,
                        title: local.romanized_title().to_owned(),
                        lastfm: self.lastfm.reader(),
                    });
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
                if self.stalled {
                    // Waiting for the connection: try again now.
                    let at = self.start_at;
                    self.start_current(at);
                } else if self.current().is_some() {
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

    /// The queue as the last session left it, at the same moment.
    fn restore_session(&mut self) {
        let session: Session = settings::load(&self.paths.session());
        if !session.tracks.is_empty() {
            let current = session.current.unwrap_or(0);
            self.queue.replace(
                session.tracks,
                current,
                Source {
                    playlist_id: session.playlist_id,
                    ..Source::default()
                },
            );
            self.start_at = session.position_secs;
            self.current_uid = self.queue.current().map(|e| e.uid);
        }
    }

    /// Started by an update of the app: the queue carries on where the
    /// old program left it, playing if it was.
    pub fn resume_after_update(&mut self, playing: bool) {
        if !self.settings.restore_session {
            self.restore_session();
            // Written for this start alone.
            let _ = std::fs::remove_file(self.paths.session());
        }
        if playing && self.current().is_some() {
            self.actions.push(Action::TogglePlay);
        }
        self.toast(format!("Updated to {}", env!("CARGO_PKG_VERSION")), ToastKind::Info);
    }

    pub fn save_session(&mut self) {
        self.last_saved = Instant::now();
        if !self.settings.restore_session && !self.keep_session {
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

    /// Back from full screen to the window as it was.
    fn leave_fullscreen(&mut self) {
        if !self.fullscreen {
            return;
        }
        self.fullscreen = false;
        self.full_view = self.full_view_before;
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }
}

// ---- Music on this PC, and updates -------------------------------------------

impl App {
    /// A search's songs from the person's folders, found once per query.
    pub fn local_hits(&mut self, query: &str) -> Arc<Vec<Track>> {
        let generation = self.local.generation();
        match &self.local_hits {
            Some((q, g, hits)) if q == query && *g == generation => hits.clone(),
            _ => {
                let hits = Arc::new(self.local.search(query));
                self.local_hits = Some((query.to_owned(), generation, hits.clone()));
                hits
            }
        }
    }

    /// Looks online for pictures of the songs on this PC that have none,
    /// two at a time, once the library has changed.
    fn art_tick(&mut self) {
        if self.local.generation() != self.art_generation {
            self.art_generation = self.local.generation();
            let wanted = self.local.art_wanted();
            self.art_queue.extend(wanted);
        }
        while self.art_flight < 2 && self.online() {
            let Some(query) = self.art_queue.pop_front() else { break };
            self.art_flight += 1;
            self.backend.send(Request::FindArt { query, lastfm: self.lastfm.reader() });
        }
    }

    /// An artist's songs on this PC, found once per change of the library.
    pub fn artist_songs(&mut self, name: &str) -> Arc<Vec<Track>> {
        let (key, generation) = (name.to_lowercase(), self.local.generation());
        match &self.artist_songs {
            Some((k, g, songs)) if *k == key && *g == generation => songs.clone(),
            _ => {
                let songs = Arc::new(self.local.by_artist(name));
                self.artist_songs = Some((key, generation, songs.clone()));
                songs
            }
        }
    }

    /// Opens Windows' folder picker on a thread of its own; the folder
    /// chosen arrives in `background`.
    fn pick_local_folder(&mut self) {
        if self.picking_folder {
            return;
        }
        self.picking_folder = true;
        let tx = self.folder_picked.0.clone();
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new().name("folder-picker".into()).spawn(move || {
            let _ = tx.send(crate::platform::pick_folder("Add a folder of music"));
            waker.wake();
        });
        if spawned.is_err() {
            self.picking_folder = false;
        }
    }

    fn add_local_folder(&mut self, folder: std::path::PathBuf) {
        let name = folder.display().to_string();
        if self.settings.local_folders.iter().any(|f| folder.starts_with(f)) {
            self.toast(format!("{name} is in your music already"), ToastKind::Info);
            return;
        }
        // A folder holding ones added before takes their place.
        self.settings.local_folders.retain(|f| !std::path::Path::new(f).starts_with(&folder));
        self.settings.local_folders.push(name.clone());
        self.save_settings();
        let folders = self.settings.local_folders.clone();
        self.local.scan(&folders);
        self.watch_folders();
        self.toast(format!("Looking for music in {name}"), ToastKind::Info);
    }

    /// Looks for an update when it is time, and restarts into one that is
    /// ready.
    fn update_tick(&mut self) {
        if let Some(due) = self.update_due {
            let now = Instant::now();
            if now >= due {
                self.update_due = Some(now + Duration::from_secs(12 * 3600));
                if self.settings.check_updates && self.online() {
                    self.updater.check(true);
                }
            } else if due - now < Duration::from_secs(60) {
                crate::theme::repaint_in(&self.ctx, due - now);
            }
        }
        if let update::State::Ready(exe) = self.updater.state()
            && !self.keep_session
        {
            self.restart_into(&exe);
        }
    }

    /// Starts the new program and quits, the queue saved where it is for
    /// it to carry on from.
    fn restart_into(&mut self, exe: &std::path::Path) {
        let playing = self.playing();
        self.start_at = self.position();
        self.keep_session = true;
        self.save_session();
        let mut command = std::process::Command::new(exe);
        command
            .arg("--updated-from")
            .arg(std::process::id().to_string())
            .arg(if playing { "--resume-playing" } else { "--resume" });
        if let Some(dir) = exe.parent() {
            command.current_dir(dir);
        }
        match command.spawn() {
            Ok(_) => {
                log::info!("restarting into the update");
                // Its position stays as saved, whatever the stopped player says.
                self.current_token = 0;
                self.player.stop();
                self.actions.push(Action::Quit);
            }
            Err(error) => {
                log::warn!("cannot start the update: {error}");
                self.toast("The update is in place: it runs the next time the app starts", ToastKind::Info);
            }
        }
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
            Route::Settings | Route::LocalAlbum(_) => {}
            Route::Stats => {
                // Last.fm's, when connected, the first time.
                if !self.stats.chosen {
                    self.stats.chosen = true;
                    if self.lastfm.reader().is_some_and(|r| r.user().is_some()) {
                        self.stats.source = crate::stats::Source::LastFm;
                    }
                }
                self.load_stats(force);
            }
            Route::Artist(name) => {
                let key = name.to_lowercase();
                if force || !matches!(self.artists.get(&key), Some(Loadable::Ready(_) | Loadable::Loading)) {
                    self.artists.insert(key, Loadable::Loading);
                    let titles = self.artist_songs(name).iter().map(|s| s.title.clone()).collect();
                    self.backend.send(Request::Artist {
                        name: name.clone(),
                        lastfm: self.lastfm.reader(),
                        titles,
                    });
                }
            }
            _ => {
                if let Some((browse_id, params)) = route.browse() {
                    // Kept from last time: shown at once, and the fresh
                    // copy asked for all the same.
                    let kept = (!self.pages.contains_key(&key)).then(|| self.kept_pages.get(&key, &self.page_context())).flatten();
                    let state = match kept {
                        Some(page) => PageState::kept(page),
                        None => PageState::loading(),
                    };
                    self.pages.insert(key.clone(), state);
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
        // A page asked for from the full view (an artist's name, an
        // album's) shows once the full view makes way for it, out of full
        // screen.
        self.leave_fullscreen();
        self.full_view = false;
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
            // An artist of songs on this PC: their page from Last.fm's.
            Target::Browse { browse_id, .. } if browse_id.starts_with(local::ARTIST) => {
                self.navigate(Route::Artist(browse_id[local::ARTIST.len()..].to_owned()));
            }
            // An album of songs on this PC: its page, from the songs.
            Target::Browse { browse_id, .. } if browse_id.starts_with(local::ALBUM) => {
                self.navigate(Route::LocalAlbum(browse_id[local::ALBUM.len()..].to_owned()));
            }
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
                self.leave_fullscreen();
                self.full_view = false;
                if let Some(route) = self.history.pop() {
                    let current = std::mem::replace(&mut self.route, route.clone());
                    self.forward.push(current);
                    self.go_to(route);
                }
            }
            Action::Forward => {
                self.leave_fullscreen();
                self.full_view = false;
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
            Action::PlaySong { query, artist } => {
                let seq = self.next_seq();
                self.song_search = Some(seq);
                // The bar shows it loading meanwhile.
                self.pending_queue = Some(seq);
                self.backend.send(Request::FindSong { seq, query, artist });
            }
            Action::PlayTrack(track) if track.is_local() => {
                // No radio follows a song on this PC.
                self.pending_queue = None;
                self.queue.replace(vec![track], 0, Source::default());
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
            Action::Prewarm(track) => self.prewarm(&track),
            Action::StartRadio(track) if track.is_local() => {
                if track.artists.is_empty() {
                    self.toast("A radio needs the song's artist, and this file doesn't say", ToastKind::Info);
                    return;
                }
                // The song itself, then at once the radio that follows songs
                // on this PC: the same song on YouTube Music, or one by an
                // artist like theirs.
                self.pending_queue = None;
                self.radio_dry = false;
                self.extending = false;
                let title = Some(format!("{} radio", track.title));
                self.queue.replace(vec![track], 0, Source { title, ..Source::default() });
                self.start_current(0.0);
                self.extend_queue();
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
            Action::SetVisualiser { on, mode, focus } => {
                self.settings.visualiser = on;
                self.settings.visualiser_mode = mode;
                self.settings.visualiser_focus = focus;
                self.settings_dirty = Some(Instant::now());
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
            Action::Rate(video_id, _) if local::is_local_id(&video_id) => {}
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
            Action::OpenRenamePlaylist { playlist_id, title } => self.rename_playlist = Some((playlist_id, title)),
            Action::OpenDeletePlaylist { playlist_id, title } => self.delete_playlist = Some((playlist_id, title)),
            Action::RenamePlaylist { playlist_id, title } => {
                self.rename_playlist = None;
                let title = title.trim().to_owned();
                if title.is_empty() || !self.require_sign_in() {
                    return;
                }
                // Shown renamed at once: its page and the sidebar.
                let browse_id = format!("VL{}", playlist_id.trim_start_matches("VL"));
                if let Some(page) = self.pages.get_mut(&format!("browse:{browse_id}:")).and_then(|s| s.data.ready_mut())
                    && let Some(header) = page.header.as_mut()
                {
                    header.title = title.clone();
                }
                for item in self.playlists.iter_mut() {
                    if matches!(&item.target, Some(Target::Browse { browse_id: id, .. }) if *id == browse_id) {
                        item.title = title.clone();
                    }
                }
                self.backend.send(Request::RenamePlaylist { playlist_id, title });
            }
            Action::DeletePlaylist { playlist_id, title } => {
                self.delete_playlist = None;
                if !self.require_sign_in() {
                    return;
                }
                // Gone at once: from the sidebar, and its page left.
                let browse_id = format!("VL{}", playlist_id.trim_start_matches("VL"));
                self.playlists.retain(|item| !matches!(&item.target, Some(Target::Browse { browse_id: id, .. }) if *id == browse_id));
                let key = format!("browse:{browse_id}:");
                self.pages.remove(&key);
                self.pages.remove(&Route::Library(LibraryTab::Playlists).key());
                if self.route.key() == key {
                    self.navigate(Route::Library(LibraryTab::Playlists));
                }
                self.history.retain(|route| route.key() != key);
                self.backend.send(Request::DeletePlaylist { playlist_id, title });
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
                if self.personal.add_search(&query) {
                    self.personal_dirty = Some(Instant::now());
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
            Action::ForgetSearch(query) => {
                self.personal.remove_search(&query);
                self.personal_dirty = Some(Instant::now());
            }
            Action::ForgetSearches => {
                self.personal.searches.clear();
                self.personal_dirty = Some(Instant::now());
            }
            Action::NudgeLyrics { video_id, by } => {
                let now = self.personal.nudge_lyrics(&video_id, by);
                self.personal_dirty = Some(Instant::now());
                let text = if now > 0.0 {
                    format!("Lyrics {now:.1} s later")
                } else if now < 0.0 {
                    format!("Lyrics {:.1} s earlier", -now)
                } else {
                    "Lyrics back in their own time".to_owned()
                };
                self.toast(text, ToastKind::Info);
            }
            Action::ResetLyrics(video_id) => {
                self.personal.reset_lyrics(&video_id);
                self.personal_dirty = Some(Instant::now());
            }
            Action::ChooseLyrics { video_id, choice } => {
                self.personal.choose_lyrics(&video_id, choice);
                self.personal_dirty = Some(Instant::now());
            }
            Action::TranslateLyrics { video_id, lines } => {
                // Once at a time, and after a failure not again for a while.
                let asked = self.lyrics_translating.get(&video_id).is_some_and(|at| at.elapsed() < LYRICS_TRANSLATE_AGAIN);
                if self.settings.lyric_translations && self.online() && !asked && !lines.is_empty() {
                    self.lyrics_translating.insert(video_id.clone(), Instant::now());
                    self.backend.send(Request::TranslateLyrics { video_id, lines });
                }
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
                if self.fullscreen {
                    // Closing the player leaves full screen with it.
                    self.leave_fullscreen();
                    self.full_view = false;
                    return;
                }
                self.full_view = !self.full_view;
                if self.full_view && self.full_tab == SideTab::Lyrics {
                    self.request_lyrics();
                }
            }
            Action::ToggleFullscreen => {
                if self.fullscreen {
                    self.leave_fullscreen();
                } else {
                    self.fullscreen = true;
                    self.full_view_before = self.full_view;
                    self.full_view = true;
                    self.active_at = self.ctx.input(|i| i.time);
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    if self.full_tab == SideTab::Lyrics {
                        self.request_lyrics();
                    }
                }
            }
            Action::AddLocalFolder => self.pick_local_folder(),
            Action::AddLocalFolderPath(folder) => self.add_local_folder(folder.into()),
            Action::RemoveLocalFolder(folder) => {
                self.settings.local_folders.retain(|f| *f != folder);
                self.save_settings();
                let folders = self.settings.local_folders.clone();
                self.local.scan(&folders);
                self.watch_folders();
            }
            Action::RescanLocal => {
                let folders = self.settings.local_folders.clone();
                self.local.scan(&folders);
            }
            Action::ShowInFolder(path) => crate::platform::show_in_folder(std::path::Path::new(&path)),
            Action::CheckForUpdates => {
                self.updater.dismiss_failure();
                self.updater.check(false);
            }
            Action::InstallUpdate => self.updater.install(),
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
            Action::ShowStats { source, period } => {
                self.stats.source = source;
                self.stats.period = period;
                self.load_stats(false);
            }
            Action::SetAutostart(on) => {
                if settings::profile().is_some() {
                    self.toast("A test profile does not start with Windows", ToastKind::Info);
                } else if crate::platform::set_autostart(on) {
                    self.autostart = on;
                    self.toast(if on { "Starts with Windows, in the tray" } else { "No longer starts with Windows" }, ToastKind::Info);
                } else {
                    self.toast("Windows would not change what starts with it", ToastKind::Error);
                }
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
                self.kept_pages.clear();
                self.backend.send(Request::SetAuth(Some(auth)));
                self.signed_in = true;
                self.backend.send(Request::Account);
            }
            Action::SignOut => {
                self.kept_pages.clear();
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
                if new.eq != self.settings.eq || new.eq_gains != self.settings.eq_gains {
                    self.player.set_eq(new.eq, &new.eq_gains);
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
                if new.cached_songs != self.settings.cached_songs {
                    self.audio_cache.set_limit(new.cached_songs as usize);
                }
                if (new.zoom - self.settings.zoom).abs() > f32::EPSILON {
                    self.ctx.set_zoom_factor(new.zoom.clamp(0.7, 2.0));
                }
                if !new.restore_session {
                    let _ = std::fs::remove_file(self.paths.session());
                }
                if new.transitions() != self.settings.transitions() {
                    let (crossfade, gap) = new.transitions();
                    self.player.set_transitions(crossfade, gap);
                }
                if new.local_folders != self.settings.local_folders {
                    self.local.scan(&new.local_folders);
                    self.settings.local_folders = new.local_folders.clone();
                    self.watch_folders();
                }
                if new.check_updates && !self.settings.check_updates {
                    self.update_due = Some(Instant::now());
                }
                if new.theme != self.settings.theme {
                    crate::theme::set(new.theme);
                    crate::theme::apply(&self.ctx);
                    crate::platform::restyle_main_window(crate::theme::sidebar());
                }
                self.settings = new;
                self.settings_dirty = Some(Instant::now());
            }
            Action::CopyLink(url) => {
                self.ctx.copy_text(url);
                self.toast("Link copied", ToastKind::Info);
            }
            Action::OpenUrl(url) => {
                crate::platform::open_url(&url);
            }
            Action::ClearSongCache => {
                let bytes = self.audio_cache.usage().kept_bytes;
                self.audio_cache.clear(false);
                self.toast(format!("Freed {:.0} MB", bytes as f64 / 1_048_576.0), ToastKind::Info);
            }
            Action::ClearDownloads => {
                self.downloads = Downloads::default();
                let bytes = self.audio_cache.usage().saved_bytes;
                self.audio_cache.clear(true);
                self.marks_dirty = true;
                self.toast(format!("Removed every download, {:.0} MB", bytes as f64 / 1_048_576.0), ToastKind::Info);
            }
            Action::SaveOffline(tracks) => self.save_offline(tracks),
            Action::SaveOfflinePlaylist { playlist_id, title } => {
                let seq = self.next_seq();
                self.watch_purposes.insert(seq, WatchPurpose::Offline { title });
                self.backend.send(Request::PlaylistTracks { seq, playlist_id });
            }
            Action::RemoveOffline(ids) => {
                self.downloads.waiting.retain(|t| !ids.contains(&t.video_id));
                if self.downloads.active.as_ref().is_some_and(|d| ids.contains(&d.track.video_id)) {
                    self.downloads.active = None;
                }
                self.audio_cache.unsave(&ids);
                self.marks_dirty = true;
                self.toast(
                    if ids.len() == 1 { "Removed from downloads".to_owned() } else { format!("Removed {} songs from downloads", ids.len()) },
                    ToastKind::Info,
                );
            }
            Action::CancelDownloads => {
                let left = self.downloads.waiting.len() + usize::from(self.downloads.active.is_some());
                self.downloads = Downloads::default();
                self.marks_dirty = true;
                self.toast(format!("Stopped; {} not downloaded", crate::ui::widgets::songs(left)), ToastKind::Info);
            }
            Action::ClearLyricsCache => {
                self.storage.lock().at = None;
                let freed = crate::cache::dir_size(self.lyrics_cache.dir());
                let _ = std::fs::remove_dir_all(self.lyrics_cache.dir());
                let _ = std::fs::create_dir_all(self.lyrics_cache.dir());
                self.toast(format!("Freed {:.1} MB", freed as f64 / 1_048_576.0), ToastKind::Info);
            }
            Action::ClearImageCache => {
                self.storage.lock().at = None;
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

/// What the app keeps on disk, by kind, in bytes, for Settings.
#[derive(Clone, Copy, Debug, Default)]
pub struct Storage {
    pub downloads: u64,
    pub recent: u64,
    pub artwork: u64,
    /// The music on this PC: its files, its artwork and index.
    pub local: u64,
    /// Of which its files, the person's own rather than the app's.
    pub music: u64,
    pub lyrics: u64,
    /// Logs and the like.
    pub other: u64,
    /// Free on the drive it is all on.
    pub free: Option<u64>,
}

#[derive(Default)]
struct Measured {
    sizes: Option<Storage>,
    at: Option<Instant>,
    busy: bool,
}

/// How long before lyrics that came back incomplete are asked for again.
const LYRICS_RETRY: Duration = Duration::from_secs(300);
/// The same, when there are none to show meanwhile.
const LYRICS_RETRY_SOON: Duration = Duration::from_secs(60);
/// How long before lyrics whose translation failed are sent again.
const LYRICS_TRANSLATE_AGAIN: Duration = Duration::from_secs(600);

/// A bitrate at or above this is Premium quality (256 kb/s streams).
const PREMIUM_KBPS: u32 = 200;

/// How long a resolved stream address is used before it is resolved again;
/// YouTube's last about six hours.
const STREAM_FRESH: Duration = Duration::from_secs(3600);

/// Quick picks, by its name (the app asks YouTube Music in English unless
/// told otherwise).
fn is_quick_picks(section: &Section) -> bool {
    section.title.eq_ignore_ascii_case("Quick picks")
}

/// Quick picks second on Home, after Listen again, wherever it came;
/// false when it is not there.
fn lift_quick_picks(page: &mut Page) -> bool {
    let Some(at) = page.sections.iter().position(is_quick_picks) else {
        return false;
    };
    if at > 1 {
        let section = page.sections.remove(at);
        page.sections.insert(1, section);
    }
    true
}

/// A search on YouTube Music for a song on this PC, which has no page
/// there: as much of its names as fits the 256 characters Discord takes.
fn search_link(track: &Track) -> String {
    let mut link = "https://music.youtube.com/search?q=".to_owned();
    for c in format!("{} {}", track.artist_line(), track.title).chars() {
        let encoded = crate::api::music::urlencode(c.encode_utf8(&mut [0; 4]));
        if link.len() + encoded.len() > 256 {
            break;
        }
        link.push_str(&encoded);
    }
    link
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_link_fits_discord() {
        let track = Track {
            title: "ダーリン・イン・ザ・フランキス エンディング集 vol.1 ".repeat(4),
            artists: vec![Link { name: "XX:me".into(), id: None }],
            ..Track::default()
        };
        let link = search_link(&track);
        assert!(link.starts_with("https://music.youtube.com/search?q=XX%3Ame%20%E3%83%80"));
        assert!(link.len() <= 256);
        // Never a character cut in half.
        assert!(!link.ends_with('%') && link.rsplit('%').next().is_some_and(|last| last.len() >= 2));
    }

    #[test]
    fn quick_picks_comes_second() {
        let shelf = |title: &str| Section {
            title: title.into(),
            strapline: None,
            layout: Layout::Carousel,
            items: Vec::new(),
            more: None,
            text: None,
            continuation: None,
        };
        let mut page = Page::default();
        page.sections = ["Listen again", "Forgotten favorites", "Your daily discover", "Mixed for you", "Quick picks"].map(shelf).to_vec();
        assert!(lift_quick_picks(&mut page));
        let titles: Vec<&str> = page.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Listen again", "Quick picks", "Forgotten favorites", "Your daily discover", "Mixed for you"]);
        // First already, it stays first.
        page.sections = ["Quick picks", "Listen again"].map(shelf).to_vec();
        assert!(lift_quick_picks(&mut page));
        assert_eq!(page.sections[0].title, "Quick picks");
        page.sections = vec![shelf("Listen again")];
        assert!(!lift_quick_picks(&mut page));
    }
}

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
                title: self.shown_title(&t).to_owned(),
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
        // Discord shows only pictures on the web: a song on this PC with a
        // picture in its file or folder has its album's looked up online.
        if self.playing()
            && self.online()
            && let Some(track) = self.queue.current_track().filter(|t| t.is_local())
            && let Some(query) = self.local.web_art_wanted(track)
        {
            self.art_queue.push_front(query);
        }
        let activity = self.current().filter(|_| self.playing()).map(|track| {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            let start_ms = now_ms - (self.position() * 1000.0) as i64;
            let end_ms = self.duration().map(|d| start_ms + (d * 1000.0) as i64);
            discord::Activity {
                title: self.shown_title(track).to_owned(),
                artist: track.artist_line(),
                album: track.album.as_ref().map(|a| a.name.clone()),
                art_url: track
                    .thumbnail
                    .as_deref()
                    .filter(|url| url.starts_with("https://"))
                    .or_else(|| if track.is_local() { self.local.found_art(track) } else { None })
                    .map(|url| sized_thumbnail(url, 512))
                    .filter(|url| url.starts_with("https://") && url.len() <= 256),
                url: if track.is_local() {
                    search_link(track)
                } else {
                    format!("https://music.youtube.com/watch?v={}", track.video_id)
                },
                artist_url: track
                    .artists
                    .first()
                    .and_then(|a| a.id.as_ref())
                    .filter(|id| !id.starts_with(local::ARTIST))
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
        // The app's own count of plays, for its stats.
        self.play_log
            .tick(track.as_ref().filter(|_| self.current_token != 0).map(|t| (uid, t, position, duration, playing)));
        let current = track.as_ref().filter(|_| self.current_token != 0).map(|track| lastfm::Current {
            uid,
            track,
            position,
            duration,
            playing,
        });
        for notice in self.lastfm.tick(current, self.settings.scrobble) {
            match notice {
                lastfm::Notice::OpenUrl(url) => crate::platform::open_url(&url),
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
            .map(|t| format!("{} \u{2014} {}", self.shown_title(t), t.artist_line()));
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
        self.play_log.finish();
        if self.personal_dirty.take().is_some() {
            settings::save(&self.paths.personal(), &self.personal);
        }
        self.audio_cache.save();
        self.save_session();
        self.save_settings();
        self.player.stop();
    }
}

impl fastframe_shell::Resident for App {
    /// Started with Windows: straight to the tray, the window a click on
    /// its icon away. Without a tray icon there would be no way back to it.
    fn start_hidden(&mut self) -> bool {
        self.tray.is_some()
    }

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
