//! What the app remembers of how it is used, beside the settings: the
//! searches made lately, and how far each song's synced lyrics are moved
//! to match the song.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// How many searches are remembered.
const SEARCHES: usize = 12;

/// How far lyrics can be moved either way, in seconds.
const MOST_OFFSET: f32 = 10.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Personal {
    /// Searches made, the latest first.
    pub searches: Vec<String>,
    /// How far each song's synced lyrics are moved, in seconds, by the
    /// song's id: later when positive.
    pub lyric_offsets: HashMap<String, f32>,
}

impl Personal {
    /// Remembers a search, first; the same one made before moves up.
    pub fn add_search(&mut self, query: &str) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return false;
        }
        if self.searches.first().is_some_and(|q| q == query) {
            return false;
        }
        let lower = query.to_lowercase();
        self.searches.retain(|q| q.to_lowercase() != lower);
        self.searches.insert(0, query.to_owned());
        self.searches.truncate(SEARCHES);
        true
    }

    pub fn remove_search(&mut self, query: &str) {
        self.searches.retain(|q| q != query);
    }

    /// How far the song's lyrics are moved, in seconds (later when positive).
    pub fn lyric_offset(&self, video_id: &str) -> f64 {
        self.lyric_offsets.get(video_id).copied().map_or(0.0, f64::from)
    }

    /// Moves the song's lyrics by `delta` seconds; back in time, it is
    /// forgotten. The new offset.
    pub fn nudge_lyrics(&mut self, video_id: &str, delta: f32) -> f32 {
        let now = self.lyric_offsets.get(video_id).copied().unwrap_or(0.0);
        // In tenths, so steps add up to round numbers.
        let next = ((now + delta) * 10.0).round() / 10.0;
        let next = next.clamp(-MOST_OFFSET, MOST_OFFSET);
        if next.abs() < 0.05 {
            self.lyric_offsets.remove(video_id);
            0.0
        } else {
            self.lyric_offsets.insert(video_id.to_owned(), next);
            next
        }
    }

    pub fn reset_lyrics(&mut self, video_id: &str) {
        self.lyric_offsets.remove(video_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_latest_first_without_repeats() {
        let mut personal = Personal::default();
        assert!(personal.add_search("LiSA"));
        assert!(personal.add_search(" Aimer "));
        assert!(!personal.add_search("Aimer"));
        assert!(personal.add_search("lisa"));
        assert_eq!(personal.searches, ["lisa", "Aimer"]);
        assert!(!personal.add_search("   "));
        for n in 0..20 {
            personal.add_search(&format!("song {n}"));
        }
        assert_eq!(personal.searches.len(), SEARCHES);
        assert_eq!(personal.searches[0], "song 19");
        personal.remove_search("song 19");
        assert_eq!(personal.searches[0], "song 18");
    }

    #[test]
    fn lyrics_move_in_steps_and_back_to_none() {
        let mut personal = Personal::default();
        assert_eq!(personal.nudge_lyrics("a", 0.5), 0.5);
        assert_eq!(personal.nudge_lyrics("a", 0.5), 1.0);
        assert!((personal.lyric_offset("a") - 1.0).abs() < 1e-6);
        assert_eq!(personal.nudge_lyrics("a", -0.5), 0.5);
        assert_eq!(personal.nudge_lyrics("a", -0.5), 0.0);
        assert!(personal.lyric_offsets.is_empty());
        for _ in 0..40 {
            personal.nudge_lyrics("b", -0.5);
        }
        assert_eq!(personal.lyric_offset("b"), -10.0);
    }
}
