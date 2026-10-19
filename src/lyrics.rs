//! Lyrics kept on disk once found, so a song's lyrics show at once the next
//! time, and offline. What is found is never let go: a later look adds to
//! it (a source that has the song since, a better one) and cannot take it
//! away, even when every source fails. A song found to have none is
//! remembered for a while too, then asked about again. Lyrics found while a
//! source could not answer (LRCLIB, or Better Lyrics' server) are kept as
//! provisional: shown at once all the same, and the sources asked again.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::model::Lyrics;

/// How long a song without lyrics is taken at its word: three days.
const NONE_FOR_SECS: i64 = 3 * 24 * 60 * 60;

/// How long provisional lyrics stand before the sources are asked again.
const PROVISIONAL_FOR_SECS: i64 = 30 * 60;

/// Lyrics not timed by the word are looked for again this long after, in
/// case a source has the song by then; what was found stays.
const FRESHEN_AFTER_SECS: i64 = 7 * 24 * 60 * 60;

/// The sources asked and what is read from them: lyrics kept before the
/// sources there are now are looked for again (and kept beside what is
/// found). 0: LRCLIB's and YouTube's alone; 1: Better Lyrics' sources too;
/// 2: their singers told apart.
const SOURCES: u32 = 2;

#[derive(Serialize, Deserialize)]
struct Kept {
    lyrics: Option<Lyrics>,
    /// When the sources were last asked, in Unix seconds.
    at: i64,
    /// Every source answered; not so when LRCLIB could not be asked.
    #[serde(default = "yes")]
    complete: bool,
    #[serde(default)]
    sources: u32,
}

fn yes() -> bool {
    true
}

/// Lyrics from disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// None: the song has none.
    pub lyrics: Option<Lyrics>,
    /// Worth asking the sources again (what is here shows meanwhile):
    /// provisional, or found before the sources there are now, or not
    /// timed by the word and a while ago.
    pub recheck: bool,
}

pub struct LyricsCache {
    dir: PathBuf,
}

impl LyricsCache {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self { dir }
    }

    /// One file per song and kind (synced lyrics preferred, or not), named
    /// by a hash: the ids of songs on this PC are paths.
    fn path(&self, video_id: &str, synced: bool) -> PathBuf {
        let hash = Sha256::digest(format!("{video_id}|{synced}").as_bytes());
        let name: String = hash.iter().take(12).map(|b| format!("{b:02x}")).collect();
        self.dir.join(format!("{name}.json"))
    }

    fn read(&self, video_id: &str, synced: bool) -> Option<Kept> {
        let bytes = std::fs::read(self.path(video_id, synced)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// The song's lyrics as last found. None when they have not been
    /// looked for (or found to be none long enough ago that they may have
    /// come since).
    pub fn get(&self, video_id: &str, synced: bool) -> Option<Found> {
        let kept = self.read(video_id, synced)?;
        let age = unix_now() - kept.at;
        if kept.lyrics.is_none() && (age > NONE_FOR_SECS || !kept.complete || kept.sources < SOURCES) {
            return None;
        }
        let by_word = kept.lyrics.as_ref().is_some_and(Lyrics::by_word);
        Some(Found {
            recheck: (!kept.complete && age > PROVISIONAL_FOR_SECS) || kept.sources < SOURCES || (synced && !by_word && age > FRESHEN_AFTER_SECS),
            lyrics: kept.lyrics,
        })
    }

    /// Keeps what was found, together with what was kept before (see
    /// `betterlyrics::merge`); `complete` when every source answered. What
    /// is kept now, to show.
    pub fn put(&self, video_id: &str, synced: bool, lyrics: &Option<Lyrics>, complete: bool) -> Option<Lyrics> {
        let path = self.path(video_id, synced);
        let old = self.read(video_id, synced);
        let merged = crate::api::betterlyrics::merge(old.as_ref().and_then(|k| k.lyrics.as_ref()), lyrics.clone());
        // Provisionally none is not kept as none.
        if merged.is_none() && !complete {
            return None;
        }
        let kept = Kept {
            lyrics: merged.clone(),
            // A look that did not finish leaves the last full one's time,
            // so it is asked again in its turn.
            at: match &old {
                Some(old) if !complete => old.at,
                _ => unix_now(),
            },
            complete: complete || old.as_ref().is_some_and(|o| o.complete && o.sources >= SOURCES),
            sources: SOURCES,
        };
        if let Ok(json) = serde_json::to_vec(&kept) {
            crate::settings::write_atomic(&path, &json);
        }
        merged
    }

    /// The kept lyrics changed in place (their lines translated), when
    /// there are kept lyrics.
    pub fn update(&self, video_id: &str, synced: bool, lyrics: &Lyrics) {
        let Some(mut kept) = self.read(video_id, synced) else {
            return;
        };
        kept.lyrics = Some(lyrics.clone());
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

    fn timed(provider: &str) -> Lyrics {
        Lyrics {
            text: "la la".into(),
            source: Some(provider.into()),
            provider: Some(provider.into()),
            lines: vec![
                LyricLine { time: 1.5, text: "la la".into(), ..LyricLine::default() },
                LyricLine { time: 3.0, text: "la".into(), ..LyricLine::default() },
            ],
            ..Lyrics::default()
        }
    }

    #[test]
    fn keeps_lyrics_and_their_absence() {
        let dir = std::env::temp_dir().join(format!("native-ytm-lyrics-{}", std::process::id()));
        let cache = LyricsCache::new(dir.clone());
        assert_eq!(cache.get("abc", true), None);
        let lyrics = timed("LRCLIB");
        cache.put("abc", true, &Some(lyrics.clone()), true);
        assert_eq!(cache.get("abc", true), Some(Found { lyrics: Some(lyrics.clone()), recheck: false }));
        // The other kind is asked for apart.
        assert_eq!(cache.get("abc", false), None);
        cache.put(r"local:C:\Music\a.flac", false, &None, true);
        assert_eq!(cache.get(r"local:C:\Music\a.flac", false).map(|f| f.lyrics), Some(None));
        // A later look that finds nothing, or fails, takes nothing away.
        assert_eq!(cache.put("abc", true, &None, true), Some(lyrics.clone()));
        assert_eq!(cache.put("abc", true, &None, false), Some(lyrics.clone()));
        assert_eq!(cache.get("abc", true).and_then(|f| f.lyrics), Some(lyrics.clone()));
        // One that finds another source adds it, the better first.
        let more = cache.put("abc", true, &Some(timed("Musixmatch")), true).unwrap();
        assert_eq!(more.provider.as_deref(), Some("LRCLIB"));
        assert_eq!(more.others.len(), 1);
        // Provisional lyrics show, and are asked about again in a while.
        let plain = Lyrics { text: "plain".into(), provider: Some("YouTube Music".into()), ..Lyrics::default() };
        cache.put("def", true, &Some(plain.clone()), false);
        assert_eq!(cache.get("def", true), Some(Found { lyrics: Some(plain), recheck: false }));
        // Provisionally none is not kept as none.
        assert_eq!(cache.put("ghi", true, &None, false), None);
        assert_eq!(cache.get("ghi", true), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lyrics_kept_before_the_sources_there_are_now_are_looked_for_again() {
        let dir = std::env::temp_dir().join(format!("native-ytm-lyrics-old-{}", std::process::id()));
        let cache = LyricsCache::new(dir.clone());
        let old = serde_json::json!({ "lyrics": timed("LRCLIB"), "at": unix_now(), "complete": true });
        std::fs::write(cache.path("abc", true), serde_json::to_vec(&old).unwrap()).unwrap();
        let found = cache.get("abc", true).unwrap();
        assert!(found.recheck);
        assert_eq!(found.lyrics.unwrap().provider.as_deref(), Some("LRCLIB"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
