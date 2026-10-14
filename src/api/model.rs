//! What the interface shows, parsed out of InnerTube's renderers.

use serde::{Deserialize, Serialize};

/// A name that may lead somewhere (an artist, an album, a channel).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Link {
    pub name: String,
    pub id: Option<String>,
}

impl Link {
    pub fn new(name: impl Into<String>, id: Option<String>) -> Self {
        Self {
            name: name.into(),
            id,
        }
    }
}

/// A playable song, video or episode.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub video_id: String,
    pub title: String,
    #[serde(default)]
    pub artists: Vec<Link>,
    #[serde(default)]
    pub album: Option<Link>,
    /// Seconds.
    #[serde(default)]
    pub duration: Option<u32>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub explicit: bool,
    /// A music video rather than an audio track.
    #[serde(default)]
    pub video: bool,
    /// The item's id within a playlist, for removing it.
    #[serde(default)]
    pub set_video_id: Option<String>,
    /// Shown instead of artists when there are none (plays, views, a date).
    #[serde(default)]
    pub extra: Option<String>,
    /// The year it came out, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    /// A song on this PC rather than YouTube Music's: its file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

impl Track {
    /// A song from a folder on this PC.
    pub fn is_local(&self) -> bool {
        self.file.is_some()
    }

    /// The title as YouTube Music's player shows it, and as Last.fm knows
    /// the song: "Tsubasa" for "ツバサ - Tsubasa".
    pub fn romanized_title(&self) -> &str {
        romanized_title(&self.title)
    }

    pub fn artist_line(&self) -> String {
        if self.artists.is_empty() {
            return self.extra.clone().unwrap_or_default();
        }
        self.artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Song,
    Video,
    Album,
    Artist,
    Playlist,
    Podcast,
    Episode,
    Profile,
    /// A mood, genre or chart button.
    Category,
    Unknown,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Song => "Song",
            Kind::Video => "Video",
            Kind::Album => "Album",
            Kind::Artist => "Artist",
            Kind::Playlist => "Playlist",
            Kind::Podcast => "Podcast",
            Kind::Episode => "Episode",
            Kind::Profile => "Profile",
            Kind::Category => "",
            Kind::Unknown => "",
        }
    }

    /// Round artwork, as YouTube Music draws artists.
    pub fn round(self) -> bool {
        matches!(self, Kind::Artist | Kind::Profile)
    }
}

/// Where an item leads.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    /// Play a video, perhaps within a playlist or radio.
    Watch {
        video_id: String,
        playlist_id: Option<String>,
        params: Option<String>,
    },
    /// Play a playlist from its start.
    WatchPlaylist {
        playlist_id: String,
        params: Option<String>,
    },
    /// Open a page.
    Browse {
        browse_id: String,
        params: Option<String>,
    },
    /// Run a search.
    Search {
        query: String,
        params: Option<String>,
    },
}

/// Anything in a shelf, carousel or grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    pub thumbnail: Option<String>,
    pub target: Option<Target>,
    /// For songs and videos: what plays.
    pub track: Option<Track>,
    /// A playlist or album's id for playing it directly from a card.
    pub play_playlist: Option<String>,
    /// For category buttons: their colour, as ARGB.
    pub color: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Rows of tracks or results.
    List,
    /// A horizontal row of cards.
    Carousel,
    /// Cards wrapping onto lines.
    Grid,
    /// Paragraphs (an artist's description).
    Text,
    /// Search's top result.
    Card,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: String,
    /// The small line above a carousel's title.
    pub strapline: Option<String>,
    pub layout: Layout,
    pub items: Vec<Item>,
    /// "More" or "See all".
    pub more: Option<Target>,
    pub text: Option<String>,
    /// The next page of this shelf's items.
    pub continuation: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Header {
    pub title: String,
    pub subtitle: Option<String>,
    pub second_subtitle: Option<String>,
    pub description: Option<String>,
    pub thumbnail: Option<String>,
    /// The album's artist or the playlist's owner.
    pub author: Option<Link>,
    pub play: Option<Target>,
    pub shuffle: Option<Target>,
    pub radio: Option<Target>,
    /// The playlist behind an album or playlist page.
    pub playlist_id: Option<String>,
    /// An artist's channel, for subscribing.
    pub channel_id: Option<String>,
    pub subscribed: Option<bool>,
    pub subscribers: Option<String>,
    /// Saved to the library.
    pub saved: Option<bool>,
    pub editable: bool,
    /// Artists draw a wide banner.
    pub banner: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub title: String,
    pub target: Target,
    pub selected: bool,
}

/// A browse page: home, explore, an artist, an album, a playlist, the
/// library, a mood.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub header: Option<Header>,
    /// An album or playlist's songs.
    pub tracks: Vec<Track>,
    /// More songs to fetch.
    pub tracks_continuation: Option<String>,
    pub sections: Vec<Section>,
    pub continuation: Option<String>,
    pub chips: Vec<Chip>,
}

/// The up-next queue for a video, with what YouTube Music shows beside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WatchQueue {
    pub tracks: Vec<Track>,
    pub continuation: Option<String>,
    pub playlist_id: Option<String>,
    pub lyrics_id: Option<String>,
    pub related_id: Option<String>,
    /// The signed-in account's rating of the first video.
    pub like: Option<Like>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Like {
    Liked,
    Disliked,
    #[default]
    Indifferent,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Suggestions {
    pub queries: Vec<String>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lyrics {
    pub text: String,
    pub source: Option<String>,
    /// The lines with their times, when the lyrics are synced.
    pub lines: Vec<LyricLine>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LyricLine {
    /// Seconds into the song.
    pub time: f64,
    /// Empty for an instrumental break.
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub handle: Option<String>,
    pub photo: Option<String>,
}

/// Seconds from "3:45" or "1:02:03".
pub fn parse_duration(text: &str) -> Option<u32> {
    let parts: Vec<&str> = text.trim().split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let mut total = 0u32;
    for part in parts {
        if part.is_empty() || part.len() > 2 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        total = total * 60 + part.parse::<u32>().ok()?;
    }
    Some(total)
}

pub fn format_duration(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A thumbnail URL at another size. Google's image hosts take the size in
/// the address, and so does Last.fm's (in a few sizes, or the original);
/// video stills come in fixed sizes.
pub fn sized_thumbnail(url: &str, px: u32) -> String {
    if url.contains(".freetls.fastly.net/i/u/")
        && let Some((base, rest)) = url.split_once("/i/u/")
    {
        let file = rest.rsplit('/').next().unwrap_or(rest);
        let size = match px {
            0..=64 => "64s/",
            65..=174 => "174s/",
            175..=300 => "300x300/",
            _ => "",
        };
        return format!("{base}/i/u/{size}{file}");
    }
    if (url.contains("googleusercontent.com") || url.contains("ggpht.com"))
        && let Some(eq) = url.rfind('=')
    {
        let options = &url[eq + 1..];
        if options.starts_with('w') || options.starts_with('s') {
            return format!("{}=w{px}-h{px}-l90-rj", &url[..eq]);
        }
    }
    url.to_owned()
}

/// A title in Latin letters, when YouTube Music has one. For songs titled in
/// Japanese (or Chinese, or Korean) its lists say "original - romanised"
/// ("ツバサ - Tsubasa", "完全感覚Dreamer - Kanzen Kankaku Dreamer"), while
/// its player, and so the web player bar and the scrobblers reading it,
/// says only the second part. Anything else comes back as it is.
pub fn romanized_title(title: &str) -> &str {
    let cuts: Vec<usize> = title.match_indices(" - ").map(|(i, _)| i).collect();
    // The original and its romanised form have the same shape, so with
    // several dashes the cut is the middle one ("ナンデモナイヤ - movie
    // ver. - Nandemonaiya - movie ver.").
    if cuts.len() % 2 == 0 {
        return title;
    }
    let cut = cuts[cuts.len() / 2];
    let (original, romanised) = (&title[..cut], title[cut + 3..].trim());
    let latin = romanised.chars().any(|c| c.is_ascii_alphabetic());
    if !original.chars().any(is_cjk_letter) || romanised.chars().any(is_cjk) || !latin || only_version_words(romanised) {
        return title;
    }
    romanised
}

/// Kana, kanji or hangul.
fn is_cjk_letter(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
        | 0xFF66..=0xFF9F | 0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F)
}

/// Those, or the full-width punctuation that goes with them.
fn is_cjk(c: char) -> bool {
    is_cjk_letter(c) || matches!(c as u32, 0x3000..=0x303F | 0xFF00..=0xFFEF)
}

/// "TV Size", "Instrumental", "Acoustic Ver.": a version of the song, not
/// a second name for it.
fn only_version_words(text: &str) -> bool {
    const WORDS: [&str; 22] = [
        "tv", "size", "instrumental", "inst", "off", "vocal", "acoustic", "ver", "version", "remix", "mix", "live",
        "remaster", "remastered", "edit", "radio", "short", "full", "karaoke", "demo", "piano", "english",
    ];
    let mut words = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && w.chars().any(char::is_alphabetic))
        .peekable();
    words.peek().is_some() && words.all(|w| WORDS.contains(&w.to_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn romanised_titles() {
        // As YouTube Music's player names them.
        for (listed, shown) in [
            ("ツバサ - Tsubasa", "Tsubasa"),
            ("完全感覚Dreamer - Kanzen Kankaku Dreamer", "Kanzen Kankaku Dreamer"),
            ("今はいいんだよ。 (Cover) - IMAWA IINDAYO (Cover)", "IMAWA IINDAYO (Cover)"),
            ("＊〜アスタリスク〜（アルバムバージョン） - *Asterisk", "*Asterisk"),
            ("ray (超かぐや姫！ Version) - ray (Cosmic Princess Kaguya! Version)", "ray (Cosmic Princess Kaguya! Version)"),
            ("ナンデモナイヤ - movie ver. - Nandemonaiya - movie ver.", "Nandemonaiya - movie ver."),
            ("자유 - Freedom", "Freedom"),
            // Left alone.
            ("Nandemonaiya - movie ver.", "Nandemonaiya - movie ver."),
            ("夜に駆ける", "夜に駆ける"),
            ("紅蓮華 - TV Size", "紅蓮華 - TV Size"),
            ("Song - Remastered 2011", "Song - Remastered 2011"),
            ("ブルーバード (いきものがかり) Arrange ver.／ダズビー COVER", "ブルーバード (いきものがかり) Arrange ver.／ダズビー COVER"),
            ("A - B - C", "A - B - C"),
        ] {
            assert_eq!(romanized_title(listed), shown, "{listed}");
        }
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("3:45"), Some(225));
        assert_eq!(parse_duration("1:02:03"), Some(3723));
        assert_eq!(parse_duration("2.1B views"), None);
        assert_eq!(parse_duration("2026"), None);
        assert_eq!(format_duration(225), "3:45");
        assert_eq!(format_duration(3723), "1:02:03");
    }

    #[test]
    fn thumbnail_sizes() {
        assert_eq!(
            sized_thumbnail("https://lh3.googleusercontent.com/abc=w60-h60-l90-rj", 226),
            "https://lh3.googleusercontent.com/abc=w226-h226-l90-rj"
        );
        let lastfm = "https://lastfm.freetls.fastly.net/i/u/300x300/6ccb.jpg";
        assert_eq!(sized_thumbnail(lastfm, 64), "https://lastfm.freetls.fastly.net/i/u/64s/6ccb.jpg");
        assert_eq!(sized_thumbnail(lastfm, 1024), "https://lastfm.freetls.fastly.net/i/u/6ccb.jpg");
        let still = "https://i.ytimg.com/vi/x/hqdefault.jpg";
        assert_eq!(sized_thumbnail(still, 226), still);
    }
}
