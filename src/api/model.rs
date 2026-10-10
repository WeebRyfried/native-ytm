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
}

impl Track {
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
/// the address; video stills come in fixed sizes.
pub fn sized_thumbnail(url: &str, px: u32) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let still = "https://i.ytimg.com/vi/x/hqdefault.jpg";
        assert_eq!(sized_thumbnail(still, 226), still);
    }
}
