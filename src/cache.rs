//! Songs kept on disk: each song heard in full is written to the cache, the
//! best quality heard of it, and plays from there next time (at once, and
//! without the network). The least recently played go once there are more
//! than the limit Settings sets.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

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
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    entries: Vec<Entry>,
}

pub struct AudioCache {
    dir: Option<PathBuf>,
    index: Mutex<Index>,
    limit: Mutex<usize>,
}

impl AudioCache {
    /// The cache in `dir`, keeping at most `limit` songs (none: off).
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
        })
    }

    pub fn enabled(&self) -> bool {
        self.dir.is_some() && *self.limit.lock() > 0
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

    /// Keeps a whole downloaded file, unless the cache has it as good
    /// already. The writing happens on a thread of its own.
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
        let _ = std::thread::Builder::new().name("cache".into()).spawn(move || cache.write(&video_id, &stream, &bytes));
    }

    fn write(&self, video_id: &str, stream: &StreamInfo, bytes: &[u8]) {
        let Some(dir) = &self.dir else { return };
        let ext = if stream.mime.contains("webm") { "webm" } else { "m4a" };
        // Named after the video and its quality, so a better copy never
        // overwrites a file that may be playing.
        let file = format!("{}-{}.{ext}", safe(video_id), stream.kbps.unwrap_or(0));
        let path = dir.join(&file);
        crate::settings::write_atomic(&path, bytes);
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) != bytes.len() as u64 {
            return;
        }
        let old = {
            let mut index = self.index.lock();
            let old: Vec<Entry> = index.entries.iter().filter(|e| e.video_id == video_id).cloned().collect();
            index.entries.retain(|e| e.video_id != video_id);
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
            });
            old
        };
        for entry in old.into_iter().filter(|e| e.file != file) {
            let _ = std::fs::remove_file(dir.join(entry.file));
        }
        log::info!("kept {video_id} on disk ({}, {:.1} MB)", stream.quality, bytes.len() as f64 / 1_048_576.0);
        self.evict();
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
        self.save();
    }

    pub fn set_limit(&self, limit: usize) {
        *self.limit.lock() = limit;
        self.evict();
    }

    /// Songs kept, and their size in bytes.
    pub fn usage(&self) -> (usize, u64) {
        let index = self.index.lock();
        (index.entries.len(), index.entries.iter().map(|e| e.bytes).sum())
    }

    pub fn clear(&self) {
        let Some(dir) = &self.dir else { return };
        let gone = std::mem::take(&mut self.index.lock().entries);
        for entry in gone {
            let _ = std::fs::remove_file(dir.join(entry.file));
        }
        self.save();
    }

    /// Down to the limit, the least recently played first.
    fn evict(&self) {
        let Some(dir) = &self.dir else { return };
        let limit = *self.limit.lock();
        let gone: Vec<Entry> = {
            let mut index = self.index.lock();
            if index.entries.len() <= limit {
                Vec::new()
            } else {
                index.entries.sort_by_key(|e| std::cmp::Reverse(e.played));
                index.entries.split_off(limit)
            }
        };
        for entry in &gone {
            let _ = std::fs::remove_file(dir.join(&entry.file));
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

    #[test]
    fn keeps_the_best_and_the_most_recent() {
        let dir = std::env::temp_dir().join(format!("native-ytm-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = AudioCache::open(dir.clone(), 2);
        cache.write("a", &stream(130), b"aaaa");
        cache.write("a", &stream(257), b"AAAA");
        assert!(cache.has_at_least("a", 257));
        assert_eq!(cache.lookup("a").map(|s| s.quality), Some("Opus 257 kb/s".into()));
        cache.write("b", &stream(130), b"bbbb");
        // "a" plays again later; "c" makes three, and the oldest, "b", goes.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        cache.lookup("a");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        cache.write("c", &stream(130), b"cccc");
        assert!(cache.lookup("b").is_none());
        assert!(cache.lookup("a").is_some() && cache.lookup("c").is_some());
        // Reopened, it remembers.
        let again = AudioCache::open(dir.clone(), 2);
        assert_eq!(again.usage().0, 2);
        again.clear();
        assert_eq!(again.usage(), (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
