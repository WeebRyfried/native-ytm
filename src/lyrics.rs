//! Lyrics kept on disk once found, so a song's lyrics show at once the next
//! time, and offline. A song found to have none is remembered for a while
//! too, then asked about again. Lyrics found while a source could not
//! answer (LRCLIB, or Better Lyrics' server) are kept as provisional: shown
//! at once all the same, and the sources asked again now and then.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::model::Lyrics;

/// Bumped when what was kept before cannot be trusted.
const VERSION: &str = "v3";

/// How long a song without lyrics is taken at its word: three days.
const NONE_FOR_SECS: i64 = 3 * 24 * 60 * 60;

/// How long provisional lyrics stand before LRCLIB is asked again.
const PROVISIONAL_FOR_SECS: i64 = 30 * 60;

#[derive(Serialize, Deserialize)]
struct Kept {
    lyrics: Option<Lyrics>,
    /// When they were found, in Unix seconds.
    at: i64,
    /// Every source answered; not so when LRCLIB could not be asked.
    #[serde(default = "yes")]
    complete: bool,
}

fn yes() -> bool {
    true
}

/// Lyrics from disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// None: the song has none.
    pub lyrics: Option<Lyrics>,
    /// Provisional, and long enough ago that LRCLIB is worth asking again.
    pub recheck: bool,
}

pub struct LyricsCache {
    dir: PathBuf,
}

impl LyricsCache {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        // 0.2.5 kept stopgaps too (YouTube's plain lyrics, or none, when
        // LRCLIB could not be asked), and before 0.3.4 only LRCLIB's and
        // YouTube's were looked for: those are let go, to be looked up
        // again from every source.
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

    /// The song's lyrics as last found. None when they have not been
    /// looked for (or found to be none long enough ago that they may have
    /// come since).
    pub fn get(&self, video_id: &str, synced: bool) -> Option<Found> {
        let bytes = std::fs::read(self.path(video_id, synced)).ok()?;
        let kept: Kept = serde_json::from_slice(&bytes).ok()?;
        let age = unix_now() - kept.at;
        if kept.lyrics.is_none() && (age > NONE_FOR_SECS || !kept.complete) {
            return None;
        }
        Some(Found {
            recheck: !kept.complete && age > PROVISIONAL_FOR_SECS,
            lyrics: kept.lyrics,
        })
    }

    /// Keeps what was found; `complete` when every source answered. A
    /// provisional answer never replaces a complete one.
    pub fn put(&self, video_id: &str, synced: bool, lyrics: &Option<Lyrics>, complete: bool) {
        let path = self.path(video_id, synced);
        if !complete
            && let Some(old) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Kept>(&b).ok())
            && old.complete
        {
            return;
        }
        let kept = Kept {
            lyrics: lyrics.clone(),
            at: unix_now(),
            complete,
        };
        if let Ok(json) = serde_json::to_vec(&kept) {
            crate::settings::write_atomic(&path, &json);
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
            lines: vec![LyricLine { time: 1.5, text: "la la".into(), ..LyricLine::default() }],
            ..Lyrics::default()
        };
        cache.put("abc", true, &Some(lyrics.clone()), true);
        assert_eq!(cache.get("abc", true), Some(Found { lyrics: Some(lyrics.clone()), recheck: false }));
        // The other kind is asked for apart.
        assert_eq!(cache.get("abc", false), None);
        cache.put(r"local:C:\Music\a.flac", false, &None, true);
        assert_eq!(cache.get(r"local:C:\Music\a.flac", false).map(|f| f.lyrics), Some(None));
        // A provisional answer shows, but does not replace a complete one.
        let plain = Lyrics { text: "plain".into(), ..Lyrics::default() };
        cache.put("abc", true, &Some(plain.clone()), false);
        assert_eq!(cache.get("abc", true).and_then(|f| f.lyrics), Some(lyrics));
        cache.put("def", true, &Some(plain.clone()), false);
        assert_eq!(cache.get("def", true), Some(Found { lyrics: Some(plain), recheck: false }));
        // Provisionally none is not kept as none.
        cache.put("ghi", true, &None, false);
        assert_eq!(cache.get("ghi", true), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
