//! The interface's own state types: where it is, what it is waiting for,
//! and what a click asks the app to do.

use crate::api::model::*;
use crate::api::music::{Filter, Rating};
use crate::queue::Source;
use crate::settings::Settings;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Home,
    Explore,
    Library(LibraryTab),
    Search { query: String, filter: Filter },
    Browse {
        browse_id: String,
        params: Option<String>,
    },
    Settings,
}

impl Route {
    /// The page cache's key.
    pub fn key(&self) -> String {
        match self {
            Route::Home => "home".into(),
            Route::Explore => "explore".into(),
            Route::Library(tab) => format!("library:{}", tab.browse_id()),
            Route::Search { query, filter } => format!("search:{filter:?}:{query}"),
            Route::Browse { browse_id, params } => {
                format!("browse:{browse_id}:{}", params.as_deref().unwrap_or(""))
            }
            Route::Settings => "settings".into(),
        }
    }

    /// The browse request behind a route, if it has one.
    pub fn browse(&self) -> Option<(String, Option<String>)> {
        match self {
            Route::Home => Some(("FEmusic_home".into(), None)),
            Route::Explore => Some(("FEmusic_explore".into(), None)),
            Route::Library(LibraryTab::Cached) => None,
            Route::Library(tab) => Some((tab.browse_id().into(), None)),
            Route::Browse { browse_id, params } => Some((browse_id.clone(), params.clone())),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LibraryTab {
    Playlists,
    Songs,
    Albums,
    Artists,
    Subscriptions,
    History,
    /// Songs on this PC, for listening offline.
    Cached,
}

impl LibraryTab {
    pub const ALL: [LibraryTab; 7] = [
        LibraryTab::Playlists,
        LibraryTab::Songs,
        LibraryTab::Albums,
        LibraryTab::Artists,
        LibraryTab::Subscriptions,
        LibraryTab::History,
        LibraryTab::Cached,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LibraryTab::Playlists => "Playlists",
            LibraryTab::Songs => "Songs",
            LibraryTab::Albums => "Albums",
            LibraryTab::Artists => "Artists",
            LibraryTab::Subscriptions => "Subscriptions",
            LibraryTab::History => "History",
            LibraryTab::Cached => "Cached",
        }
    }

    pub fn browse_id(self) -> &'static str {
        match self {
            LibraryTab::Playlists => "FEmusic_liked_playlists",
            LibraryTab::Songs => "FEmusic_liked_videos",
            LibraryTab::Albums => "FEmusic_liked_albums",
            LibraryTab::Artists => "FEmusic_library_corpus_track_artists",
            LibraryTab::Subscriptions => "FEmusic_library_corpus_artists",
            LibraryTab::History => "FEmusic_history",
            LibraryTab::Cached => "",
        }
    }
}

pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Loadable<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Loadable::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn ready_mut(&mut self) -> Option<&mut T> {
        match self {
            Loadable::Ready(value) => Some(value),
            _ => None,
        }
    }
}

pub struct PageState {
    pub data: Loadable<Page>,
    pub loading_more: bool,
    pub loading_tracks: bool,
    /// Filter for a long playlist's songs.
    pub filter: String,
}

impl PageState {
    pub fn loading() -> Self {
        Self {
            data: Loadable::Loading,
            loading_more: false,
            loading_tracks: false,
            filter: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideTab {
    UpNext,
    Lyrics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Error,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub born: std::time::Instant,
    pub until: std::time::Instant,
}

/// What a view asks for. Views push these while drawing; the app applies
/// them after the frame is drawn.
#[derive(Clone, Debug)]
pub enum Action {
    Navigate(Route),
    Back,
    Forward,
    Refresh,
    /// Wherever a target leads: a page, a search, or playback.
    Open(Target),
    /// Play songs already listed, starting at one, as the queue.
    PlayTracks {
        tracks: Vec<Track>,
        start: usize,
        source: Source,
    },
    /// Play one song, followed by its radio.
    PlayTrack(Track),
    /// Ask YouTube Music for the queue: a playlist, a mix, a radio.
    PlayWatch {
        video_id: Option<String>,
        playlist_id: Option<String>,
        params: Option<String>,
        title: Option<String>,
    },
    /// Play a playlist's songs from the start, in full, shuffled or not.
    PlayPlaylist {
        playlist_id: String,
        shuffle: bool,
        title: Option<String>,
    },
    StartRadio(Track),
    PlayNext(Vec<Track>),
    AddToQueue(Vec<Track>),
    PlayQueueEntry(u64),
    RemoveFromQueue(u64),
    MoveInQueue(usize, usize),
    ClearQueue,
    TogglePlay,
    Next,
    Previous,
    Seek(f64),
    SeekBy(f64),
    SetVolume(f32),
    ToggleMute,
    ToggleShuffle,
    SetShuffle(bool),
    CycleRepeat,
    Rate(String, Rating),
    SavePlaylist(String, bool),
    Subscribe(String, bool),
    /// Opens "Save to playlist" for these songs.
    PickPlaylist(Vec<Track>),
    AddToPlaylist {
        playlist_id: String,
        title: String,
        tracks: Vec<Track>,
    },
    RemoveFromPlaylist {
        playlist_id: String,
        track: Track,
    },
    CreatePlaylist {
        title: String,
        tracks: Vec<Track>,
    },
    LoadMore(String),
    LoadMoreTracks(String),
    SearchMore,
    Search(String),
    SetSearchFilter(Filter),
    Suggest(String),
    ShowSidePanel(SideTab),
    ToggleSidePanel,
    /// The full-window player, or back.
    ToggleFullView,
    /// Starts Last.fm's authorisation with the API account typed in.
    LastFmConnect,
    LastFmDisconnect,
    ToggleSidebar,
    OpenSignIn,
    SignIn(String),
    SignOut,
    ApplySettings(Box<Settings>),
    CopyLink(String),
    OpenUrl(String),
    ClearImageCache,
    /// Deletes the songs kept from plays.
    ClearSongCache,
    /// Downloads songs for offline listening.
    SaveOffline(Vec<Track>),
    /// Downloads a whole playlist or album, more than its page has loaded.
    SaveOfflinePlaylist {
        playlist_id: String,
        title: String,
    },
    /// No longer downloaded.
    RemoveOffline(Vec<String>),
    /// Stops the downloads waiting and the one under way.
    CancelDownloads,
    /// Deletes every downloaded song.
    ClearDownloads,
    ShowWindow,
    Quit,
}
