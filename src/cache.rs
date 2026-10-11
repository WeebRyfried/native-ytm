//! Songs on disk, which play at once and without the network. Two kinds:
//!
//! - kept: each song heard in full, at the best quality heard. The least
//!   recently played go once there are more than the limit Settings sets.
//! - downloaded: songs saved for listening offline (a song, an album, a
//!   playlist). They stay until removed, whatever the limit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::api::model::Track;
use crate::player::StreamInfo;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    video_id: String,
    /// The file's name in the cache folder.
    file: String,
    mime: String,
    quality: String,
    kbps: Option<u32>,
    loudness_db: Option<f32>,
    duration_ms: Option<u64>,
    bytes: u64,
    /// When it last played, in Unix seconds.
    played: i64,
    /// Downloaded for offline listening: never evicted.
    #[serde(default)]
    saved: bool,
    /// When it was downloaded, in Unix seconds.
    #[serde(default)]
    saved_at: i64,
    /// The song's names and artwork, for the Cached page.
    #[serde(default)]
    track: Option<Track>,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    entries: Vec<Entry>,
}

/// How a song stands, for the mark beside its name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    /// Downloaded for offline listening.
    Saved,
    /// Kept from a recent play: it plays offline too, for now.
    Kept,
    /// Downloading, this far.
    Saving(f32),
    /// Waiting its turn to download.
    Waiting,
}

/// What the cache holds, for Settings: songs and bytes of each kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub kept: usize,
    pub kept_bytes: u64,
    pub saved: usize,
    pub saved_bytes: u64,
}

pub struct AudioCache {
    dir: Option<PathBuf>,
    index: Mutex<Index>,
    limit: Mutex<usize>,
    /// The songs that started playing lately, so a file kept once whole
    /// carries the song's names.
    names: Mutex<HashMap<String, Track>>,
    /// Bumped whenever what is on disk changes, so views rebuild marks.
    generation: AtomicU64,
}

impl AudioCache {
    /// The cache in `dir`, keeping at most `limit` songs from plays.
    pub fn open(dir: PathBuf, limit: usize) -> Arc<Self> {
        let _ = std::fs::create_dir_all(&dir);
        let mut index: Index = crate::settings::load(&dir.join("index.json"));
        // Files that went missing, or were left half written, are forgotten.
        index.entries.retain(|e| {
            std::fs::metadata(dir.join(&e.file)).is_ok_and(|m| m.len() == e.bytes)
        });
        let cache = Arc::new(Self {
            dir: Some(dir),
            index: Mutex::new(index),
            limit: Mutex::new(limit),
            names: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(1),
        });
        cache.clean_strays();
        cache.evict();
        cache
    }

    /// No cache at all (the probe tool).
    pub fn disabled() -> Arc<Self> {
        Arc::new(Self {
            dir: None,
            index: Mutex::new(Index::default()),
            limit: Mutex::new(0),
            names: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
        })
    }

    /// Whether songs heard are kept.
    pub fn enabled(&self) -> bool {
        self.dir.is_some() && *self.limit.lock() > 0
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    fn changed(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Remembers a song's names, for when its file is kept, and gives them
    /// to a kept file that has none (kept before names were).
    pub fn note(&self, track: &Track) {
        {
            let mut names = self.names.lock();
            if names.len() > 300 {
                names.clear();
            }
            names.insert(track.video_id.clone(), track.clone());
        }
        let named = {
            let mut index = self.index.lock();
            match index.entries.iter_mut().find(|e| e.video_id == track.video_id && e.track.is_none()) {
                Some(entry) => {
                    entry.track = Some(track.clone());
                    true
                }
                None => false,
            }
        };
        if named {
            self.changed();
        }
    }

    /// The song from the cache, as a stream the player opens from disk.
    pub fn lookup(&self, video_id: &str) -> Option<StreamInfo> {
        let dir = self.dir.as_ref()?;
        let mut index = self.index.lock();
        let entry = index.entries.iter_mut().find(|e| e.video_id == video_id)?;
        entry.played = unix_now();
        let path = dir.join(&entry.file);
        Some(StreamInfo {
            url: format!("file:{}", path.display()),
            headers: Vec::new(),
            content_length: entry.bytes,
            duration: entry.duration_ms.map(std::time::Duration::from_millis),
            loudness_db: entry.loudness_db,
            mime: entry.mime.clone(),
            quality: entry.quality.clone(),
            kbps: entry.kbps,
            cache_key: None,
        })
    }

    /// Whether the cache already has the song at `kbps` or better.
    pub fn has_at_least(&self, video_id: &str, kbps: u32) -> bool {
        self.index
            .lock()
            .entries
            .iter()
            .any(|e| e.video_id == video_id && e.kbps.unwrap_or(0) >= kbps)
    }

    /// Keeps a song heard in full, unless the cache has it as good already.
    /// The writing happens on a thread of its own.
    pub fn store(self: &Arc<Self>, video_id: &str, stream: &StreamInfo, bytes: Vec<u8>) {
        if !self.enabled() || bytes.is_empty() {
            return;
        }
        let kbps = stream.kbps.unwrap_or(0);
        if self.has_at_least(video_id, kbps.max(1)) {
            return;
        }
        let cache = self.clone();
        let (video_id, stream) = (video_id.to_owned(), stream.clone());
        let _ = std::thread::Builder::new()
            .name("cache".into())
            .spawn(move || cache.write(&video_id, &stream, &bytes, false, None));
    }

    /// Keeps a downloaded song for offline listening, on a thread of its
    /// own; `done` hears whether it was written.
    pub fn save_offline(
        self: &Arc<Self>,
        track: Track,
        stream: StreamInfo,
        bytes: Vec<u8>,
        done: impl FnOnce(bool) + Send + 'static,
    ) {
        let cache = self.clone();
        let spawned = std::thread::Builder::new().name("cache".into()).spawn(move || {
            let video_id = track.video_id.clone();
            done(!bytes.is_empty() && cache.write(&video_id, &stream, &bytes, true, Some(track)));
        });
        if let Err(error) = spawned {
            log::warn!("cannot save a song for offline: {error}");
        }
    }

    /// Marks a song already on disk as downloaded. False when it is not
    /// on disk.
    pub fn pin(&self, track: &Track) -> bool {
        let found = {
            let mut index = self.index.lock();
            match index.entries.iter_mut().find(|e| e.video_id == track.video_id) {
                Some(entry) => {
                    if !entry.saved {
                        entry.saved = true;
                        entry.saved_at = unix_now();
                    }
                    entry.track = Some(track.clone());
                    true
                }
                None => false,
            }
        };
        if found {
            self.changed();
            self.save();
        }
        found
    }

    /// No longer downloaded: the songs stay as kept ones, the limit
    /// permitting.
    pub fn unsave(&self, video_ids: &[String]) {
        {
            let mut index = self.index.lock();
            for entry in index.entries.iter_mut().filter(|e| video_ids.contains(&e.video_id)) {
                entry.saved = false;
                entry.played = unix_now();
            }
        }
        self.changed();
        self.evict();
    }

    /// Writes the file and its entry. True once both are in place.
    fn write(&self, video_id: &str, stream: &StreamInfo, bytes: &[u8], saved: bool, track: Option<Track>) -> bool {
        let Some(dir) = &self.dir else { return false };
        let ext = if stream.mime.contains("webm") { "webm" } else { "m4a" };
        // Named after the video and its quality, so a better copy never
        // overwrites a file that may be playing.
        let file = format!("{}-{}.{ext}", safe(video_id), stream.kbps.unwrap_or(0));
        let path = dir.join(&file);
        crate::settings::write_atomic(&path, bytes);
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) != bytes.len() as u64 {
            return false;
        }
        let named = track.or_else(|| self.names.lock().get(video_id).cloned());
        let old = {
            let mut index = self.index.lock();
            let old: Vec<Entry> = index.entries.iter().filter(|e| e.video_id == video_id).cloned().collect();
            index.entries.retain(|e| e.video_id != video_id);
            // A better copy of a downloaded song is downloaded too.
            let was_saved = old.iter().find(|e| e.saved);
            index.entries.push(Entry {
                video_id: video_id.to_owned(),
                file: file.clone(),
                mime: stream.mime.clone(),
                quality: stream.quality.clone(),
                kbps: stream.kbps,
                loudness_db: stream.loudness_db,
                duration_ms: stream.duration.map(|d| d.as_millis() as u64),
                bytes: bytes.len() as u64,
                played: unix_now(),
                saved: saved || was_saved.is_some(),
                saved_at: was_saved.map_or_else(unix_now, |e| e.saved_at),
                track: named.or_else(|| old.iter().find_map(|e| e.track.clone())),
            });
            old
        };
        for entry in old.into_iter().filter(|e| e.file != file) {
            let _ = std::fs::remove_file(dir.join(entry.file));
        }
        log::info!(
            "{} {video_id} on disk ({}, {:.1} MB)",
            if saved { "downloaded" } else { "kept" },
            stream.quality,
            bytes.len() as f64 / 1_048_576.0
        );
        self.changed();
        self.evict();
        true
    }

    /// Forgets a song whose file would not play.
    pub fn remove(&self, video_id: &str) {
        let Some(dir) = &self.dir else { return };
        let gone: Vec<Entry> = {
            let mut index = self.index.lock();
            let gone = index.entries.iter().filter(|e| e.video_id == video_id).cloned().collect();
            index.entries.retain(|e| e.video_id != video_id);
            gone
        };
        for entry in gone {
            let _ = std::fs::remove_file(dir.join(entry.file));
        }
        self.changed();
        self.save();
    }

    pub fn set_limit(&self, limit: usize) {
        *self.limit.lock() = limit;
        self.evict();
    }

    pub fn usage(&self) -> Usage {
        let index = self.index.lock();
        let mut usage = Usage::default();
        for entry in &index.entries {
            if entry.saved {
                usage.saved += 1;
                usage.saved_bytes += entry.bytes;
            } else {
                usage.kept += 1;
                usage.kept_bytes += entry.bytes;
            }
        }
        usage
    }

    /// Deletes the songs kept from plays (`saved` false) or the downloaded
    /// ones (`saved` true).
    pub fn clear(&self, saved: bool) {
        let Some(dir) = &self.dir else { return };
        let gone: Vec<Entry> = {
            let mut index = self.index.lock();
            let (gone, stay) = std::mem::take(&mut index.entries).into_iter().partition(|e| e.saved == saved);
            index.entries = stay;
            gone
        };
        for entry in gone {
            let _ = std::fs::remove_file(dir.join(entry.file));
        }
        self.changed();
        self.save();
    }

    /// Each song on disk and how it stands.
    pub fn marks(&self) -> HashMap<String, Mark> {
        self.index
            .lock()
            .entries
            .iter()
            .map(|e| (e.video_id.clone(), if e.saved { Mark::Saved } else { Mark::Kept }))
            .collect()
    }

    /// The downloaded songs, the latest first.
    pub fn saved_tracks(&self) -> Vec<Track> {
        let index = self.index.lock();
        let mut saved: Vec<&Entry> = index.entries.iter().filter(|e| e.saved && e.track.is_some()).collect();
        saved.sort_by_key(|e| std::cmp::Reverse(e.saved_at));
        saved.into_iter().filter_map(|e| e.track.clone()).collect()
    }

    /// The songs kept from plays, the most recently played first.
    pub fn kept_tracks(&self) -> Vec<Track> {
        let index = self.index.lock();
        let mut kept: Vec<&Entry> = index.entries.iter().filter(|e| !e.saved && e.track.is_some()).collect();
        kept.sort_by_key(|e| std::cmp::Reverse(e.played));
        kept.into_iter().filter_map(|e| e.track.clone()).collect()
    }

    /// Down to the limit, the least recently played first. Downloaded
    /// songs neither count nor go.
    fn evict(&self) {
        let Some(dir) = &self.dir else { return };
        let limit = *self.limit.lock();
        let gone: Vec<Entry> = {
            let mut index = self.index.lock();
            let kept = index.entries.iter().filter(|e| !e.saved).count();
            if kept <= limit {
                Vec::new()
            } else {
                let (mut kept, saved): (Vec<Entry>, Vec<Entry>) =
                    std::mem::take(&mut index.entries).into_iter().partition(|e| !e.saved);
                // Newest first; within the same second, the one written last.
                kept.reverse();
                kept.sort_by_key(|e| std::cmp::Reverse(e.played));
                let gone = kept.split_off(limit);
                index.entries = saved;
                index.entries.extend(kept);
                gone
            }
        };
        for entry in &gone {
            let _ = std::fs::remove_file(dir.join(&entry.file));
        }
        if !gone.is_empty() {
            self.changed();
        }
        self.save();
    }

    /// Saves the index (which songs, and when each last played).
    pub fn save(&self) {
        let Some(dir) = &self.dir else { return };
        let index = self.index.lock();
        crate::settings::save(&dir.join("index.json"), &*index);
    }

    /// Files the index does not know (a crash mid-write) are removed.
    fn clean_strays(&self) {
        let Some(dir) = &self.dir else { return };
        let known: Vec<String> = self.index.lock().entries.iter().map(|e| e.file.clone()).collect();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name != "index.json" && !known.contains(&name) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// A video id as a file name: they are already safe, but nothing from the
/// network goes into a path unchecked.
fn safe(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The path of a `file:` stream address.
pub fn file_of(url: &str) -> Option<&Path> {
    url.strip_prefix("file:").map(Path::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(kbps: u32) -> StreamInfo {
        StreamInfo {
            url: String::new(),
            headers: Vec::new(),
            content_length: 4,
            duration: None,
            loudness_db: Some(-1.0),
            mime: "audio/webm".into(),
            quality: format!("Opus {kbps} kb/s"),
            kbps: Some(kbps),
            cache_key: None,
        }
    }

    fn track(id: &str) -> Track {
        Track {
            video_id: id.into(),
            title: format!("Song {id}"),
            ..Track::default()
        }
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("native-ytm-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn keeps_the_best_and_the_most_recent() {
        let dir = dir("kept");
        let cache = AudioCache::open(dir.clone(), 2);
        cache.write("a", &stream(130), b"aaaa", false, None);
        cache.write("a", &stream(257), b"AAAA", false, None);
        assert!(cache.has_at_least("a", 257));
        assert_eq!(cache.lookup("a").map(|s| s.quality), Some("Opus 257 kb/s".into()));
        cache.write("b", &stream(130), b"bbbb", false, None);
        // "a" plays again later; "c" makes three, and the oldest, "b", goes.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        cache.lookup("a");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        cache.write("c", &stream(130), b"cccc", false, None);
        assert!(cache.lookup("b").is_none());
        assert!(cache.lookup("a").is_some() && cache.lookup("c").is_some());
        // Reopened, it remembers.
        let again = AudioCache::open(dir.clone(), 2);
        assert_eq!(again.usage().kept, 2);
        again.clear(false);
        assert_eq!(again.usage(), Usage::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn downloads_stay_whatever_the_limit() {
        let dir = dir("saved");
        let cache = AudioCache::open(dir.clone(), 1);
        assert!(cache.write("s", &stream(257), b"ssss", true, Some(track("s"))));
        cache.note(&track("k"));
        cache.write("k", &stream(130), b"kkkk", false, None);
        cache.write("m", &stream(130), b"mmmm", false, None);
        // One kept song at most, and the download besides.
        let usage = cache.usage();
        assert_eq!((usage.saved, usage.kept), (1, 1));
        assert!(cache.lookup("s").is_some());
        assert_eq!(cache.marks().get("s"), Some(&Mark::Saved));
        assert_eq!(cache.saved_tracks()[0].title, "Song s");
        // A song kept from a play, downloaded: it carries the names it was
        // noted with, and stays downloaded through a better copy.
        assert!(cache.pin(&track("m")));
        cache.write("m", &stream(257), b"MMMM", false, None);
        assert_eq!(cache.marks().get("m"), Some(&Mark::Saved));
        // No longer downloaded, it is a kept song again, under the limit.
        cache.unsave(&["s".into(), "m".into()]);
        assert_eq!(cache.usage().saved, 0);
        assert_eq!(cache.usage().kept, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
