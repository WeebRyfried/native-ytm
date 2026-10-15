//! Lyrics kept on disk once found, so a song's lyrics show at once the next
//! time, and offline. A song found to have none is remembered for a while
//! too, then asked about again.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::model::Lyrics;

/// Bumped when what was kept before cannot be trusted.
const VERSION: &str = "v2";

/// How long a song without lyrics is taken at its word: three days.
const NONE_FOR_SECS: i64 = 3 * 24 * 60 * 60;

#[derive(Serialize, Deserialize)]
struct Kept {
    lyrics: Option<Lyrics>,
    /// When they were found, in Unix seconds.
    at: i64,
}

pub struct LyricsCache {
    dir: PathBuf,
}

impl LyricsCache {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        // 0.2.5 kept stopgaps too (YouTube's plain lyrics, or none, when
        // LRCLIB could not be asked): those are let go.
        let marker = dir.join(VERSION);
        if !marker.exists() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            let _ = std::fs::write(&marker, b"");
        }
        Self { dir }
    }

    /// One file per song and kind (synced lyrics preferred, or not), named
    /// by a hash: the ids of songs on this PC are paths.
    fn path(&self, video_id: &str, synced: bool) -> PathBuf {
        let hash = Sha256::digest(format!("{video_id}|{synced}").as_bytes());
        let name: String = hash.iter().take(12).map(|b| format!("{b:02x}")).collect();
        self.dir.join(format!("{name}.json"))
    }

    /// The song's lyrics as last found: `Some(None)` when it has none.
    /// None when they have not been looked for (or long enough ago that
    /// they may have come since).
    pub fn get(&self, video_id: &str, synced: bool) -> Option<Option<Lyrics>> {
        let bytes = std::fs::read(self.path(video_id, synced)).ok()?;
        let kept: Kept = serde_json::from_slice(&bytes).ok()?;
        if kept.lyrics.is_none() && unix_now() - kept.at > NONE_FOR_SECS {
            return None;
        }
        Some(kept.lyrics)
    }

    pub fn put(&self, video_id: &str, synced: bool, lyrics: &Option<Lyrics>) {
        let kept = Kept {
            lyrics: lyrics.clone(),
            at: unix_now(),
        };
        if let Ok(json) = serde_json::to_vec(&kept) {
            crate::settings::write_atomic(&self.path(video_id, synced), &json);
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::model::LyricLine;

    #[test]
    fn keeps_lyrics_and_their_absence() {
        let dir = std::env::temp_dir().join(format!("native-ytm-lyrics-{}", std::process::id()));
        let cache = LyricsCache::new(dir.clone());
        assert_eq!(cache.get("abc", true), None);
        let lyrics = Lyrics {
            text: "la la".into(),
            source: Some("LRCLIB".into()),
            lines: vec![LyricLine { time: 1.5, text: "la la".into() }],
        };
        cache.put("abc", true, &Some(lyrics.clone()));
        assert_eq!(cache.get("abc", true), Some(Some(lyrics)));
        // The other kind is asked for apart.
        assert_eq!(cache.get("abc", false), None);
        cache.put(r"local:C:\Music\a.flac", false, &None);
        assert_eq!(cache.get(r"local:C:\Music\a.flac", false), Some(None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
