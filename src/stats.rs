//! Listening stats: the person's top artists, songs and albums over a
//! stretch of time, and how much they listened. From Last.fm when it is
//! connected (all the years of scrobbles there), or from the plays this
//! app has heard, which it keeps from the first one on: those count songs
//! on this PC too, and the time listened.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::api::model::{Link, Track};

/// How many of each kind of top list.
pub const TOP: usize = 10;

/// Where the stats come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    LastFm,
    App,
}

/// The stretch of time the stats cover, up to now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Period {
    Week,
    Month,
    Quarter,
    HalfYear,
    Year,
    All,
}

impl Period {
    pub const ALL: [Period; 6] = [Period::Week, Period::Month, Period::Quarter, Period::HalfYear, Period::Year, Period::All];

    pub fn label(self) -> &'static str {
        match self {
            Period::Week => "7 days",
            Period::Month => "30 days",
            Period::Quarter => "3 months",
            Period::HalfYear => "6 months",
            Period::Year => "12 months",
            Period::All => "All time",
        }
    }

    /// As Last.fm's methods name it.
    pub fn lastfm(self) -> &'static str {
        match self {
            Period::Week => "7day",
            Period::Month => "1month",
            Period::Quarter => "3month",
            Period::HalfYear => "6month",
            Period::Year => "12month",
            Period::All => "overall",
        }
    }

    /// How many days back it reaches; all of them for All.
    pub fn days(self) -> Option<i64> {
        match self {
            Period::Week => Some(7),
            Period::Month => Some(30),
            Period::Quarter => Some(91),
            Period::HalfYear => Some(182),
            Period::Year => Some(365),
            Period::All => None,
        }
    }
}

/// One of a top list: an artist, a song or an album.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ranked {
    pub name: String,
    /// The artist, for a song or an album.
    pub by: Option<String>,
    pub plays: u64,
    pub art: Option<String>,
    /// For a song the app heard: what plays it again.
    pub track: Option<Track>,
}

/// How the plays fall over the stretch, oldest first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bins {
    #[default]
    Days,
    Weeks,
    Months,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub plays: Option<u64>,
    /// Time listened, in seconds (the app's own plays only).
    pub seconds: Option<u64>,
    /// How many different artists and songs.
    pub artists: Option<u64>,
    pub songs: Option<u64>,
    pub top_artists: Vec<Ranked>,
    pub top_songs: Vec<Ranked>,
    pub top_albums: Vec<Ranked>,
    /// Plays in each day, week or month of the stretch, oldest first (the
    /// app's own plays only).
    pub bins: Vec<u32>,
    pub bin: Bins,
    /// When the plays begin: the first one kept, or joining Last.fm.
    pub since: Option<i64>,
}

/// A play the app heard: enough of a song (half of it, or 30 seconds).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Play {
    /// When it started, in Unix seconds.
    pub at: i64,
    /// How long was heard, in seconds.
    pub secs: u32,
    /// The song's id: YouTube Music's, or one of this PC's.
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
}

/// The song playing, followed until it changes.
struct Following {
    uid: u64,
    track: Track,
    duration: Option<f64>,
    started: i64,
    heard: f64,
    last_position: f64,
    last_tick: Instant,
    was_playing: bool,
}

/// The plays the app keeps, one line of JSON each, added to as songs end.
pub struct PlayLog {
    path: PathBuf,
    following: Option<Following>,
    /// Bumped as plays are written, so stats know to look again.
    pub written: u64,
}

impl PlayLog {
    pub fn new(path: PathBuf) -> Self {
        Self { path, following: None, written: 0 }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Follows what plays: the song (by its queue entry), where it is, how
    /// long it is, and whether it is playing. Call often.
    pub fn tick(&mut self, current: Option<(u64, &Track, f64, Option<f64>, bool)>) {
        let now = Instant::now();
        let Some((uid, track, position, duration, playing)) = current else {
            self.finish();
            return;
        };
        // The same entry from the top again (repeat one) is another play.
        let restarted = self.following.as_ref().is_some_and(|f| f.uid == uid && position < 3.0 && f.last_position > 10.0 && counts(f.heard, f.duration));
        if restarted || self.following.as_ref().is_none_or(|f| f.uid != uid) {
            self.finish();
            self.following = Some(Following {
                uid,
                track: track.clone(),
                duration,
                started: unix_now() - position.max(0.0) as i64,
                heard: 0.0,
                last_position: position,
                last_tick: now,
                was_playing: playing,
            });
        }
        let Some(f) = &mut self.following else { return };
        let moved = position - f.last_position;
        let elapsed = now.duration_since(f.last_tick).as_secs_f64();
        // Steady playback moves the position with the clock; a seek does not.
        if f.was_playing && playing && moved > 0.0 && (moved - elapsed).abs() < 2.5 {
            f.heard += moved;
        }
        if duration.is_some() {
            f.duration = duration;
        }
        f.last_position = position;
        f.last_tick = now;
        f.was_playing = playing;
    }

    /// The song followed is over: kept when enough of it was heard.
    pub fn finish(&mut self) {
        let Some(f) = self.following.take() else { return };
        if !counts(f.heard, f.duration) {
            return;
        }
        let play = Play {
            at: f.started,
            secs: f.heard.round() as u32,
            id: f.track.video_id.clone(),
            title: f.track.romanized_title().to_owned(),
            artists: f.track.artists.iter().map(|a| a.name.clone()).collect(),
            album: f.track.album.as_ref().map(|a| a.name.clone()),
            art: f.track.thumbnail.clone(),
        };
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let line = serde_json::to_string(&play).unwrap_or_default();
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| writeln!(file, "{line}"));
        match written {
            Ok(()) => self.written += 1,
            Err(error) => log::warn!("cannot keep a play: {error}"),
        }
    }
}

/// Enough heard to count: half the song, or 30 seconds, and at least ten.
fn counts(heard: f64, duration: Option<f64>) -> bool {
    let half = duration.filter(|d| *d > 0.0).map_or(30.0, |d| d / 2.0);
    heard >= half.min(30.0).max(10.0)
}

/// Every play kept, oldest first; a line that will not read is passed over.
pub fn load(path: &Path) -> Vec<Play> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

/// The app's own stats for a stretch ending `now` (Unix seconds).
pub fn summarize(plays: &[Play], period: Period, now: i64) -> Summary {
    const DAY: i64 = 24 * 60 * 60;
    let from = period.days().map(|days| now - days * DAY);
    let shown: Vec<&Play> = plays.iter().filter(|p| from.is_none_or(|from| p.at >= from) && p.at <= now).collect();
    let since = plays.iter().map(|p| p.at).min();

    // Artists: by name, any case; each play counts for each of its artists.
    let mut artists: HashMap<String, Ranked> = HashMap::new();
    let mut songs: HashMap<String, Ranked> = HashMap::new();
    let mut albums: HashMap<String, Ranked> = HashMap::new();
    for play in &shown {
        for name in &play.artists {
            let entry = artists.entry(name.to_lowercase()).or_insert_with(|| Ranked { name: name.clone(), ..Ranked::default() });
            entry.plays += 1;
            if entry.art.is_none() {
                entry.art.clone_from(&play.art);
            }
        }
        let by = play.artists.join(", ");
        let entry = songs.entry(play.id.clone()).or_insert_with(|| Ranked {
            name: play.title.clone(),
            by: (!by.is_empty()).then(|| by.clone()),
            track: Some(Track {
                video_id: play.id.clone(),
                title: play.title.clone(),
                artists: play.artists.iter().map(|a| Link::new(a.clone(), None)).collect(),
                album: play.album.as_ref().map(|a| Link::new(a.clone(), None)),
                thumbnail: play.art.clone(),
                ..Track::default()
            }),
            ..Ranked::default()
        });
        entry.plays += 1;
        if entry.art.is_none() {
            entry.art.clone_from(&play.art);
        }
        if let Some(album) = &play.album {
            let first = play.artists.first().cloned().unwrap_or_default();
            let key = format!("{}\t{}", first.to_lowercase(), album.to_lowercase());
            let entry = albums.entry(key).or_insert_with(|| Ranked {
                name: album.clone(),
                by: (!first.is_empty()).then_some(first),
                ..Ranked::default()
            });
            entry.plays += 1;
            if entry.art.is_none() {
                entry.art.clone_from(&play.art);
            }
        }
    }
    let top = |map: HashMap<String, Ranked>| -> Vec<Ranked> {
        let mut list: Vec<Ranked> = map.into_values().collect();
        list.sort_by(|a, b| b.plays.cmp(&a.plays).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        list.truncate(TOP);
        list
    };
    let (artist_count, song_count) = (artists.len() as u64, songs.len() as u64);

    // The plays over the stretch: by day for a month or less, by week up to
    // half a year, by month beyond.
    let start = from.or(since).unwrap_or(now);
    let span = (now - start).max(DAY);
    let (bin, width) = if span <= 31 * DAY {
        (Bins::Days, DAY)
    } else if span <= 186 * DAY {
        (Bins::Weeks, 7 * DAY)
    } else {
        (Bins::Months, 30 * DAY)
    };
    let count = ((span + width - 1) / width).clamp(1, 60) as usize;
    let mut bins = vec![0u32; count];
    for play in &shown {
        let back = ((now - play.at) / width) as usize;
        if back < count {
            bins[count - 1 - back] += 1;
        }
    }

    Summary {
        plays: Some(shown.len() as u64),
        seconds: Some(shown.iter().map(|p| u64::from(p.secs)).sum()),
        artists: Some(artist_count),
        songs: Some(song_count),
        top_artists: top(artists),
        top_songs: top(songs),
        top_albums: top(albums),
        bins,
        bin,
        since,
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(at: i64, title: &str, artist: &str, album: &str) -> Play {
        Play {
            at,
            secs: 200,
            id: format!("id-{title}"),
            title: title.into(),
            artists: vec![artist.into()],
            album: Some(album.into()),
            art: None,
        }
    }

    #[test]
    fn tops_over_a_stretch() {
        let now = 1_000_000_000;
        let day = 86_400;
        let plays = vec![
            play(now - 40 * day, "Old", "Aimer", "A"),
            play(now - 3 * day, "Brave Shine", "Aimer", "DAWN"),
            play(now - 2 * day, "Brave Shine", "Aimer", "DAWN"),
            play(now - day, "Gurenge", "LiSA", "LEO-NiNE"),
            play(now - 100, "Brave Shine", "aimer", "DAWN"),
        ];
        let week = summarize(&plays, Period::Week, now);
        assert_eq!(week.plays, Some(4));
        assert_eq!(week.seconds, Some(800));
        assert_eq!(week.artists, Some(2));
        assert_eq!(week.songs, Some(2));
        assert_eq!(week.top_artists[0].name, "Aimer");
        assert_eq!(week.top_artists[0].plays, 3);
        assert_eq!(week.top_songs[0].name, "Brave Shine");
        assert_eq!(week.top_songs[0].plays, 3);
        assert_eq!(week.top_albums[0].name, "DAWN");
        assert_eq!(week.bin, Bins::Days);
        assert_eq!(week.bins.len(), 7);
        assert_eq!(week.bins.iter().sum::<u32>(), 4);
        assert_eq!(*week.bins.last().unwrap(), 1);
        let all = summarize(&plays, Period::All, now);
        assert_eq!(all.plays, Some(5));
        assert_eq!(all.bin, Bins::Weeks);
        assert_eq!(all.since, Some(now - 40 * day));
    }

    #[test]
    fn a_play_counts_once_enough_is_heard() {
        assert!(!counts(9.0, Some(15.0)));
        assert!(counts(10.0, Some(15.0)));
        assert!(!counts(29.0, Some(240.0)));
        assert!(counts(30.0, Some(240.0)));
        assert!(counts(30.0, None));
    }

    #[test]
    fn plays_are_kept_and_read_back() {
        let dir = std::env::temp_dir().join(format!("native-ytm-plays-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut log = PlayLog::new(dir.join("plays.jsonl"));
        let track = Track { video_id: "abc".into(), title: "Song".into(), artists: vec![Link::new("Someone", None)], ..Track::default() };
        // Heard steadily for 40 s of a 3-minute song, then another song.
        log.tick(Some((1, &track, 0.0, Some(180.0), true)));
        if let Some(f) = log.following.as_mut() {
            f.heard = 40.0;
        }
        let other = Track { video_id: "def".into(), title: "Other".into(), ..Track::default() };
        log.tick(Some((2, &other, 0.0, Some(180.0), true)));
        log.finish();
        let plays = load(log.path());
        assert_eq!(plays.len(), 1);
        assert_eq!((plays[0].id.as_str(), plays[0].secs), ("abc", 40));
        assert_eq!(log.written, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
