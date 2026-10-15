//! Settings and the restored session, as JSON in the roaming app data
//! folder, written atomically so a crash never leaves half a file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::model::Track;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn next(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub volume: f32,
    pub muted: bool,
    /// Turn loud tracks down to YouTube's target, as the web player does.
    pub normalize: bool,
    /// Keep playing related songs when the queue runs out.
    pub autoplay: bool,
    /// Songs follow one another without a pause (off: a moment of silence).
    pub gapless: bool,
    /// Each song fades into the next where it goes quiet (the smart
    /// crossfade), overlapping by `crossfade_secs` at most.
    pub crossfade: bool,
    pub crossfade_secs: f32,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// Closing the window keeps the music playing in the tray.
    pub close_to_tray: bool,
    /// The interface language YouTube Music answers in.
    pub language: String,
    /// The region for charts and recommendations; YouTube's guess when empty.
    pub region: String,
    /// Fall back to yt-dlp when YouTube refuses the native stream request.
    pub use_ytdlp: bool,
    /// yt-dlp's path, when it is not on PATH.
    pub ytdlp_path: String,
    /// Signed in, ask for Premium's 256 kb/s streams (through yt-dlp).
    pub premium_quality: bool,
    /// Songs kept on disk, at the best quality heard; 0 keeps none.
    pub cached_songs: u32,
    /// Look up synced lyrics on LRCLIB.
    pub synced_lyrics: bool,
    /// The equaliser, its bands in dB, and the preset they came from.
    pub eq: bool,
    pub eq_gains: [f32; 10],
    pub eq_preset: String,
    /// Readings over Japanese lyrics, in the full-window player.
    pub lyric_readings: bool,
    pub reading_style: crate::readings::Style,
    /// Show what plays on Discord.
    pub discord: bool,
    /// Another Discord application than the default, whose name shows.
    pub discord_client_id: String,
    /// Scrobble to Last.fm, once connected.
    pub scrobble: bool,
    /// Signed in, add the songs played to the account's history.
    pub history: bool,
    /// Titles in Latin letters when YouTube Music has them ("Tsubasa" for
    /// "ツバサ - Tsubasa").
    pub romanized_titles: bool,
    pub sidebar: bool,
    pub side_panel: bool,
    /// Pick up where the last session left off.
    pub restore_session: bool,
    /// The interface zoom.
    pub zoom: f32,
    /// Folders of the person's own music, played and searched beside
    /// YouTube Music's.
    pub local_folders: Vec<String>,
    /// Look for a newer version now and then.
    pub check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 0.8,
            muted: false,
            normalize: true,
            autoplay: true,
            gapless: true,
            crossfade: false,
            crossfade_secs: 6.0,
            repeat: Repeat::Off,
            shuffle: false,
            close_to_tray: true,
            language: "en".into(),
            region: String::new(),
            use_ytdlp: true,
            ytdlp_path: String::new(),
            premium_quality: true,
            cached_songs: 100,
            synced_lyrics: true,
            eq: false,
            eq_gains: [0.0; 10],
            eq_preset: "Flat".into(),
            lyric_readings: true,
            reading_style: crate::readings::Style::Romaji,
            discord: true,
            discord_client_id: String::new(),
            scrobble: true,
            history: true,
            romanized_titles: true,
            sidebar: true,
            side_panel: false,
            restore_session: true,
            zoom: 1.0,
            local_folders: Vec::new(),
            check_updates: true,
        }
    }
}

/// What was playing, for the next start.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub tracks: Vec<Track>,
    pub current: Option<usize>,
    pub position_secs: f64,
    pub playlist_id: Option<String>,
}

impl Settings {
    /// How songs follow one another, for the player: the crossfade (zero:
    /// none), and the silence between songs (zero: gapless).
    pub fn transitions(&self) -> (std::time::Duration, std::time::Duration) {
        let crossfade = if self.crossfade { self.crossfade_secs.clamp(1.0, 12.0) } else { 0.0 };
        let gap = if self.gapless || self.crossfade { 0.0 } else { 1.2 };
        (std::time::Duration::from_secs_f32(crossfade), std::time::Duration::from_secs_f32(gap))
    }
}

pub struct Paths {
    pub config: PathBuf,
    pub cache: PathBuf,
}

impl Paths {
    pub fn new() -> Self {
        // A profile of its own, for test runs beside the real app (dev).
        if let Some(dir) = profile() {
            let (config, cache) = (dir.join("config"), dir.join("cache"));
            let _ = std::fs::create_dir_all(&config);
            let _ = std::fs::create_dir_all(&cache);
            return Self { config, cache };
        }
        let dirs = directories::ProjectDirs::from("", "", "native-ytm");
        let (config, cache) = match &dirs {
            Some(dirs) => (dirs.config_dir().to_path_buf(), dirs.cache_dir().to_path_buf()),
            None => {
                let base = std::env::temp_dir().join("native-ytm");
                (base.clone(), base.join("cache"))
            }
        };
        let _ = std::fs::create_dir_all(&config);
        let _ = std::fs::create_dir_all(&cache);
        Self { config, cache }
    }

    pub fn settings(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    pub fn session(&self) -> PathBuf {
        self.config.join("session.json")
    }

    pub fn account(&self) -> PathBuf {
        self.config.join("account.bin")
    }

    /// Recent searches and lyric offsets.
    pub fn personal(&self) -> PathBuf {
        self.config.join("personal.json")
    }

    /// The plays the app has heard, for its stats.
    pub fn plays(&self) -> PathBuf {
        self.config.join("plays.jsonl")
    }

    pub fn images(&self) -> PathBuf {
        self.cache.join("images")
    }

    pub fn lyrics(&self) -> PathBuf {
        self.cache.join("lyrics")
    }

    /// Pages as they last loaded, shown at once on the next start.
    pub fn pages(&self) -> PathBuf {
        self.cache.join("pages")
    }

    pub fn songs(&self) -> PathBuf {
        self.cache.join("songs")
    }

    pub fn logs(&self) -> PathBuf {
        self.cache.join("logs")
    }

    /// Short-lived files: yt-dlp's cookie file while it runs.
    pub fn scratch(&self) -> PathBuf {
        self.cache.join("scratch")
    }
}

/// `NATIVE_YTM_PROFILE`: a folder holding a separate set of settings,
/// session and cache, run as a separate instance (dev).
pub fn profile() -> Option<PathBuf> {
    std::env::var_os("NATIVE_YTM_PROFILE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn load<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            log::warn!("{} is unreadable ({error}); using defaults", path.display());
            T::default()
        }),
        Err(_) => T::default(),
    }
}

/// Writes through a temporary file and a rename, so the old file stays
/// whole until the new one is.
pub fn save<T: Serialize>(path: &Path, value: &T) {
    let Ok(json) = serde_json::to_vec_pretty(value) else {
        return;
    };
    write_atomic(path, &json);
}

pub fn write_atomic(path: &Path, bytes: &[u8]) {
    let tmp = path.with_extension("tmp");
    if let Err(error) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
        log::warn!("cannot save {}: {error}", path.display());
    }
}
