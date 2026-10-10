//! The play queue: what plays, in which order, and what comes next.
//!
//! Every entry has an id of its own, so the same song twice is two entries
//! and an entry keeps its identity while the list is shuffled or edited.

use crate::api::model::Track;
use crate::settings::Repeat;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub uid: u64,
    pub track: Track,
}

/// Where the queue came from, for fetching more of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Source {
    /// The playlist or album, or the radio's playlist id.
    pub playlist_id: Option<String>,
    /// A continuation token for more of it.
    pub continuation: Option<String>,
    /// More comes from the watch endpoint (a radio or a mix), rather than
    /// from the playlist's page.
    pub watch: bool,
    /// The title shown above the queue.
    pub title: Option<String>,
}

#[derive(Default)]
pub struct Queue {
    pub entries: Vec<Entry>,
    pub current: Option<usize>,
    pub source: Source,
    pub shuffle: bool,
    /// The order before shuffling, by uid, to restore.
    unshuffled: Option<Vec<u64>>,
    next_uid: u64,
}

impl Queue {
    fn entry(&mut self, track: Track) -> Entry {
        self.next_uid += 1;
        Entry {
            uid: self.next_uid,
            track,
        }
    }

    pub fn current(&self) -> Option<&Entry> {
        self.current.and_then(|i| self.entries.get(i))
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.current().map(|e| &e.track)
    }

    pub fn index_of(&self, uid: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.uid == uid)
    }

    /// Replaces the queue with `tracks`, playing `start`.
    pub fn replace(&mut self, tracks: Vec<Track>, start: usize, source: Source) {
        self.entries = tracks.into_iter().map(|t| self.entry(t)).collect();
        self.current = (!self.entries.is_empty()).then(|| start.min(self.entries.len() - 1));
        self.source = source;
        self.unshuffled = None;
        if self.shuffle {
            self.shuffle_upcoming();
        }
    }

    /// Adds after whatever plays now.
    pub fn play_next(&mut self, tracks: Vec<Track>) {
        let at = self.current.map_or(0, |i| i + 1);
        let new: Vec<Entry> = tracks.into_iter().map(|t| self.entry(t)).collect();
        if let Some(order) = &mut self.unshuffled {
            let anchor = self.current.and_then(|i| self.entries.get(i)).map(|e| e.uid);
            let pos = anchor
                .and_then(|uid| order.iter().position(|&u| u == uid))
                .map_or(0, |p| p + 1);
            for (k, e) in new.iter().enumerate() {
                order.insert(pos + k, e.uid);
            }
        }
        self.entries.splice(at..at, new);
        if self.current.is_none() && !self.entries.is_empty() {
            self.current = Some(0);
        }
    }

    pub fn append(&mut self, tracks: Vec<Track>) {
        for track in tracks {
            let entry = self.entry(track);
            if let Some(order) = &mut self.unshuffled {
                order.push(entry.uid);
            }
            self.entries.push(entry);
        }
        if self.current.is_none() && !self.entries.is_empty() {
            self.current = Some(0);
        }
    }

    /// Appends what a radio or playlist continuation brought, leaving out
    /// songs the queue already has coming up.
    pub fn extend_unique(&mut self, tracks: Vec<Track>) -> usize {
        let start = self.current.unwrap_or(0);
        let fresh: Vec<Track> = tracks
            .into_iter()
            .filter(|t| {
                !self.entries[start.min(self.entries.len())..]
                    .iter()
                    .any(|e| e.track.video_id == t.video_id)
            })
            .collect();
        let added = fresh.len();
        self.append(fresh);
        added
    }

    pub fn remove(&mut self, uid: u64) -> bool {
        let Some(index) = self.index_of(uid) else {
            return false;
        };
        let was_current = self.current == Some(index);
        self.entries.remove(index);
        if let Some(order) = &mut self.unshuffled {
            order.retain(|&u| u != uid);
        }
        match self.current {
            Some(c) if c > index => self.current = Some(c - 1),
            Some(c) if c == index => {
                self.current = (!self.entries.is_empty()).then(|| c.min(self.entries.len() - 1));
            }
            _ => {}
        }
        was_current
    }

    pub fn move_entry(&mut self, from: usize, to: usize) {
        if from >= self.entries.len() || to >= self.entries.len() || from == to {
            return;
        }
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
        if let Some(c) = self.current {
            self.current = Some(if c == from {
                to
            } else if from < c && to >= c {
                c - 1
            } else if from > c && to <= c {
                c + 1
            } else {
                c
            });
        }
    }

    /// Keeps only what plays now.
    pub fn clear_upcoming(&mut self) {
        if let Some(c) = self.current {
            self.entries.truncate(c + 1);
            if let Some(order) = &mut self.unshuffled {
                let keep: Vec<u64> = self.entries.iter().map(|e| e.uid).collect();
                order.retain(|u| keep.contains(u));
            }
        } else {
            self.entries.clear();
        }
        self.source.continuation = None;
    }

    /// The index after the current one, under the repeat mode. `manual` is
    /// the Next button, which skips even a repeated track.
    pub fn next_index(&self, repeat: Repeat, manual: bool) -> Option<usize> {
        let c = self.current?;
        if repeat == Repeat::One && !manual {
            return Some(c);
        }
        if c + 1 < self.entries.len() {
            Some(c + 1)
        } else if repeat != Repeat::Off && !self.entries.is_empty() {
            Some(0)
        } else {
            None
        }
    }

    pub fn previous_index(&self, repeat: Repeat) -> Option<usize> {
        let c = self.current?;
        if c > 0 {
            Some(c - 1)
        } else if repeat == Repeat::All && !self.entries.is_empty() {
            Some(self.entries.len() - 1)
        } else {
            None
        }
    }

    pub fn upcoming(&self) -> usize {
        self.current
            .map_or(self.entries.len(), |c| self.entries.len().saturating_sub(c + 1))
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        if on {
            self.shuffle_upcoming();
        } else if let Some(order) = self.unshuffled.take() {
            let current = self.current().map(|e| e.uid);
            let mut rest: Vec<Entry> = std::mem::take(&mut self.entries);
            let mut sorted = Vec::with_capacity(rest.len());
            for uid in order {
                if let Some(i) = rest.iter().position(|e| e.uid == uid) {
                    sorted.push(rest.swap_remove(i));
                }
            }
            sorted.extend(rest);
            self.entries = sorted;
            self.current = current.and_then(|uid| self.index_of(uid));
        }
    }

    fn shuffle_upcoming(&mut self) {
        if self.unshuffled.is_none() {
            self.unshuffled = Some(self.entries.iter().map(|e| e.uid).collect());
        }
        let start = self.current.map_or(0, |c| c + 1);
        if start >= self.entries.len() {
            return;
        }
        let upcoming = &mut self.entries[start..];
        fastrand::shuffle(upcoming);
    }

    pub fn tracks(&self) -> Vec<Track> {
        self.entries.iter().map(|e| e.track.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks(ids: &[&str]) -> Vec<Track> {
        ids.iter()
            .map(|id| Track {
                video_id: (*id).into(),
                ..Track::default()
            })
            .collect()
    }

    fn ids(q: &Queue) -> Vec<String> {
        q.entries.iter().map(|e| e.track.video_id.clone()).collect()
    }

    #[test]
    fn play_next_goes_after_current() {
        let mut q = Queue::default();
        q.replace(tracks(&["a", "b", "c"]), 1, Source::default());
        q.play_next(tracks(&["x"]));
        assert_eq!(ids(&q), ["a", "b", "x", "c"]);
        assert_eq!(q.current, Some(1));
    }

    #[test]
    fn repeat_modes() {
        let mut q = Queue::default();
        q.replace(tracks(&["a", "b"]), 1, Source::default());
        assert_eq!(q.next_index(Repeat::Off, false), None);
        assert_eq!(q.next_index(Repeat::All, false), Some(0));
        assert_eq!(q.next_index(Repeat::One, false), Some(1));
        // Next while one song repeats moves on, round the queue like Repeat All.
        assert_eq!(q.next_index(Repeat::One, true), Some(0));
    }

    #[test]
    fn shuffle_keeps_current_and_restores_order() {
        let mut q = Queue::default();
        q.replace(tracks(&["a", "b", "c", "d", "e", "f"]), 2, Source::default());
        q.set_shuffle(true);
        assert_eq!(q.current_track().unwrap().video_id, "c");
        assert_eq!(&ids(&q)[..3], ["a", "b", "c"]);
        q.set_shuffle(false);
        assert_eq!(ids(&q), ["a", "b", "c", "d", "e", "f"]);
        assert_eq!(q.current, Some(2));
    }

    #[test]
    fn removing_before_current_keeps_it() {
        let mut q = Queue::default();
        q.replace(tracks(&["a", "b", "c"]), 2, Source::default());
        let uid = q.entries[0].uid;
        assert!(!q.remove(uid));
        assert_eq!(q.current_track().unwrap().video_id, "c");
    }

    #[test]
    fn moving_entries_follows_current() {
        let mut q = Queue::default();
        q.replace(tracks(&["a", "b", "c", "d"]), 1, Source::default());
        q.move_entry(3, 0);
        assert_eq!(ids(&q), ["d", "a", "b", "c"]);
        assert_eq!(q.current_track().unwrap().video_id, "b");
    }

    #[test]
    fn radio_skips_duplicates() {
        let mut q = Queue::default();
        q.replace(tracks(&["a", "b"]), 0, Source::default());
        assert_eq!(q.extend_unique(tracks(&["a", "c", "b", "d"])), 2);
        assert_eq!(ids(&q), ["a", "b", "c", "d"]);
    }
}
