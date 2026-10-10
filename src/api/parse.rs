//! Turning InnerTube's renderers into the model.
//!
//! YouTube Music moves its renderers between wrappers often, so the parsers
//! look for the item renderers wherever they are, and read each item from its
//! endpoints (which say what it is) rather than from its labels (which are
//! translated).

use serde_json::Value;

use super::model::*;

pub fn text(v: &Value) -> String {
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return s.to_owned();
    }
    if let Some(s) = v.get("content").and_then(Value::as_str) {
        return s.to_owned();
    }
    runs(v).iter().filter_map(|r| r["text"].as_str()).collect()
}

pub fn opt_text(v: &Value) -> Option<String> {
    let s = text(v);
    (!s.trim().is_empty()).then_some(s)
}

pub fn runs(v: &Value) -> &[Value] {
    v.get("runs")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

/// The largest thumbnail anywhere under `v`.
pub fn thumbnail(v: &Value) -> Option<String> {
    fn find(v: &Value, depth: u32) -> Option<&Vec<Value>> {
        if depth > 6 {
            return None;
        }
        match v {
            Value::Object(map) => {
                if let Some(list) = map.get("thumbnails").and_then(Value::as_array)
                    && !list.is_empty()
                {
                    return Some(list);
                }
                for key in [
                    "musicThumbnailRenderer",
                    "croppedSquareThumbnailRenderer",
                    "thumbnail",
                    "thumbnailRenderer",
                    "foregroundThumbnail",
                    "musicAnimatedThumbnailRenderer",
                    "backupRenderer",
                ] {
                    if let Some(inner) = map.get(key)
                        && let Some(found) = find(inner, depth + 1)
                    {
                        return Some(found);
                    }
                }
                None
            }
            _ => None,
        }
    }
    let list = find(v, 0)?;
    list.iter()
        .max_by_key(|t| t["width"].as_u64().unwrap_or(0))
        .and_then(|t| t["url"].as_str())
        .map(|url| {
            if url.starts_with("//") {
                format!("https:{url}")
            } else {
                url.to_owned()
            }
        })
}

fn page_type(browse: &Value) -> Option<&str> {
    browse
        .pointer("/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
        .and_then(Value::as_str)
}

fn kind_of_page(page: Option<&str>, browse_id: &str) -> Kind {
    match page {
        Some("MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK") => Kind::Album,
        Some("MUSIC_PAGE_TYPE_ARTIST") => Kind::Artist,
        Some("MUSIC_PAGE_TYPE_USER_CHANNEL") => Kind::Profile,
        Some("MUSIC_PAGE_TYPE_PLAYLIST") => Kind::Playlist,
        Some("MUSIC_PAGE_TYPE_PODCAST_SHOW_DETAIL_PAGE") => Kind::Podcast,
        Some("MUSIC_PAGE_TYPE_NON_MUSIC_AUDIO_TRACK_PAGE") => Kind::Episode,
        _ if browse_id.starts_with("MPRE") => Kind::Album,
        _ if browse_id.starts_with("VL") => Kind::Playlist,
        _ if browse_id.starts_with("UC") => Kind::Artist,
        _ if browse_id.starts_with("FEmusic_moods_and_genres_category") => Kind::Category,
        _ => Kind::Unknown,
    }
}

fn video_kind(watch: &Value) -> Kind {
    match watch
        .pointer("/watchEndpointMusicSupportedConfigs/watchEndpointMusicConfig/musicVideoType")
        .and_then(Value::as_str)
    {
        Some("MUSIC_VIDEO_TYPE_OMV" | "MUSIC_VIDEO_TYPE_UGC" | "MUSIC_VIDEO_TYPE_OFFICIAL_SOURCE_MUSIC") => {
            Kind::Video
        }
        Some("MUSIC_VIDEO_TYPE_PODCAST_EPISODE") => Kind::Episode,
        _ => Kind::Song,
    }
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Where a navigation endpoint leads, and what kind of thing is there.
pub fn target(endpoint: &Value) -> Option<(Target, Kind)> {
    if let Some(watch) = endpoint.get("watchEndpoint") {
        if let Some(video_id) = str_of(watch, "videoId") {
            return Some((
                Target::Watch {
                    video_id,
                    playlist_id: str_of(watch, "playlistId"),
                    params: str_of(watch, "params"),
                },
                video_kind(watch),
            ));
        }
        if let Some(playlist_id) = str_of(watch, "playlistId") {
            return Some((
                Target::WatchPlaylist {
                    playlist_id,
                    params: str_of(watch, "params"),
                },
                Kind::Playlist,
            ));
        }
    }
    if let Some(watch) = endpoint.get("watchPlaylistEndpoint")
        && let Some(playlist_id) = str_of(watch, "playlistId")
    {
        return Some((
            Target::WatchPlaylist {
                playlist_id,
                params: str_of(watch, "params"),
            },
            Kind::Playlist,
        ));
    }
    if let Some(browse) = endpoint.get("browseEndpoint")
        && let Some(browse_id) = str_of(browse, "browseId")
    {
        let kind = kind_of_page(page_type(browse), &browse_id);
        return Some((
            Target::Browse {
                browse_id,
                params: str_of(browse, "params"),
            },
            kind,
        ));
    }
    if let Some(search) = endpoint.get("searchEndpoint")
        && let Some(query) = str_of(search, "query")
    {
        return Some((
            Target::Search {
                query,
                params: str_of(search, "params"),
            },
            Kind::Unknown,
        ));
    }
    None
}

const SEPARATORS: [&str; 4] = [" • ", " & ", ", ", " · "];

fn is_separator(s: &str) -> bool {
    SEPARATORS.contains(&s) || s.trim().is_empty() || s.trim() == "•" || s.trim() == "&" || s.trim() == ","
}

/// Labels YouTube Music puts first in a subtitle to say what an item is.
fn is_type_label(s: &str) -> bool {
    matches!(
        s,
        "Song" | "Video" | "Album" | "Single" | "EP" | "Artist" | "Playlist" | "Episode"
            | "Podcast" | "Profile" | "Station" | "Audiobook" | "Mix"
    )
}

fn is_count(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    ["views", "plays", "listeners", "subscribers", "likes", "songs", "episodes", "monthly", "audience", "tracks"]
        .iter()
        .any(|w| lower.contains(w))
}

struct Byline {
    artists: Vec<Link>,
    album: Option<Link>,
    duration: Option<u32>,
    /// What else was said (plays, a year).
    extra: Vec<String>,
}

/// Reads artists, the album and a duration out of a subtitle's runs.
fn byline(runs: &[Value], plain_artists: bool) -> Byline {
    let mut out = Byline {
        artists: Vec::new(),
        album: None,
        duration: None,
        extra: Vec::new(),
    };
    let mut plain = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        let text = run["text"].as_str().unwrap_or_default();
        if is_separator(text) {
            continue;
        }
        if let Some(browse) = run.pointer("/navigationEndpoint/browseEndpoint") {
            let id = str_of(browse, "browseId");
            match kind_of_page(page_type(browse), id.as_deref().unwrap_or_default()) {
                Kind::Album => out.album = Some(Link::new(text, id)),
                Kind::Artist | Kind::Profile => out.artists.push(Link::new(text, id)),
                _ => out.extra.push(text.to_owned()),
            }
            continue;
        }
        if let Some(d) = parse_duration(text) {
            out.duration = Some(d);
            continue;
        }
        if index == 0 && is_type_label(text) {
            continue;
        }
        if is_count(text) || (text.len() == 4 && text.bytes().all(|b| b.is_ascii_digit())) {
            out.extra.push(text.to_owned());
            continue;
        }
        plain.push(text.to_owned());
    }
    if out.artists.is_empty() && plain_artists && !plain.is_empty() {
        // Artists with no page to link to, as on compilations: the first
        // plain run, which may itself list several.
        out.artists.push(Link::new(plain.remove(0), None));
    }
    out.extra.extend(plain);
    out
}

fn explicit(r: &Value) -> bool {
    r.get("badges")
        .and_then(Value::as_array)
        .is_some_and(|badges| {
            badges.iter().any(|b| {
                b.pointer("/musicInlineBadgeRenderer/icon/iconType").and_then(Value::as_str)
                    == Some("MUSIC_EXPLICIT_BADGE")
            })
        })
        || r.get("subtitleBadges").is_some_and(|b| b.to_string().contains("MUSIC_EXPLICIT_BADGE"))
}

fn flex_column(r: &Value, index: usize) -> Option<&Value> {
    r.get("flexColumns")?
        .get(index)?
        .get("musicResponsiveListItemFlexColumnRenderer")?
        .get("text")
}

fn fixed_column(r: &Value, index: usize) -> Option<&Value> {
    r.get("fixedColumns")?
        .get(index)?
        .get("musicResponsiveListItemFixedColumnRenderer")?
        .get("text")
}

/// The play button's endpoint on an item's artwork.
fn overlay_play(r: &Value) -> Option<&Value> {
    r.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint")
        .or_else(|| r.pointer("/thumbnailOverlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint"))
}

/// A row: a song, or a search result of any kind.
pub fn responsive_item(r: &Value) -> Option<Item> {
    let title_col = flex_column(r, 0)?;
    let title = text(title_col);
    if title.is_empty() {
        return None;
    }
    let mut kind = Kind::Unknown;
    let mut item_target = None;
    if let Some((t, k)) = r.get("navigationEndpoint").and_then(target) {
        item_target = Some(t);
        kind = k;
    }
    let mut video_id = r
        .pointer("/playlistItemData/videoId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut watch_kind = None;
    let mut watch_target = None;
    for endpoint in [
        runs(title_col).first().and_then(|r| r.get("navigationEndpoint")),
        overlay_play(r),
    ]
    .into_iter()
    .flatten()
    {
        if let Some((t, k)) = target(endpoint) {
            if let Target::Watch { video_id: id, .. } = &t {
                video_id.get_or_insert_with(|| id.clone());
                watch_kind.get_or_insert(k);
                watch_target.get_or_insert(t.clone());
            } else if item_target.is_none() {
                item_target = Some(t);
                kind = k;
            }
        }
    }
    let subtitle_runs: Vec<Value> = (1..4)
        .filter_map(|i| flex_column(r, i))
        .flat_map(|col| {
            let mut v = runs(col).to_vec();
            v.push(serde_json::json!({ "text": " • " }));
            v
        })
        .collect();
    let mut subtitle: Vec<String> = Vec::new();
    for col in (1..4).filter_map(|i| flex_column(r, i)) {
        if let Some(t) = opt_text(col) {
            subtitle.push(t);
        }
    }
    let track = if let Some(video_id) = video_id.filter(|_| {
        // Albums and playlists in search results carry a play button for
        // their first song; they are not themselves tracks.
        !matches!(kind, Kind::Album | Kind::Playlist | Kind::Artist | Kind::Podcast | Kind::Profile)
    }) {
        kind = watch_kind.unwrap_or(Kind::Song);
        let line = byline(&subtitle_runs, true);
        let duration = line.duration.or_else(|| {
            fixed_column(r, 0).and_then(|t| parse_duration(&text(t)))
        });
        if item_target.is_none() {
            item_target = watch_target;
        }
        Some(Track {
            video_id,
            title: title.clone(),
            artists: line.artists,
            album: line.album,
            duration,
            thumbnail: thumbnail(r.get("thumbnail").unwrap_or(&Value::Null)),
            explicit: explicit(r),
            video: kind == Kind::Video,
            set_video_id: r
                .pointer("/playlistItemData/playlistSetVideoId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            extra: line.extra.first().cloned(),
        })
    } else {
        None
    };
    let play_playlist = overlay_play(r).and_then(|e| {
        e.pointer("/watchPlaylistEndpoint/playlistId")
            .or_else(|| e.pointer("/watchEndpoint/playlistId"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    Some(Item {
        kind,
        title,
        subtitle: strip_type_label(&subtitle.join(" • ")),
        thumbnail: thumbnail(r.get("thumbnail").unwrap_or(&Value::Null)),
        target: item_target,
        track,
        play_playlist,
        color: None,
    })
}

fn strip_type_label(s: &str) -> String {
    match s.split_once(" • ") {
        Some((first, rest)) if is_type_label(first) && !matches!(first, "Single" | "EP" | "Album") => {
            rest.to_owned()
        }
        _ => s.to_owned(),
    }
}

/// A card in a carousel or grid.
pub fn two_row_item(r: &Value) -> Option<Item> {
    let title = text(r.get("title")?);
    if title.is_empty() {
        return None;
    }
    let endpoint = r
        .get("navigationEndpoint")
        .or_else(|| runs(&r["title"]).first().and_then(|t| t.get("navigationEndpoint")));
    let (item_target, mut kind) = endpoint.and_then(target).map_or((None, Kind::Unknown), |(t, k)| (Some(t), k));
    let subtitle_value = r.get("subtitle").cloned().unwrap_or(Value::Null);
    let subtitle = text(&subtitle_value);
    let thumb = thumbnail(r.get("thumbnailRenderer").unwrap_or(&Value::Null));
    let track = match &item_target {
        Some(Target::Watch { video_id, .. }) => {
            let line = byline(runs(&subtitle_value), true);
            if kind == Kind::Unknown {
                kind = Kind::Song;
            }
            Some(Track {
                video_id: video_id.clone(),
                title: title.clone(),
                artists: line.artists,
                album: line.album,
                duration: line.duration,
                thumbnail: thumb.clone(),
                explicit: explicit(r),
                video: kind == Kind::Video,
                set_video_id: None,
                extra: line.extra.first().cloned(),
            })
        }
        _ => None,
    };
    let play_playlist = overlay_play(r).and_then(|e| {
        e.pointer("/watchPlaylistEndpoint/playlistId")
            .or_else(|| e.pointer("/watchEndpoint/playlistId"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    Some(Item {
        kind,
        title,
        subtitle: strip_type_label(&subtitle),
        thumbnail: thumb,
        target: item_target,
        track,
        play_playlist,
        color: None,
    })
}

/// A mood, genre or chart button.
fn navigation_button(r: &Value) -> Option<Item> {
    let title = text(r.get("buttonText")?);
    let (t, _) = target(r.get("clickCommand")?)?;
    let color = r
        .pointer("/solid/leftStripeColor")
        .and_then(Value::as_u64)
        .map(|c| c as u32);
    Some(Item {
        kind: Kind::Category,
        title,
        subtitle: String::new(),
        thumbnail: None,
        target: Some(t),
        track: None,
        play_playlist: None,
        color,
    })
}

/// An entry of the up-next queue.
pub fn panel_video(r: &Value) -> Option<Track> {
    let r = r
        .pointer("/playlistPanelVideoWrapperRenderer/primaryRenderer/playlistPanelVideoRenderer")
        .or_else(|| r.get("playlistPanelVideoRenderer"))?;
    let video_id = str_of(r, "videoId")?;
    let title = text(&r["title"]);
    let line = byline(runs(&r["longBylineText"]), true);
    let kind = r
        .get("navigationEndpoint")
        .and_then(|e| e.get("watchEndpoint"))
        .map_or(Kind::Song, video_kind);
    Some(Track {
        video_id,
        title,
        artists: if line.artists.is_empty() {
            opt_text(&r["shortBylineText"])
                .map(|name| vec![Link::new(name, None)])
                .unwrap_or_default()
        } else {
            line.artists
        },
        album: line.album,
        duration: parse_duration(&text(&r["lengthText"])),
        thumbnail: thumbnail(r),
        explicit: explicit(r),
        video: kind == Kind::Video,
        set_video_id: r
            .pointer("/navigationEndpoint/watchEndpoint/playlistSetVideoId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        extra: line.extra.first().cloned(),
    })
}

/// The items in a list of renderers, of whichever kind each is.
pub fn items(contents: &[Value]) -> Vec<Item> {
    contents
        .iter()
        .filter_map(|c| {
            if let Some(r) = c.get("musicResponsiveListItemRenderer") {
                responsive_item(r)
            } else if let Some(r) = c.get("musicTwoRowItemRenderer") {
                two_row_item(r)
            } else if let Some(r) = c.get("musicNavigationButtonRenderer") {
                navigation_button(r)
            } else if let Some(r) = c.get("musicMultiRowListItemRenderer") {
                multi_row_item(r)
            } else {
                None
            }
        })
        .collect()
}

/// A podcast episode row.
fn multi_row_item(r: &Value) -> Option<Item> {
    let title = text(r.get("title")?);
    let (t, kind) = r
        .pointer("/onTap")
        .and_then(target)
        .or_else(|| runs(&r["title"]).first().and_then(|t| t.get("navigationEndpoint")).and_then(target))?;
    let video_id = match &t {
        Target::Watch { video_id, .. } => Some(video_id.clone()),
        _ => None,
    };
    let subtitle = text(&r["subtitle"]);
    let thumb = thumbnail(r.get("thumbnail").unwrap_or(&Value::Null));
    Some(Item {
        kind: if video_id.is_some() { Kind::Episode } else { kind },
        title: title.clone(),
        subtitle: subtitle.clone(),
        thumbnail: thumb.clone(),
        track: video_id.map(|video_id| Track {
            video_id,
            title,
            thumbnail: thumb,
            extra: Some(subtitle),
            ..Track::default()
        }),
        target: Some(t),
        play_playlist: None,
        color: None,
    })
}

/// A continuation token in either of the two shapes InnerTube uses.
pub fn continuation(v: &Value) -> Option<String> {
    if let Some(list) = v.get("continuations").and_then(Value::as_array) {
        for c in list {
            for key in ["nextContinuationData", "nextRadioContinuationData", "reloadContinuationData"] {
                if let Some(token) = c.pointer(&format!("/{key}/continuation")).and_then(Value::as_str) {
                    return Some(token.to_owned());
                }
            }
        }
    }
    item_continuation(v.get("contents").and_then(Value::as_array).map_or(&[][..], Vec::as_slice))
}

/// A `continuationItemRenderer` at the end of a list of items.
pub fn item_continuation(contents: &[Value]) -> Option<String> {
    contents.iter().rev().find_map(|c| {
        c.pointer("/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

fn header_title(header: &Value) -> (String, Option<String>, Option<Target>) {
    let basic = header
        .get("musicCarouselShelfBasicHeaderRenderer")
        .or_else(|| header.get("musicImmersiveCarouselShelfHeaderRenderer"))
        .unwrap_or(header);
    let title = text(&basic["title"]);
    let strapline = opt_text(&basic["strapline"]);
    let more = basic
        .pointer("/moreContentButton/buttonRenderer/navigationEndpoint")
        .and_then(target)
        .map(|(t, _)| t)
        .or_else(|| {
            runs(&basic["title"])
                .first()
                .and_then(|r| r.get("navigationEndpoint"))
                .and_then(target)
                .map(|(t, _)| t)
        });
    (title, strapline, more)
}

/// One section renderer, if it is one the interface draws.
pub fn section(v: &Value) -> Vec<Section> {
    let Some((key, r)) = v.as_object().and_then(|m| m.iter().next()) else {
        return Vec::new();
    };
    match key.as_str() {
        "musicCarouselShelfRenderer" => {
            let (title, strapline, more) = header_title(r.get("header").unwrap_or(&Value::Null));
            let contents = r["contents"].as_array().map_or(&[][..], Vec::as_slice);
            let items = items(contents);
            if items.is_empty() {
                return Vec::new();
            }
            // Quick picks come as rows of songs inside a carousel.
            let rows = contents
                .first()
                .is_some_and(|c| c.get("musicResponsiveListItemRenderer").is_some());
            vec![Section {
                title,
                strapline,
                layout: if rows { Layout::Grid } else { Layout::Carousel },
                items,
                more,
                text: None,
                continuation: None,
            }]
        }
        "musicShelfRenderer" | "musicPlaylistShelfRenderer" => {
            let contents = r["contents"].as_array().map_or(&[][..], Vec::as_slice);
            let more = r
                .pointer("/bottomEndpoint")
                .and_then(target)
                .map(|(t, _)| t)
                .or_else(|| {
                    runs(&r["title"])
                        .first()
                        .and_then(|t| t.get("navigationEndpoint"))
                        .and_then(target)
                        .map(|(t, _)| t)
                });
            vec![Section {
                title: text(&r["title"]),
                strapline: None,
                layout: Layout::List,
                items: items(contents),
                more,
                text: None,
                continuation: continuation(r),
            }]
        }
        "gridRenderer" => {
            let contents = r["items"].as_array().map_or(&[][..], Vec::as_slice);
            let (title, strapline, more) = header_title(
                r.pointer("/header/gridHeaderRenderer").unwrap_or(&Value::Null),
            );
            vec![Section {
                title,
                strapline,
                layout: Layout::Grid,
                items: items(contents),
                more,
                text: None,
                continuation: continuation(r).or_else(|| item_continuation(contents)),
            }]
        }
        "musicDescriptionShelfRenderer" => vec![Section {
            title: text(&r["header"]),
            strapline: None,
            layout: Layout::Text,
            items: Vec::new(),
            more: None,
            text: opt_text(&r["description"]),
            continuation: None,
        }],
        "musicCardShelfRenderer" => {
            let mut top = two_row_item(r).or_else(|| {
                let (t, k) = r.get("title").and_then(|t| runs(t).first()).and_then(|r| r.get("navigationEndpoint")).and_then(target)?;
                Some(Item {
                    kind: k,
                    title: text(&r["title"]),
                    subtitle: text(&r["subtitle"]),
                    thumbnail: thumbnail(r),
                    target: Some(t),
                    track: None,
                    play_playlist: None,
                    color: None,
                })
            });
            if let Some(item) = &mut top {
                if item.thumbnail.is_none() {
                    item.thumbnail = thumbnail(r.get("thumbnail").unwrap_or(&Value::Null));
                }
                // Search's top result names its kind in the subtitle.
                if let Some(Target::Watch { .. }) = &item.target
                    && let Some(track) = &mut item.track
                {
                    track.thumbnail = item.thumbnail.clone();
                }
            }
            let mut rest = items(r["contents"].as_array().map_or(&[][..], Vec::as_slice));
            // Songs under an artist's top result leave the artist out.
            if let Some(top) = top.as_ref().filter(|t| t.kind == Kind::Artist) {
                let artist = Link::new(
                    top.title.clone(),
                    match &top.target {
                        Some(Target::Browse { browse_id, .. }) => Some(browse_id.clone()),
                        _ => None,
                    },
                );
                for item in &mut rest {
                    if let Some(track) = &mut item.track
                        && track.artists.is_empty()
                    {
                        track.artists.push(artist.clone());
                        track.extra = None;
                    }
                }
            }
            let mut sections = Vec::new();
            if let Some(top) = top {
                sections.push(Section {
                    title: text(r.pointer("/header/musicCardShelfHeaderBasicRenderer/title").unwrap_or(&Value::Null)),
                    strapline: None,
                    layout: Layout::Card,
                    items: vec![top],
                    more: None,
                    text: None,
                    continuation: None,
                });
            }
            if !rest.is_empty() {
                sections.push(Section {
                    title: String::new(),
                    strapline: None,
                    layout: Layout::List,
                    items: rest,
                    more: None,
                    text: None,
                    continuation: None,
                });
            }
            sections
        }
        "itemSectionRenderer" => {
            // Search groups each result in its own item section; gather them
            // into lists by what they hold.
            let contents = r["contents"].as_array().map_or(&[][..], Vec::as_slice);
            let mut out = Vec::new();
            let loose = items(contents);
            if !loose.is_empty() {
                out.push(Section {
                    title: String::new(),
                    strapline: None,
                    layout: Layout::List,
                    items: loose,
                    more: None,
                    text: None,
                    continuation: None,
                });
            }
            for c in contents {
                out.extend(section(c));
            }
            out
        }
        "musicTastebuilderShelfRenderer" | "musicResponsiveHeaderRenderer" | "musicDetailHeaderRenderer"
        | "musicEditablePlaylistDetailHeaderRenderer" | "chipCloudRenderer" => Vec::new(),
        _ => Vec::new(),
    }
}

/// Every section in a list, with neighbouring loose lists merged.
pub fn sections(contents: &[Value]) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for c in contents {
        for s in section(c) {
            match out.last_mut() {
                Some(last)
                    if s.title.is_empty()
                        && s.layout == Layout::List
                        && last.layout == Layout::List
                        && s.more.is_none() =>
                {
                    last.items.extend(s.items);
                }
                _ => out.push(s),
            }
        }
    }
    out.retain(|s| !s.items.is_empty() || s.text.is_some());
    out
}

pub fn chips(v: &Value) -> Vec<Chip> {
    v.pointer("/chipCloudRenderer/chips")
        .and_then(Value::as_array)
        .map(|chips| {
            chips
                .iter()
                .filter_map(|c| {
                    let r = c.get("chipCloudChipRenderer")?;
                    let (target, _) = target(r.get("navigationEndpoint")?)?;
                    Some(Chip {
                        title: text(&r["text"]),
                        target,
                        selected: r["isSelected"].as_bool().unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn find_menu_target(menu: &Value, icon: &str) -> Option<Target> {
    let items = menu.pointer("/menuRenderer/items")?.as_array()?;
    items.iter().find_map(|i| {
        let r = i.get("menuNavigationItemRenderer")?;
        (r.pointer("/icon/iconType")?.as_str()? == icon)
            .then(|| target(r.get("navigationEndpoint")?).map(|(t, _)| t))
            .flatten()
    })
}

/// An album, playlist or artist page's header, in any of its shapes.
pub fn header(v: &Value) -> Option<Header> {
    let (key, r) = v.as_object()?.iter().next()?;
    let mut h = Header {
        title: text(&r["title"]),
        ..Header::default()
    };
    match key.as_str() {
        "musicResponsiveHeaderRenderer" | "musicDetailHeaderRenderer" => {
            h.subtitle = opt_text(&r["subtitle"]);
            h.second_subtitle = opt_text(&r["secondSubtitle"]);
            h.description = r
                .pointer("/description/musicDescriptionShelfRenderer/description")
                .and_then(opt_text_ref)
                .or_else(|| opt_text(&r["description"]));
            h.thumbnail = thumbnail(r.get("thumbnail").unwrap_or(&Value::Null));
            let strapline = &r["straplineTextOne"];
            if let Some(first) = runs(strapline).first() {
                h.author = Some(Link::new(
                    text(strapline),
                    first
                        .pointer("/navigationEndpoint/browseEndpoint/browseId")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                ));
            }
            if let Some(buttons) = r["buttons"].as_array() {
                for b in buttons {
                    if let Some(play) = b.pointer("/musicPlayButtonRenderer/playNavigationEndpoint") {
                        h.play = target(play).map(|(t, _)| t);
                    }
                    if let Some(toggle) = b.get("toggleButtonRenderer") {
                        let icon = toggle.pointer("/defaultIcon/iconType").and_then(Value::as_str);
                        if matches!(icon, Some("LIBRARY_ADD" | "BOOKMARK_BORDER" | "LIBRARY_SAVED" | "BOOKMARK")) {
                            h.saved = toggle["isToggled"].as_bool();
                        }
                    }
                    if b.get("menuRenderer").is_some() {
                        h.shuffle = find_menu_target(b, "MUSIC_SHUFFLE");
                        h.radio = find_menu_target(b, "MIX");
                    }
                }
            }
            h.playlist_id = match &h.play {
                Some(Target::WatchPlaylist { playlist_id, .. }) => Some(playlist_id.clone()),
                Some(Target::Watch { playlist_id, .. }) => playlist_id.clone(),
                _ => None,
            };
        }
        "musicEditablePlaylistDetailHeaderRenderer" => {
            let inner = r
                .pointer("/header/musicResponsiveHeaderRenderer")
                .map(|h| serde_json::json!({ "musicResponsiveHeaderRenderer": h }))
                .or_else(|| {
                    r.pointer("/header/musicDetailHeaderRenderer")
                        .map(|h| serde_json::json!({ "musicDetailHeaderRenderer": h }))
                });
            let mut parsed = inner.as_ref().and_then(header).unwrap_or_default();
            parsed.editable = true;
            if parsed.playlist_id.is_none() {
                parsed.playlist_id = r
                    .pointer("/editHeader/musicPlaylistEditHeaderRenderer/playlistId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            return Some(parsed);
        }
        "musicImmersiveHeaderRenderer" | "musicVisualHeaderRenderer" => {
            h.banner = key == "musicImmersiveHeaderRenderer";
            h.description = opt_text(&r["description"]);
            h.thumbnail = thumbnail(r.get("thumbnail").unwrap_or(&Value::Null))
                .or_else(|| thumbnail(r.get("foregroundThumbnail").unwrap_or(&Value::Null)));
            h.subtitle = opt_text(&r["monthlyListenerCount"]);
            if let Some(sub) = r.pointer("/subscriptionButton/subscribeButtonRenderer") {
                h.channel_id = str_of(sub, "channelId");
                h.subscribed = sub["subscribed"].as_bool();
                h.subscribers = opt_text(&sub["subscriberCountText"]);
            }
            h.shuffle = r
                .pointer("/playButton/buttonRenderer/navigationEndpoint")
                .and_then(target)
                .map(|(t, _)| t);
            h.radio = r
                .pointer("/startRadioButton/buttonRenderer/navigationEndpoint")
                .and_then(target)
                .map(|(t, _)| t);
        }
        _ => return None,
    }
    Some(h)
}

fn opt_text_ref(v: &Value) -> Option<String> {
    opt_text(v)
}

/// A browse response: whichever layout it came in.
pub fn page(v: &Value) -> Page {
    let mut page = Page::default();
    if let Some(h) = v.get("header") {
        page.header = header(h);
    }
    let contents = &v["contents"];
    if let Some(two) = contents.get("twoColumnBrowseResultsRenderer") {
        // Albums and playlists: the header in the first column, the songs
        // and more in the second.
        if let Some(first) = two
            .pointer("/tabs/0/tabRenderer/content/sectionListRenderer/contents")
            .and_then(Value::as_array)
        {
            for c in first {
                if page.header.is_none() {
                    page.header = header(c);
                }
            }
        }
        if let Some(list) = two.pointer("/secondaryContents/sectionListRenderer") {
            let contents = list["contents"].as_array().map_or(&[][..], Vec::as_slice);
            for (index, c) in contents.iter().enumerate() {
                let shelf = c
                    .get("musicShelfRenderer")
                    .or_else(|| c.get("musicPlaylistShelfRenderer"));
                if index == 0
                    && let Some(shelf) = shelf
                {
                    let shelf_items = shelf["contents"].as_array().map_or(&[][..], Vec::as_slice);
                    page.tracks = items(shelf_items).into_iter().filter_map(|i| i.track).collect();
                    page.tracks_continuation = continuation(shelf);
                    if page.header.as_ref().is_some_and(|h| h.playlist_id.is_none()) {
                        page.header.as_mut().expect("checked").playlist_id = str_of(shelf, "playlistId");
                    }
                    continue;
                }
                page.sections.extend(sections(std::slice::from_ref(c)));
            }
            page.continuation = continuation(list);
        }
    } else if let Some(single) = contents.get("singleColumnBrowseResultsRenderer") {
        if let Some(list) = single.pointer("/tabs/0/tabRenderer/content/sectionListRenderer") {
            let contents = list["contents"].as_array().map_or(&[][..], Vec::as_slice);
            page.sections = sections(contents);
            page.continuation = continuation(list);
            if let Some(header) = list.get("header") {
                page.chips = chips(header);
            }
            // Some pages put their header in the list.
            for c in contents {
                if page.header.is_none() {
                    page.header = header(c);
                }
            }
        }
        // The library's grid tabs.
        if let Some(grid) = single.pointer("/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/gridRenderer")
            && page.sections.is_empty()
        {
            page.sections = section(&serde_json::json!({ "gridRenderer": grid }));
        }
    }
    // Album songs carry no artwork or album of their own.
    if let Some(h) = &page.header
        && !h.banner
    {
        let album = h.playlist_id.as_deref().is_some_and(|id| id.starts_with("OLAK5uy_"));
        for track in &mut page.tracks {
            if track.thumbnail.is_none() {
                track.thumbnail = h.thumbnail.clone();
            }
            if album && track.album.is_none() {
                track.album = Some(Link::new(h.title.clone(), None));
            }
            if track.artists.is_empty()
                && let Some(author) = &h.author
            {
                track.artists.push(author.clone());
            }
        }
    }
    page
}

/// A continuation answer: more sections, or more items for one shelf.
pub fn continuation_page(v: &Value) -> (Vec<Section>, Vec<Item>, Option<String>) {
    if let Some(cc) = v.get("continuationContents") {
        if let Some(list) = cc.get("sectionListContinuation") {
            let contents = list["contents"].as_array().map_or(&[][..], Vec::as_slice);
            return (sections(contents), Vec::new(), continuation(list));
        }
        for key in ["musicShelfContinuation", "musicPlaylistShelfContinuation", "gridContinuation"] {
            if let Some(shelf) = cc.get(key) {
                let contents = shelf
                    .get("contents")
                    .or_else(|| shelf.get("items"))
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                return (Vec::new(), items(contents), continuation(shelf).or_else(|| item_continuation(contents)));
            }
        }
    }
    if let Some(actions) = v.get("onResponseReceivedActions").and_then(Value::as_array) {
        for action in actions {
            if let Some(list) = action
                .pointer("/appendContinuationItemsAction/continuationItems")
                .and_then(Value::as_array)
            {
                let found = items(list);
                if found.is_empty() {
                    return (sections(list), Vec::new(), item_continuation(list));
                }
                return (Vec::new(), found, item_continuation(list));
            }
        }
    }
    (Vec::new(), Vec::new(), None)
}

pub fn watch_queue(v: &Value) -> WatchQueue {
    let mut queue = WatchQueue::default();
    let tabs = v
        .pointer("/contents/singleColumnMusicWatchNextResultsRenderer/tabbedRenderer/watchNextTabbedResultsRenderer/tabs")
        .and_then(Value::as_array);
    if let Some(tabs) = tabs {
        for tab in tabs {
            let browse = tab
                .pointer("/tabRenderer/endpoint/browseEndpoint/browseId")
                .and_then(Value::as_str);
            match browse {
                Some(id) if id.starts_with("MPLY") => queue.lyrics_id = Some(id.to_owned()),
                Some(id) if id.starts_with("MPTR") => queue.related_id = Some(id.to_owned()),
                _ => {}
            }
        }
        if let Some(panel) = tabs
            .first()
            .and_then(|t| t.pointer("/tabRenderer/content/musicQueueRenderer/content/playlistPanelRenderer"))
        {
            read_panel(panel, &mut queue);
        }
    }
    if let Some(panel) = v.pointer("/continuationContents/playlistPanelContinuation") {
        read_panel(panel, &mut queue);
    }
    queue.like = find_key(v, "likeButtonRenderer", 12)
        .and_then(|b| b["likeStatus"].as_str())
        .map(|status| match status {
            "LIKE" => Like::Liked,
            "DISLIKE" => Like::Disliked,
            _ => Like::Indifferent,
        });
    queue
}

fn read_panel(panel: &Value, queue: &mut WatchQueue) {
    let contents = panel["contents"].as_array().map_or(&[][..], Vec::as_slice);
    queue.tracks.extend(contents.iter().filter_map(panel_video));
    queue.continuation = continuation(panel);
    if queue.playlist_id.is_none() {
        queue.playlist_id = str_of(panel, "playlistId");
    }
}

pub fn lyrics(v: &Value) -> Option<Lyrics> {
    let shelf = find_key(v, "musicDescriptionShelfRenderer", 8)?;
    Some(Lyrics {
        text: opt_text(&shelf["description"])?,
        source: opt_text(&shelf["footer"]),
    })
}

pub fn suggestions(v: &Value) -> Suggestions {
    let mut out = Suggestions::default();
    let sections = v["contents"].as_array().map_or(&[][..], Vec::as_slice);
    for s in sections {
        let contents = s
            .pointer("/searchSuggestionsSectionRenderer/contents")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for c in contents {
            if let Some(r) = c.get("searchSuggestionRenderer") {
                if let Some(q) = r.pointer("/navigationEndpoint/searchEndpoint/query").and_then(Value::as_str) {
                    out.queries.push(q.to_owned());
                }
            } else if let Some(r) = c.get("historySuggestionRenderer") {
                if let Some(q) = r.pointer("/navigationEndpoint/searchEndpoint/query").and_then(Value::as_str) {
                    out.queries.push(q.to_owned());
                }
            }
        }
        out.items.extend(items(contents));
    }
    out
}

pub fn search(v: &Value) -> (Vec<Section>, Vec<Chip>, Option<String>) {
    let Some(list) = v
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer")
    else {
        let (sections, items, token) = continuation_page(v);
        let mut sections = sections;
        if !items.is_empty() {
            sections.push(Section {
                title: String::new(),
                strapline: None,
                layout: Layout::List,
                items,
                more: None,
                text: None,
                continuation: token.clone(),
            });
        }
        return (sections, Vec::new(), token);
    };
    let contents = list["contents"].as_array().map_or(&[][..], Vec::as_slice);
    let found = sections(contents);
    let token = found.iter().rev().find_map(|s| s.continuation.clone()).or_else(|| continuation(list));
    (found, list.get("header").map(chips).unwrap_or_default(), token)
}

pub fn account(v: &Value) -> Option<Account> {
    let header = find_key(v, "activeAccountHeaderRenderer", 12)?;
    Some(Account {
        name: text(&header["accountName"]),
        handle: opt_text(&header["channelHandle"]),
        photo: thumbnail(header.get("accountPhoto").unwrap_or(&Value::Null)),
    })
}

/// The first value under `key`, searching depth-first.
pub fn find_key<'a>(v: &'a Value, key: &str, depth: u32) -> Option<&'a Value> {
    if depth == 0 {
        return None;
    }
    match v {
        Value::Object(map) => {
            if let Some(found) = map.get(key) {
                return Some(found);
            }
            map.values().find_map(|x| find_key(x, key, depth - 1))
        }
        Value::Array(list) => list.iter().find_map(|x| find_key(x, key, depth - 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn song_row_from_search() {
        let row = json!({
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Starboy", "navigationEndpoint": {"watchEndpoint": {"videoId": "abc", "watchEndpointMusicSupportedConfigs": {"watchEndpointMusicConfig": {"musicVideoType": "MUSIC_VIDEO_TYPE_ATV"}}}}}]}}},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                    {"text": "Song"}, {"text": " • "},
                    {"text": "The Weeknd", "navigationEndpoint": {"browseEndpoint": {"browseId": "UC1", "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}}},
                    {"text": " • "},
                    {"text": "Starboy", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_1", "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}},
                    {"text": " • "}, {"text": "3:50"}
                ]}}}
            ]
        });
        let item = responsive_item(&row).expect("item");
        assert_eq!(item.kind, Kind::Song);
        let track = item.track.expect("track");
        assert_eq!(track.video_id, "abc");
        assert_eq!(track.artists, vec![Link::new("The Weeknd", Some("UC1".into()))]);
        assert_eq!(track.album, Some(Link::new("Starboy", Some("MPREb_1".into()))));
        assert_eq!(track.duration, Some(230));
    }

    #[test]
    fn album_card() {
        let card = json!({
            "title": {"runs": [{"text": "Discovery"}]},
            "subtitle": {"runs": [{"text": "Album"}, {"text": " • "}, {"text": "2001"}]},
            "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_x", "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}
        });
        let item = two_row_item(&card).expect("item");
        assert_eq!(item.kind, Kind::Album);
        assert!(item.track.is_none());
        assert_eq!(item.subtitle, "Album • 2001");
    }

    #[test]
    fn compilation_artists_without_links() {
        let line = byline(&[json!({"text": "HYOLYN"})], true);
        assert_eq!(line.artists, vec![Link::new("HYOLYN", None)]);
        let line = byline(&[json!({"text": "8M plays"})], true);
        assert!(line.artists.is_empty());
    }
}
