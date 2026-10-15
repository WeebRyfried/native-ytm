//! Songs in the person's own folders: MP3, FLAC, M4A, Ogg, WAV and the rest.
//!
//! A scan on a thread of its own walks the folders and reads each file's
//! tags and picture. What it read is kept in an index on disk, so the next
//! scan opens only the files that changed. The songs are plain [`Track`]s
//! carrying their file: they play from disk as a downloaded song does, and
//! search finds them before anything on YouTube Music, by their names or,
//! for Japanese ones, by how they read ("torikago" finds トリカゴ). A song
//! with no picture of its own takes one found online, remembered here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_VORBIS,
};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTag, StandardVisualKey, Tag, Visual};

use crate::api::model::{Link, Track};
use crate::player::StreamInfo;

/// The files played, by extension.
const AUDIO: [&str; 14] = ["mp3", "flac", "m4a", "m4b", "mp4", "aac", "ogg", "oga", "opus", "wav", "wave", "aif", "aiff", "mka"];

/// Pictures beside the songs that are taken as their album's artwork.
const COVERS: [&str; 6] = ["cover", "folder", "front", "album", "albumart", "artwork"];

/// The index's layout. An index of another version is read again from the
/// files.
const VERSION: u32 = 2;

/// Pictures found online, by album: kept between runs.
const FOUND: &str = "found-art.json";

/// How long a picture not found online is not looked for again.
const RETRY_SECS: u64 = 3 * 24 * 3600;

/// The id an artist's link carries: their page, made from Last.fm's.
pub const ARTIST: &str = "artist:";

/// An album of the music on this PC, in its page's address.
pub const ALBUM: &str = "localalbum:";

/// What a file's tags say, and how it is encoded.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Meta {
    title: String,
    /// The artists as one line, and one by one.
    artist: String,
    artists: Vec<String>,
    album: String,
    album_artist: String,
    year: Option<u16>,
    track: Option<u32>,
    disc: Option<u32>,
    duration_ms: Option<u64>,
    /// The artwork: a picture from the file, kept in the art folder, or
    /// one beside it.
    art: Option<String>,
    /// For people: "FLAC 24-bit 96 kHz", "MP3 320 kb/s".
    quality: String,
    kbps: Option<u32>,
    mime: String,
    /// ReplayGain's loudness in YouTube's terms: dB above its target.
    loudness_db: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    path: String,
    size: u64,
    /// Seconds since 1970.
    modified: u64,
    /// None for a file that will not play.
    meta: Option<Meta>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Index {
    version: u32,
    entries: Vec<Entry>,
}

/// A picture looked for online, and when.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Found {
    url: Option<String>,
    at: u64,
}

/// A song (or its album) wanting a picture: what to look it up by.
#[derive(Clone, Debug, PartialEq)]
pub struct ArtQuery {
    pub key: String,
    pub artist: String,
    pub album: Option<String>,
    pub title: String,
}

/// A song's words for search, lowercased.
struct Words {
    /// The title, and its reading in Latin letters, on lines of their own.
    title: String,
    artists: String,
    /// All of it: title, artists, album, and their readings.
    text: String,
    /// All of it run together without spaces, marks or long vowels.
    loose: String,
}

/// A scan made ready on its thread: the interface only swaps it in.
struct Built {
    songs: Vec<Track>,
    words: Vec<Words>,
    playback: HashMap<String, Playback>,
    albums: usize,
    bytes: u64,
    found: HashMap<String, Found>,
}

/// How a song plays, beside its track.
struct Playback {
    quality: String,
    kbps: Option<u32>,
    mime: String,
    loudness_db: Option<f32>,
    duration_ms: Option<u64>,
}

/// A scan's progress, shared with its thread.
#[derive(Default)]
pub struct Progress {
    running: AtomicBool,
    /// Files looked at so far.
    seen: AtomicUsize,
    /// Asked for again while running: it runs once more at the end.
    again: AtomicBool,
}

/// An album of the music on this PC: its songs share its name and its
/// artist (the album artist's tag, else the first song's artist).
#[derive(Clone, Debug, PartialEq)]
pub struct LocalAlbum {
    /// What its page is found by: its artist and name.
    pub key: String,
    pub title: String,
    /// Its artist, or "Various artists" when its songs' differ.
    pub artist: String,
    pub year: Option<u16>,
    pub art: Option<String>,
    pub songs: usize,
    pub seconds: u64,
    /// The folder its first song is in.
    pub folder: Option<String>,
}

pub struct LocalLibrary {
    dir: PathBuf,
    /// The albums the songs make, as of the songs' generation.
    album_list: (u64, Arc<Vec<LocalAlbum>>),
    /// Every song, by artist, album, disc and track.
    songs: Arc<Vec<Track>>,
    /// Each song's words for search.
    words: Vec<Words>,
    playback: HashMap<String, Playback>,
    /// Pictures found online by album, and those being looked for.
    found: HashMap<String, Found>,
    found_dirty: bool,
    asked: HashSet<String>,
    /// How many albums the songs make.
    albums: usize,
    /// The songs' files, all told.
    bytes: u64,
    /// Changes whenever the songs do.
    generation: u64,
    progress: Arc<Progress>,
    folders: Arc<parking_lot::Mutex<Vec<PathBuf>>>,
    results: (Sender<Built>, Receiver<Built>),
    wake: Arc<dyn Fn() + Send + Sync>,
    /// A scan has finished since the app started.
    pub scanned: bool,
}

impl LocalLibrary {
    /// The library kept in `dir`. Nothing is read until [`Self::scan`].
    pub fn new(dir: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            dir,
            album_list: (u64::MAX, Arc::default()),
            songs: Arc::default(),
            words: Vec::new(),
            playback: HashMap::new(),
            found: HashMap::new(),
            found_dirty: false,
            asked: HashSet::new(),
            albums: 0,
            bytes: 0,
            generation: 0,
            progress: Arc::default(),
            folders: Arc::default(),
            results: channel(),
            wake,
            scanned: false,
        }
    }

    pub fn songs(&self) -> Arc<Vec<Track>> {
        self.songs.clone()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Scanning, and how many files it has looked at.
    pub fn scanning(&self) -> Option<usize> {
        self.progress.running.load(Ordering::Relaxed).then(|| self.progress.seen.load(Ordering::Relaxed))
    }

    /// Looks through `folders` for songs, on a thread of its own: first the
    /// songs the index already knows, at once, then whatever changed.
    pub fn scan(&mut self, folders: &[String]) {
        *self.folders.lock() = folders.iter().map(PathBuf::from).collect();
        if folders.is_empty() {
            self.progress.again.store(false, Ordering::SeqCst);
            if !self.songs.is_empty() || !self.scanned {
                self.take(build(Vec::new(), HashMap::new()));
            }
            self.scanned = true;
            let _ = std::fs::remove_file(self.dir.join("index.json"));
            return;
        }
        if self.progress.running.swap(true, Ordering::SeqCst) {
            self.progress.again.store(true, Ordering::SeqCst);
            return;
        }
        self.progress.seen.store(0, Ordering::Relaxed);
        let (dir, progress, folders, tx, wake) = (
            self.dir.clone(),
            self.progress.clone(),
            self.folders.clone(),
            self.results.0.clone(),
            self.wake.clone(),
        );
        let quick = !self.scanned && self.songs.is_empty();
        let spawned = std::thread::Builder::new().name("local-scan".into()).spawn(move || {
            lower_priority();
            let mut quick = quick;
            loop {
                let wanted = folders.lock().clone();
                scan(&wanted, &dir, &progress, &tx, &*wake, quick);
                quick = false;
                if !progress.again.swap(false, Ordering::SeqCst) {
                    break;
                }
            }
            progress.running.store(false, Ordering::SeqCst);
            wake();
        });
        if spawned.is_err() {
            self.progress.running.store(false, Ordering::SeqCst);
        }
    }

    /// Takes in a finished scan. Call once a frame; true when the songs
    /// changed.
    pub fn poll(&mut self) -> bool {
        let mut latest = None;
        while let Ok(built) = self.results.1.try_recv() {
            latest = Some(built);
        }
        if !self.scanned && !self.progress.running.load(Ordering::SeqCst) {
            self.scanned = true;
        }
        match latest {
            Some(built) => {
                self.take(built);
                true
            }
            None => false,
        }
    }

    fn take(&mut self, built: Built) {
        let Built { mut songs, words, playback, albums, bytes, found } = built;
        // Pictures found since the scan read the file of them.
        for (key, found) in found {
            self.found.entry(key).or_insert(found);
        }
        for song in songs.iter_mut().filter(|s| s.thumbnail.is_none()) {
            song.thumbnail = self.found.get(&art_key(song)).and_then(|f| f.url.clone());
        }
        self.songs = Arc::new(songs);
        self.words = words;
        self.playback = playback;
        self.albums = albums;
        self.bytes = bytes;
        self.generation += 1;
    }

    /// The songs matching `query`: by every word of it in their title,
    /// artists or album (or how those read, for Japanese ones), or by all
    /// of it run together ("torikago", "xxme"). Those whose title has it
    /// come first, then those whose artists do.
    pub fn search(&self, query: &str) -> Vec<Track> {
        let query = query.trim().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        if words.is_empty() {
            return Vec::new();
        }
        let squeezed = loose(&query);
        let mut hits: Vec<(u8, usize)> = self
            .words
            .iter()
            .enumerate()
            .filter(|(_, w)| words.iter().all(|q| w.text.contains(q)) || (squeezed.chars().count() >= 2 && w.loose.contains(&squeezed)))
            .map(|(i, w)| {
                let rank = if w.title.lines().any(|t| t.starts_with(&query)) {
                    0
                } else if w.title.contains(&query) || loose(&w.title).contains(&squeezed) {
                    1
                } else if w.artists.contains(&query) || loose(&w.artists).contains(&squeezed) {
                    2
                } else {
                    3
                };
                (rank, i)
            })
            .collect();
        hits.sort_unstable();
        hits.into_iter().map(|(_, i)| self.songs[i].clone()).collect()
    }

    /// The songs (by album) still without a picture, to look for online,
    /// each asked for once.
    pub fn art_wanted(&mut self) -> Vec<ArtQuery> {
        let now = unix_now();
        let mut wanted = Vec::new();
        for song in self.songs.iter().filter(|s| s.thumbnail.is_none()) {
            let Some(artist) = song.artists.first() else { continue };
            let key = art_key(song);
            let tried = self.found.get(&key).is_some_and(|f| f.url.is_some() || now.saturating_sub(f.at) < RETRY_SECS);
            if tried || !self.asked.insert(key.clone()) {
                continue;
            }
            wanted.push(ArtQuery {
                key,
                artist: artist.name.clone(),
                album: song.album.as_ref().map(|a| a.name.clone()),
                title: song.title.clone(),
            });
        }
        wanted
    }

    /// A picture found online (or not) for the songs under `key`.
    pub fn art_found(&mut self, key: &str, url: Option<String>) {
        self.found.insert(key.to_owned(), Found { url: url.clone(), at: unix_now() });
        self.found_dirty = true;
        let Some(url) = url else { return };
        let songs = Arc::make_mut(&mut self.songs);
        for song in songs.iter_mut().filter(|s| s.thumbnail.is_none() && art_key(s) == key) {
            song.thumbnail = Some(url.clone());
        }
        self.generation += 1;
    }

    /// Writes down the pictures found, once a round of looking is over.
    pub fn save_found(&mut self) {
        if std::mem::take(&mut self.found_dirty) {
            let _ = std::fs::create_dir_all(&self.dir);
            crate::settings::save(&self.dir.join(FOUND), &self.found);
        }
    }

    /// The songs by an artist (any of their artists, by name).
    pub fn by_artist(&self, name: &str) -> Vec<Track> {
        let name = name.trim().to_lowercase();
        self.songs
            .iter()
            .filter(|s| s.artists.iter().any(|a| a.name.trim().to_lowercase() == name))
            .cloned()
            .collect()
    }

    /// The stream for a song on this PC, or None when its file is gone.
    pub fn stream_for(&self, track: &Track) -> Option<StreamInfo> {
        let path = track.file.as_deref()?;
        let len = std::fs::metadata(path).ok().filter(|m| m.is_file())?.len();
        let known = self.playback.get(&track.video_id);
        let duration_ms = known
            .and_then(|p| p.duration_ms)
            .or_else(|| track.duration.map(|s| u64::from(s) * 1000));
        Some(StreamInfo {
            url: format!("file:{path}"),
            headers: Vec::new(),
            content_length: len,
            duration: duration_ms.map(Duration::from_millis),
            loudness_db: known.and_then(|p| p.loudness_db),
            mime: known.map_or_else(|| mime_of(Path::new(path)).to_owned(), |p| p.mime.clone()),
            quality: known.map_or_else(|| format_of(Path::new(path)).to_owned(), |p| p.quality.clone()),
            kbps: known.and_then(|p| p.kbps),
            cache_key: None,
        })
    }

    /// How many albums the songs make.
    pub fn albums(&self) -> usize {
        self.albums
    }

    /// The size of the songs' files, all told.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The albums, in the songs' order (by artist, then album), made again
    /// only when the songs change.
    pub fn album_list(&mut self) -> Arc<Vec<LocalAlbum>> {
        if self.album_list.0 != self.generation {
            self.album_list = (self.generation, Arc::new(albums_of(&self.songs)));
        }
        self.album_list.1.clone()
    }

    /// An album's songs, in order, by its key.
    pub fn album_songs(&self, key: &str) -> Vec<Track> {
        let id = format!("{ALBUM}{key}");
        self.songs.iter().filter(|s| s.album.as_ref().and_then(|a| a.id.as_deref()) == Some(id.as_str())).cloned().collect()
    }
}

/// What an album is found by: its artist and its name, in lower case.
pub fn album_key(artist: &str, album: &str) -> String {
    format!("{}\t{}", artist.trim().to_lowercase(), album.trim().to_lowercase())
}

/// The albums songs make, in their order.
fn albums_of(songs: &[Track]) -> Vec<LocalAlbum> {
    let mut albums: Vec<LocalAlbum> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for song in songs {
        let Some(album) = &song.album else { continue };
        let Some(key) = album.id.as_deref().and_then(|id| id.strip_prefix(ALBUM)) else {
            continue;
        };
        let artist = song.artists.first().map(|a| a.name.clone()).unwrap_or_default();
        let seconds = u64::from(song.duration.unwrap_or(0));
        match at.get(key) {
            Some(&i) => {
                let entry = &mut albums[i];
                entry.songs += 1;
                entry.seconds += seconds;
                if !entry.artist.eq_ignore_ascii_case(&artist) && entry.artist != VARIOUS {
                    entry.artist = VARIOUS.to_owned();
                }
                entry.year = entry.year.or(song.year);
                if entry.art.is_none() {
                    entry.art = song.thumbnail.clone();
                }
            }
            None => {
                at.insert(key.to_owned(), albums.len());
                albums.push(LocalAlbum {
                    key: key.to_owned(),
                    title: album.name.clone(),
                    artist,
                    year: song.year,
                    art: song.thumbnail.clone(),
                    songs: 1,
                    seconds,
                    folder: song.file.as_deref().and_then(|f| Path::new(f).parent()).map(|p| p.to_string_lossy().into_owned()),
                });
            }
        }
    }
    albums
}

/// An album whose songs are by more than one artist.
const VARIOUS: &str = "Various artists";

/// Which picture a song shares: its album's, or its own when it has none.
fn art_key(track: &Track) -> String {
    let artist = track.artists.first().map(|a| a.name.to_lowercase()).unwrap_or_default();
    match &track.album {
        Some(album) => format!("{artist}\t{}", album.name.to_lowercase()),
        None => format!("{artist}\t\t{}", track.title.to_lowercase()),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Text as search compares it loosely: lowercase letters and digits only
/// (full-width ones as ordinary ones), and a vowel written twice or "ou"
/// as one, so "daarin" meets "darin" and "toukyou" meets "tokyo".
pub fn loose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = '\0';
    for c in text.chars() {
        // Ｘ, ７: the full-width forms of ASCII.
        let c = match c as u32 {
            0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        };
        for c in c.to_lowercase().filter(|c| c.is_alphanumeric()) {
            let held = "aeiou".contains(c) && (c == last || (last == 'o' && c == 'u'));
            if !held {
                out.push(c);
            }
            last = c;
        }
    }
    out
}

/// A scan's entries as songs, sorted, with their words for search: the
/// slow part, done on the scan's thread.
fn build(entries: Vec<Entry>, found: HashMap<String, Found>) -> Built {
    let bytes = entries.iter().filter(|e| e.meta.is_some()).map(|e| e.size).sum();
    let mut rows: Vec<(Track, Playback, (String, String, u32, u32))> = entries
        .into_iter()
        .filter_map(|entry| {
            let meta = entry.meta?;
            let track = track_of(&entry.path, &meta);
            // By the album's artist, or the song's (perhaps from its file's
            // name).
            let artist = if meta.album_artist.is_empty() { track.artists.first().map(|a| a.name.clone()).unwrap_or_default() } else { meta.album_artist.clone() };
            let order = (sort_key(&artist), sort_key(&meta.album), meta.disc.unwrap_or(0), meta.track.unwrap_or(0));
            let playback = Playback {
                quality: meta.quality,
                kbps: meta.kbps,
                mime: meta.mime,
                loudness_db: meta.loudness_db,
                duration_ms: meta.duration_ms,
            };
            Some((track, playback, order))
        })
        .collect();
    rows.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| sort_key(&a.0.title).cmp(&sort_key(&b.0.title))));
    // Japanese names read in Latin letters too; the dictionary is read only
    // when there are some.
    let japanese = rows.iter().any(|(t, _, _)| {
        crate::readings::has_japanese(&t.title) || crate::readings::has_japanese(&t.artist_line())
            || t.album.as_ref().is_some_and(|a| crate::readings::has_japanese(&a.name))
    });
    let dictionary = if japanese { crate::readings::dictionary_now() } else { None };
    let read = |text: &str| -> String {
        match dictionary {
            Some(d) if crate::readings::has_japanese(text) => crate::readings::romanize(d, text),
            _ => String::new(),
        }
    };
    let words = rows
        .iter()
        .map(|(track, _, _)| {
            let title = track.title.to_lowercase();
            let artists = track.artists.iter().map(|a| a.name.to_lowercase()).collect::<Vec<_>>().join(", ");
            let album = track.album.as_ref().map(|a| a.name.to_lowercase()).unwrap_or_default();
            let (title_read, artists_read, album_read) = (read(&title), read(&artists), read(&album));
            let title = if title_read.is_empty() { title } else { format!("{title}\n{title_read}") };
            let artists = if artists_read.is_empty() { artists } else { format!("{artists}\n{artists_read}") };
            let text = format!("{title}\n{artists}\n{album}\n{album_read}");
            let squeezed = text.lines().map(loose).collect::<Vec<_>>().join("\n");
            Words { title, artists, text, loose: squeezed }
        })
        .collect();
    let mut playback = HashMap::with_capacity(rows.len());
    let mut songs = Vec::with_capacity(rows.len());
    for (track, play, _) in rows {
        playback.insert(track.video_id.clone(), play);
        songs.push(track);
    }
    let albums = songs
        .iter()
        .filter_map(|t| t.album.as_ref().map(|a| (a.name.as_str(), Path::new(t.file.as_deref().unwrap_or_default()).parent())))
        .collect::<HashSet<_>>()
        .len();
    Built { songs, words, playback, albums, bytes, found }
}

/// Whether an id is a song on this PC's rather than YouTube's.
pub fn is_local_id(id: &str) -> bool {
    id.starts_with("local:")
}

/// A song's id: its path, hashed, so a song keeps its id between scans.
pub fn id_of(path: &str) -> String {
    let hash = path
        .to_lowercase()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("local:{hash:016x}")
}

/// A name as it sorts: without case or a leading "The".
fn sort_key(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    lower.strip_prefix("the ").map(str::to_owned).unwrap_or(lower)
}

fn track_of(path: &str, meta: &Meta) -> Track {
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    // Untagged files are often named "Artist - Title".
    let (named_artist, named_title) = match stem.split_once(" - ") {
        Some((artist, title)) if meta.title.is_empty() && meta.artist.is_empty() => (artist.trim().to_owned(), title.trim().to_owned()),
        _ => (String::new(), stem.clone()),
    };
    let title = if meta.title.is_empty() { named_title } else { meta.title.clone() };
    let artists = if meta.artists.is_empty() {
        [&meta.artist, &meta.album_artist, &named_artist].into_iter().find(|a| !a.is_empty()).cloned().into_iter().collect()
    } else {
        meta.artists.clone()
    };
    // The album leads to its page, by its artist and name.
    let album = (!meta.album.is_empty()).then(|| {
        let by = if meta.album_artist.is_empty() { artists.first().cloned().unwrap_or_default() } else { meta.album_artist.clone() };
        Link::new(meta.album.clone(), Some(format!("{ALBUM}{}", album_key(&by, &meta.album))))
    });
    Track {
        video_id: id_of(path),
        title,
        // Each artist leads to their page.
        artists: artists.into_iter().map(|name| Link::new(name.clone(), Some(format!("{ARTIST}{name}")))).collect(),
        album,
        duration: meta.duration_ms.map(|ms| ((ms + 500) / 1000) as u32),
        thumbnail: meta.art.as_ref().map(|art| format!("file:{art}")),
        year: meta.year,
        file: Some(path.to_owned()),
        extra: Some("On this PC".into()),
        ..Track::default()
    }
}

#[cfg(windows)]
fn lower_priority() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
    // SAFETY: the calling thread's pseudo handle. Background mode lowers its
    // CPU and disk priority, so a scan never stutters the music.
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

#[cfg(not(windows))]
fn lower_priority() {}

/// One scan: the index's songs first when `quick`, then every file in
/// `folders`, those unchanged since the index taken from it.
fn scan(folders: &[PathBuf], dir: &Path, progress: &Progress, tx: &Sender<Built>, wake: &(dyn Fn() + Send + Sync), quick: bool) {
    let index_path = dir.join("index.json");
    let art_dir = dir.join("art");
    let index: Index = crate::settings::load(&index_path);
    let found: HashMap<String, Found> = crate::settings::load(&dir.join(FOUND));
    let mut known: HashMap<String, Entry> = if index.version == VERSION {
        index.entries.into_iter().map(|e| (e.path.clone(), e)).collect()
    } else {
        HashMap::new()
    };
    let inside = |path: &str| folders.iter().any(|f| Path::new(path).starts_with(f));
    if quick && !known.is_empty() {
        let _ = tx.send(build(known.values().filter(|e| inside(&e.path)).cloned().collect(), found.clone()));
        wake();
    }
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for folder in folders {
        walk(folder, &mut files, &mut seen, 0);
    }
    // The files the index knows as they are stay; the rest are read, on a
    // few threads at once (each at background priority) when there are
    // many, as on a first scan.
    let mut entries: Vec<Option<Entry>> = Vec::with_capacity(files.len());
    let mut unread = Vec::new();
    let mut changed = index.version != VERSION;
    for (path, size, modified) in files {
        let key = path.to_string_lossy().into_owned();
        match known.remove(&key) {
            Some(entry) if entry.size == size && entry.modified == modified => {
                progress.seen.fetch_add(1, Ordering::Relaxed);
                entries.push(Some(entry));
            }
            _ => {
                changed = true;
                unread.push((entries.len(), path, key, size, modified));
                entries.push(None);
            }
        }
    }
    for (at, entry) in read_all(&unread, &art_dir, progress) {
        entries[at] = Some(entry);
    }
    let entries: Vec<Entry> = entries.into_iter().flatten().collect();
    // Files gone from the folders, or folders no longer scanned.
    changed |= !known.is_empty();
    if changed {
        let _ = std::fs::create_dir_all(dir);
        crate::settings::save(&index_path, &Index { version: VERSION, entries: entries.clone() });
        forget_unused_art(&art_dir, &entries);
        log::info!("local music: {} songs in {} folders", entries.iter().filter(|e| e.meta.is_some()).count(), folders.len());
    }
    if changed || !quick {
        let _ = tx.send(build(entries, found));
        wake();
    }
}

/// Reads the files not in the index (where each goes among the entries,
/// its path, key, size and time), on as many threads as is worth it: a
/// few files on this one, many on up to four.
fn read_all(unread: &[(usize, PathBuf, String, u64, u64)], art_dir: &Path, progress: &Progress) -> Vec<(usize, Entry)> {
    let threads = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 4));
    let threads = if unread.len() < 16 { 1 } else { threads };
    let next = AtomicUsize::new(0);
    let read = parking_lot::Mutex::new(Vec::with_capacity(unread.len()));
    let work = || {
        // The pictures beside the songs, by folder: each folder looked in once.
        let mut covers = HashMap::new();
        loop {
            let n = next.fetch_add(1, Ordering::Relaxed);
            let Some((at, path, key, size, modified)) = unread.get(n) else {
                break;
            };
            let meta = read_meta(path, *size, art_dir, &mut covers);
            progress.seen.fetch_add(1, Ordering::Relaxed);
            read.lock().push((*at, Entry { path: key.clone(), size: *size, modified: *modified, meta }));
        }
    };
    if threads == 1 {
        work();
    } else {
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    lower_priority();
                    work();
                });
            }
        });
        log::info!("read {} files on {threads} threads", unread.len());
    }
    read.into_inner()
}

/// The audio files under `dir`, with their sizes and times.
fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64, u64)>, seen: &mut HashSet<PathBuf>, depth: u32) {
    if depth > 24 || !seen.insert(dir.to_path_buf()) {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            dirs.push(path);
        } else if kind.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| AUDIO.contains(&e.to_ascii_lowercase().as_str()))
            && let Ok(meta) = entry.metadata()
        {
            let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            out.push((path, meta.len(), modified));
        }
    }
    for dir in dirs {
        walk(&dir, out, seen, depth + 1);
    }
}

/// Reads a file's tags, picture and stream, or None if it will not play.
fn read_meta(path: &Path, size: u64, art_dir: &Path, covers: &mut HashMap<PathBuf, Option<String>>) -> Option<Meta> {
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = match symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default()) {
        Ok(format) => format,
        Err(error) => {
            log::info!("not playable: {} ({error})", path.display());
            return None;
        }
    };
    let track = format.default_track(TrackType::Audio)?;
    let Some(CodecParameters::Audio(params)) = &track.codec_params else {
        return None;
    };
    let mut meta = Meta {
        duration_ms: length(track).map(|d| d.as_millis() as u64),
        mime: mime_of(path).to_owned(),
        ..Meta::default()
    };
    let kbps = meta
        .duration_ms
        .filter(|ms| *ms > 0)
        .map(|ms| ((size as f64 * 8.0 / ms as f64).round() as u32).max(1));
    meta.kbps = kbps;
    // Worked out from the file's size, which tags and pictures add to: an
    // encoder's usual rate when it is that, near enough.
    let kbps = kbps.map(|k| {
        const USUAL: [u32; 14] = [32, 48, 64, 96, 112, 128, 160, 192, 224, 256, 288, 320, 384, 512];
        USUAL.into_iter().find(|u| k.abs_diff(*u) * 25 <= *u).unwrap_or(k)
    });
    let rate = params.sample_rate.map(|r| format!("{} kHz", trim_zeros(f64::from(r) / 1000.0)));
    let bits = params.bits_per_sample.or(params.bits_per_coded_sample).map(|b| format!("{b}-bit"));
    let codec = params.codec;
    let lossless = |name: &str| [Some(name.to_owned()), bits.clone(), rate.clone()].into_iter().flatten().collect::<Vec<_>>().join(" ");
    let lossy = |name: &str| match kbps {
        Some(k) => format!("{name} {k} kb/s"),
        None => name.to_owned(),
    };
    meta.quality = if codec == CODEC_ID_FLAC {
        lossless("FLAC")
    } else if codec == CODEC_ID_ALAC {
        lossless("ALAC")
    } else if codec == CODEC_ID_MP3 {
        lossy("MP3")
    } else if codec == CODEC_ID_AAC {
        lossy("AAC")
    } else if codec == CODEC_ID_VORBIS {
        lossy("Vorbis")
    } else if codec == CODEC_ID_OPUS {
        lossy("Opus")
    } else {
        lossless(format_of(path))
    };

    // The tags, from whichever revision says most first: the ID3v2 tag
    // over the ID3v1 one at the end of the same file.
    let mut revisions: Vec<MetadataRevision> = Vec::new();
    {
        let mut log = format.metadata();
        while let Some(revision) = log.pop() {
            revisions.push(revision);
        }
        if let Some(revision) = log.current() {
            revisions.push(revision.clone());
        }
    }
    let tags_of = |r: &MetadataRevision| -> Vec<Tag> {
        r.media.tags.iter().chain(r.per_track.iter().flat_map(|t| t.metadata.tags.iter())).cloned().collect()
    };
    revisions.sort_by_key(|r| std::cmp::Reverse(r.media.tags.len() + r.per_track.iter().map(|t| t.metadata.tags.len()).sum::<usize>()));
    for revision in &revisions {
        read_tags(&tags_of(revision), &mut meta);
    }
    let visuals: Vec<&Visual> = revisions
        .iter()
        .flat_map(|r| r.media.visuals.iter().chain(r.per_track.iter().flat_map(|t| t.metadata.visuals.iter())))
        .collect();
    let picture = visuals
        .iter()
        .find(|v| v.usage == Some(StandardVisualKey::FrontCover))
        .or_else(|| visuals.first())
        .filter(|v| !v.data.is_empty());
    meta.art = match picture {
        Some(visual) => keep_art(art_dir, visual),
        None => path.parent().and_then(|folder| {
            covers
                .entry(folder.to_path_buf())
                .or_insert_with(|| cover_beside(folder))
                .clone()
        }),
    };
    Some(meta)
}

fn read_tags(tags: &[Tag], meta: &mut Meta) {
    fn fill(field: &mut String, value: &str) {
        let value = value.trim().trim_matches('\0');
        if field.is_empty() && !value.is_empty() {
            *field = value.to_owned();
        }
    }
    let mut artists: Vec<String> = Vec::new();
    for tag in tags {
        let Some(std) = &tag.std else { continue };
        match std {
            StandardTag::TrackTitle(s) => fill(&mut meta.title, s),
            StandardTag::Artist(s) => {
                // Several artist tags (as FLAC files often have) are one line.
                for name in s.split(['\0', ';']).map(str::trim).filter(|n| !n.is_empty()) {
                    if !artists.iter().any(|a| a == name) {
                        artists.push(name.to_owned());
                    }
                }
            }
            StandardTag::AlbumArtist(s) => fill(&mut meta.album_artist, s),
            StandardTag::Album(s) => fill(&mut meta.album, s),
            StandardTag::TrackNumber(n) => {
                meta.track.get_or_insert(*n as u32);
            }
            StandardTag::DiscNumber(n) => {
                meta.disc.get_or_insert(*n as u32);
            }
            StandardTag::ReleaseYear(y) | StandardTag::RecordingYear(y) | StandardTag::OriginalReleaseYear(y) => {
                meta.year = meta.year.or(Some(*y)).filter(|y| *y > 0);
            }
            StandardTag::ReleaseDate(s) | StandardTag::RecordingDate(s) | StandardTag::OriginalReleaseDate(s) => {
                meta.year = meta.year.or_else(|| year_in(s));
            }
            StandardTag::ReplayGainTrackGain(s) => {
                // ReplayGain lifts or lowers a song to -18 LUFS; YouTube's
                // level is -14, so a song needing -7 dB is 3 dB too loud.
                if meta.loudness_db.is_none()
                    && let Some(gain) = s.trim().trim_end_matches("dB").trim().parse::<f32>().ok()
                {
                    meta.loudness_db = Some(-4.0 - gain);
                }
            }
            _ => {}
        }
    }
    if meta.artist.is_empty() && !artists.is_empty() {
        meta.artist = artists.join(", ");
        meta.artists = artists;
    }
}

/// The first plausible year in a date such as "2019-05-01".
fn year_in(date: &str) -> Option<u16> {
    let digits: String = date.chars().skip_while(|c| !c.is_ascii_digit()).take(4).collect();
    digits.parse().ok().filter(|y| (1000..3000).contains(y))
}

/// A track's length, from its timebase or its frames.
pub fn length(track: &symphonia::core::formats::Track) -> Option<Duration> {
    if let (Some(base), Some(duration)) = (track.time_base, track.duration)
        && let Some(time) = base.calc_duration(duration)
    {
        return Some(Duration::from_secs_f64(time.as_secs_f64().max(0.0)));
    }
    let rate = match &track.codec_params {
        Some(CodecParameters::Audio(params)) => params.sample_rate?,
        _ => return None,
    };
    track.num_frames.map(|frames| Duration::from_secs_f64(frames as f64 / f64::from(rate)))
}

/// "44.1" for 44.1, "48" for 48.0.
fn trim_zeros(n: f64) -> String {
    let text = format!("{n:.1}");
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

/// A picture from a file, kept once in the art folder however many songs
/// carry it: its path.
fn keep_art(art_dir: &Path, visual: &Visual) -> Option<String> {
    let data = &visual.data;
    let ext = match visual.media_type.as_deref() {
        Some("image/png") => "png",
        Some("image/webp") => "webp",
        Some("image/jpeg" | "image/jpg") => "jpg",
        _ if data.starts_with(&[0x89, b'P', b'N', b'G']) => "png",
        _ if data.starts_with(b"RIFF") => "webp",
        _ => "jpg",
    };
    let hash = data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
    let path = art_dir.join(format!("{hash:016x}.{ext}"));
    if !path.is_file() {
        let _ = std::fs::create_dir_all(art_dir);
        // Songs of one album, read at once on two threads, carry the same
        // picture: each writes its own temporary file, and the last rename
        // stands.
        let tmp = art_dir.join(format!("{hash:016x}.{:?}.tmp", std::thread::current().id()).replace(['(', ')'], ""));
        if std::fs::write(&tmp, data).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

/// A picture beside the songs named as their cover ("cover.jpg",
/// "folder.png", "Front.jpg").
fn cover_beside(folder: &Path) -> Option<String> {
    let read = std::fs::read_dir(folder).ok()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in read.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        if !matches!(ext.as_deref(), Some("jpg" | "jpeg" | "png" | "webp")) {
            continue;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        if let Some(rank) = COVERS.iter().position(|c| stem == *c || stem.starts_with(&format!("{c} ")) || stem.starts_with(&format!("{c}_"))) {
            if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                best = Some((rank, path));
            }
        }
    }
    best.map(|(_, path)| path.to_string_lossy().into_owned())
}

/// Deletes pictures no song carries any more.
fn forget_unused_art(art_dir: &Path, entries: &[Entry]) {
    let used: HashSet<&str> = entries.iter().filter_map(|e| e.meta.as_ref()?.art.as_deref()).collect();
    let Ok(read) = std::fs::read_dir(art_dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !used.contains(path.to_string_lossy().as_ref()) {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn mime_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("m4a" | "m4b" | "mp4" | "aac") => "audio/mp4",
        Some("ogg" | "oga" | "opus") => "audio/ogg",
        Some("wav" | "wave") => "audio/wav",
        Some("aif" | "aiff") => "audio/aiff",
        Some("mka") => "audio/x-matroska",
        _ => "audio/unknown",
    }
}

/// A file's format, for people, from its name alone.
fn format_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp3") => "MP3",
        Some("flac") => "FLAC",
        Some("m4a" | "m4b" | "mp4" | "aac") => "AAC",
        Some("ogg" | "oga") => "Vorbis",
        Some("opus") => "Opus",
        Some("wav" | "wave") => "WAV",
        Some("aif" | "aiff") => "AIFF",
        _ => "Audio",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_in_dates() {
        assert_eq!(year_in("2019-05-01"), Some(2019));
        assert_eq!(year_in("2003"), Some(2003));
        assert_eq!(year_in("c. 1999"), Some(1999));
        assert_eq!(year_in("May"), None);
    }

    #[test]
    fn untagged_files_are_named_from_their_file() {
        let track = track_of(r"D:\Music\Aimer - Brave Shine.mp3", &Meta::default());
        assert_eq!(track.title, "Brave Shine");
        assert_eq!(track.artist_line(), "Aimer");
        assert!(track.is_local());
        assert_eq!(track.video_id, id_of(r"d:\music\AIMER - brave shine.MP3"));
        let tagged = Meta { title: "Brave Shine".into(), artist: "Aimer".into(), album: "DAWN".into(), ..Meta::default() };
        let track = track_of(r"D:\Music\03 track.flac", &tagged);
        assert_eq!((track.title.as_str(), track.album.map(|a| a.name)), ("Brave Shine", Some("DAWN".to_owned())));
    }

    #[test]
    fn sorting_ignores_the() {
        assert_eq!(sort_key("The Beatles"), sort_key("beatles"));
    }

    #[test]
    fn search_ranks_titles_first() {
        let mut library = LocalLibrary::new(std::env::temp_dir(), Arc::new(|| {}));
        let entry = |path: &str, title: &str, artist: &str| Entry {
            path: path.into(),
            size: 1,
            modified: 1,
            meta: Some(Meta { title: title.into(), artist: artist.into(), ..Meta::default() }),
        };
        library.take(build(
            vec![
                entry("a.mp3", "Something Else", "Blue Sky"),
                entry("b.mp3", "Blue Sky", "Someone"),
                entry("c.mp3", "Unrelated", "Nobody"),
                entry("d.flac", "トリカゴ", "XX:me"),
            ],
            HashMap::new(),
        ));
        let titles = |query: &str| library.search(query).into_iter().map(|t| t.title).collect::<Vec<_>>();
        assert_eq!(titles("blue sky"), ["Blue Sky", "Something Else"]);
        assert!(titles("  ").is_empty());
        // Japanese by how it reads, and names without their marks.
        assert_eq!(titles("torikago"), ["トリカゴ"]);
        assert_eq!(titles("Tori Kago"), ["トリカゴ"]);
        assert_eq!(titles("xxme"), ["トリカゴ"]);
        assert_eq!(titles("トリカゴ"), ["トリカゴ"]);
    }

    #[test]
    fn loose_spelling() {
        assert_eq!(loose("ＸＸ:me"), "xxme");
        assert_eq!(loose("daarin"), loose("darin"));
        assert_eq!(loose("toukyou"), loose("tokyo"));
    }

    #[test]
    fn found_pictures_go_to_their_album() {
        let mut library = LocalLibrary::new(std::env::temp_dir().join("native-ytm-found-test"), Arc::new(|| {}));
        let meta = Meta { title: "トリカゴ".into(), artist: "XX:me".into(), album: "Vol.1".into(), ..Meta::default() };
        let other = Meta { title: "Escape".into(), ..meta.clone() };
        library.take(build(
            vec![
                Entry { path: "a.flac".into(), size: 1, modified: 1, meta: Some(meta) },
                Entry { path: "b.flac".into(), size: 1, modified: 1, meta: Some(other) },
            ],
            HashMap::new(),
        ));
        let wanted = library.art_wanted();
        assert_eq!(wanted.len(), 1, "one look-up per album");
        assert!(library.art_wanted().is_empty(), "asked once");
        library.art_found(&wanted[0].key, Some("https://example.com/a.jpg".into()));
        assert!(library.songs().iter().all(|s| s.thumbnail.as_deref() == Some("https://example.com/a.jpg")));
        assert_eq!(library.songs()[0].artists[0].id.as_deref(), Some("artist:XX:me"));
    }
}
