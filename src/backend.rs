//! Network work, on a small tokio runtime. The interface sends requests and
//! reads answers each frame; nothing here ever blocks it.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::api::client::{Auth, Innertube};
use crate::api::model::*;
use crate::api::music::{Filter, Rating};
use crate::player::StreamInfo;
use crate::stream::{Resolver, Via};

/// Which view a page answer belongs to.
pub type PageKey = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Play it now.
    Load,
    /// Have it ready for a gapless start after the current track.
    Preload,
    /// Only find it, for later: resolving is slow.
    Prefetch,
    /// Found while the pointer rests on it, so a click starts it at once.
    Prewarm,
    /// A better stream of the song already starting, to switch to.
    Upgrade,
    /// Downloading the song for offline listening.
    Download,
}

/// Which way to resolve a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// The best the settings allow, falling back as needed.
    Full,
    /// The signed-out clients only: quick, for a fast start.
    Native,
    /// yt-dlp with the account: Premium quality, taking seconds.
    Account,
}

pub enum Request {
    Bootstrap,
    Browse {
        key: PageKey,
        browse_id: String,
        params: Option<String>,
    },
    More {
        key: PageKey,
        token: String,
        /// More songs of an album or playlist, rather than more sections.
        tracks: bool,
    },
    Search {
        seq: u64,
        query: String,
        filter: Filter,
    },
    SearchMore {
        seq: u64,
        token: String,
    },
    Suggest {
        seq: u64,
        input: String,
    },
    /// The up-next queue: its songs, lyrics and rating.
    Watch {
        seq: u64,
        video_id: Option<String>,
        playlist_id: Option<String>,
        params: Option<String>,
    },
    WatchMore {
        seq: u64,
        token: String,
        playlist_id: Option<String>,
    },
    /// A whole playlist's songs, to play it.
    PlaylistTracks {
        seq: u64,
        playlist_id: String,
    },
    /// YouTube's lyrics (by their browse id), and timed ones from Better
    /// Lyrics' sources and LRCLIB (by the song's names).
    Lyrics {
        video_id: String,
        browse_id: Option<String>,
        query: Option<crate::api::lrclib::Query>,
    },
    Resolve {
        token: u64,
        video_id: String,
        purpose: Purpose,
        start: f64,
        how: How,
    },
    Rate {
        video_id: String,
        rating: Rating,
    },
    SavePlaylist {
        playlist_id: String,
        save: bool,
    },
    Subscribe {
        channel_id: String,
        subscribe: bool,
    },
    AddToPlaylist {
        playlist_id: String,
        title: String,
        video_ids: Vec<String>,
    },
    RemoveFromPlaylist {
        key: PageKey,
        playlist_id: String,
        video_id: String,
        set_video_id: String,
    },
    /// The person's stats from Last.fm over a stretch.
    LastFmStats {
        period: crate::stats::Period,
        reader: crate::lastfm::Reader,
    },
    RenamePlaylist {
        playlist_id: String,
        title: String,
    },
    DeletePlaylist {
        playlist_id: String,
        title: String,
    },
    CreatePlaylist {
        title: String,
        video_ids: Vec<String>,
    },
    Account,
    LibraryPlaylists,
    /// The song started: into the account's history.
    History {
        video_id: String,
    },
    SetAuth(Option<Auth>),
    SetLanguage {
        language: String,
        region: Option<String>,
    },
    SetYtDlp {
        enabled: bool,
        path: Option<std::path::PathBuf>,
    },
    /// Premium's 256 kb/s streams, through yt-dlp, when signed in.
    SetPremium(bool),
    /// An artist's page: Last.fm's story and numbers (with the person's
    /// API account), and their page on YouTube Music.
    Artist {
        name: String,
        lastfm: Option<crate::lastfm::Reader>,
        /// Their songs on this PC, to find them on YouTube Music by.
        titles: Vec<String>,
    },
    /// A picture for songs on this PC that carry none: their album's on
    /// Last.fm, else on YouTube Music.
    FindArt {
        query: crate::local::ArtQuery,
        lastfm: Option<crate::lastfm::Reader>,
    },
    /// A song on YouTube Music, by its names, to play: by `artist` alone,
    /// when given.
    FindSong {
        seq: u64,
        query: String,
        artist: Option<String>,
    },
    /// A song on YouTube Music to start a radio after a song on this PC
    /// from: the same song by the same artist; else, when that artist is
    /// not on YouTube Music, a song by one like them (from Last.fm); else
    /// the same song by anyone (a cover).
    RadioSeed {
        seq: u64,
        artist: String,
        title: String,
        lastfm: Option<crate::lastfm::Reader>,
    },
}

pub enum Response {
    Ready,
    Page {
        key: PageKey,
        result: Result<Page, String>,
    },
    More {
        key: PageKey,
        sections: Vec<Section>,
        items: Vec<Item>,
        token: Option<String>,
        tracks: bool,
    },
    Search {
        seq: u64,
        result: Result<(Vec<Section>, Option<String>), String>,
        more: bool,
    },
    Suggest {
        seq: u64,
        result: Suggestions,
    },
    Watch {
        seq: u64,
        result: Result<WatchQueue, String>,
        more: bool,
    },
    PlaylistTracks {
        seq: u64,
        result: Result<Vec<Track>, String>,
    },
    LastFmStats {
        period: crate::stats::Period,
        result: Result<crate::stats::Summary, String>,
    },
    Lyrics {
        video_id: String,
        result: Result<Option<Lyrics>, String>,
        /// Every source asked answered: what was found can be kept. Not so
        /// when LRCLIB was out of reach or turned the request away (it
        /// limits how often it is asked), or Better Lyrics' server could
        /// not be asked, and the answer is a stopgap.
        complete: bool,
        /// Shown while a slower source is still being waited for; the
        /// answer itself follows.
        partial: bool,
    },
    Resolved {
        token: u64,
        video_id: String,
        purpose: Purpose,
        start: f64,
        result: Result<(StreamInfo, Via), String>,
    },
    Account(Result<Account, String>),
    LibraryPlaylists(Result<Vec<Item>, String>),
    Artist {
        name: String,
        /// None without a Last.fm account.
        lastfm: Option<Result<crate::lastfm::ArtistInfo, String>>,
        /// Their artist card on YouTube Music: page and picture.
        youtube: Option<Item>,
    },
    ArtFound {
        key: String,
        url: Option<String>,
    },
    SongFound {
        seq: u64,
        track: Option<Track>,
    },
    /// Something the person did went through, or did not.
    Done {
        message: String,
        ok: bool,
        /// A page to reload, as after removing a song from a playlist.
        refresh: Option<PageKey>,
    },
}

pub struct Backend {
    pub runtime: tokio::runtime::Runtime,
    pub http: reqwest::Client,
    api: Arc<Innertube>,
    resolver: Arc<parking_lot::RwLock<Resolver>>,
    tx: Sender<Response>,
    rx: Receiver<Response>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Backend {
    pub fn new(language: &str, resolver: Resolver, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("network")
            .enable_all()
            .build()
            .expect("tokio runtime");
        let http = reqwest::Client::builder()
            .gzip(true)
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("http client");
        let api = Arc::new(Innertube::new(http.clone(), language));
        let (tx, rx) = channel();
        Self {
            runtime,
            http,
            api,
            resolver: Arc::new(parking_lot::RwLock::new(resolver)),
            tx,
            rx,
            wake,
        }
    }

    pub fn signed_in(&self) -> bool {
        self.api.signed_in()
    }

    pub fn ytdlp_found(&self) -> Option<std::path::PathBuf> {
        self.resolver.read().ytdlp.clone()
    }

    pub fn responses(&self) -> Vec<Response> {
        self.rx.try_iter().collect()
    }

    pub fn send(&self, request: Request) {
        // Settings that change the client apply at once, in order.
        match request {
            Request::SetAuth(auth) => {
                self.api.set_auth(auth);
                return;
            }
            Request::SetLanguage { language, region } => {
                self.api.set_language(&language);
                self.api.set_region(region);
                return;
            }
            Request::SetYtDlp { enabled, path } => {
                let mut resolver = self.resolver.write();
                resolver.use_ytdlp = enabled;
                if path.is_some() {
                    resolver.ytdlp = path;
                } else {
                    let (premium, scratch) = (resolver.premium, resolver.scratch.clone());
                    *resolver = Resolver::new(None, enabled, premium, scratch);
                }
                return;
            }
            Request::SetPremium(on) => {
                self.resolver.write().premium = on;
                return;
            }
            _ => {}
        }
        let api = self.api.clone();
        let resolver = self.resolver.clone();
        let tx = self.tx.clone();
        let wake = self.wake.clone();
        self.runtime.spawn(async move {
            let early = |response: Response| {
                let _ = tx.send(response);
                wake();
            };
            if let Some(response) = run(&api, &resolver, request, &early).await {
                let _ = tx.send(response);
                wake();
            }
        });
    }
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// Whether two names are the same, near enough: one holds the other once
/// case, spaces and marks are set aside.
fn same_name(a: &str, b: &str) -> bool {
    let (a, b) = (crate::local::loose(a), crate::local::loose(b));
    !a.is_empty() && !b.is_empty() && (a.contains(&b) || b.contains(&a))
}

/// The first item a search finds of a kind, under the name looked for and,
/// when given, by the artist looked for: an album of the same name by
/// someone else is not theirs.
async fn first_found(api: &Innertube, query: &str, filter: Filter, name: &str, by: Option<&str>, kind: &[Kind]) -> Option<Item> {
    let (sections, _) = api.search(query, filter).await.ok()?;
    sections.into_iter().flat_map(|s| s.items).find(|item| {
        let artists = match &item.track {
            Some(track) => track.artist_line(),
            None => item.subtitle.clone(),
        };
        kind.contains(&item.kind)
            && same_name(&item.title, name)
            && by.is_none_or(|by| {
                let by = crate::local::loose(by);
                !by.is_empty() && crate::local::loose(&artists).contains(&by)
            })
    })
}

/// Whether a song is by an artist (one of its artists, near enough).
fn by_artist(track: &Track, artist: &str) -> bool {
    let artist = crate::local::loose(artist);
    !artist.is_empty() && crate::local::loose(&track.artist_line()).contains(&artist)
}

/// The first song a search finds by `by` and named `title`, as far as
/// either is asked for.
async fn find_song(api: &Innertube, query: &str, by: Option<&str>, title: Option<&str>) -> Option<Track> {
    let (sections, _) = api.search(query, Filter::Songs).await.ok()?;
    sections
        .into_iter()
        .flat_map(|s| s.items)
        .filter_map(|i| i.track)
        .find(|t| by.is_none_or(|by| by_artist(t, by)) && title.is_none_or(|title| same_name(&t.title, title)))
}

/// An artist's card on YouTube Music: from a search for them, or, when
/// that turns up others (a search for "XX:me" finds XXXTENTACION), from
/// one of their songs, whose artist leads to their page.
async fn youtube_artist(api: &Innertube, name: &str, titles: &[String]) -> Option<Item> {
    if let Some(item) = first_found(api, name, Filter::Artists, name, None, &[Kind::Artist]).await {
        return Some(item);
    }
    for title in titles.iter().take(2) {
        let Ok((sections, _)) = api.search(&format!("{name} {title}"), Filter::Songs).await else {
            continue;
        };
        let found = sections
            .into_iter()
            .flat_map(|s| s.items)
            .filter_map(|i| i.track)
            .flat_map(|t| t.artists)
            .find(|a| a.id.as_deref().is_some_and(|id| id.starts_with("UC")) && same_name(&a.name, name));
        let Some(Link { name: found_name, id: Some(id) }) = found else {
            continue;
        };
        let thumbnail = api.browse(&id, None).await.ok().and_then(|p| p.header).and_then(|h| h.thumbnail);
        return Some(Item {
            kind: Kind::Artist,
            title: found_name,
            subtitle: String::new(),
            thumbnail,
            target: Some(Target::Browse { browse_id: id, params: None }),
            track: None,
            play_playlist: None,
            color: None,
        });
    }
    None
}

/// A picture on YouTube Music for songs on this PC: their album's, or the
/// song's own.
async fn youtube_art(api: &Innertube, query: &crate::local::ArtQuery) -> Option<String> {
    let by = Some(query.artist.as_str());
    if let Some(album) = &query.album
        && let Some(item) = first_found(api, &format!("{} {album}", query.artist), Filter::Albums, album, by, &[Kind::Album]).await
    {
        return item.thumbnail;
    }
    let song = first_found(api, &format!("{} {}", query.artist, query.title), Filter::Songs, &query.title, by, &[Kind::Song, Kind::Video]).await?;
    song.track.and_then(|t| t.thumbnail).or(song.thumbnail)
}

/// `early` takes an answer given before the request's own, as lyrics
/// shown while a slower source is waited for.
async fn run(
    api: &Innertube,
    resolver: &parking_lot::RwLock<Resolver>,
    request: Request,
    early: &(dyn Fn(Response) + Sync),
) -> Option<Response> {
    Some(match request {
        Request::Bootstrap => {
            api.bootstrap().await;
            Response::Ready
        }
        Request::Browse {
            key,
            browse_id,
            params,
        } => Response::Page {
            key,
            result: api.browse(&browse_id, params.as_deref()).await.map_err(err),
        },
        Request::More { key, token, tracks } => match api.more(&token).await {
            Ok((sections, items, token)) => Response::More {
                key,
                sections,
                items,
                token,
                tracks,
            },
            Err(error) => {
                log::warn!("more: {error:#}");
                Response::More {
                    key,
                    sections: Vec::new(),
                    items: Vec::new(),
                    token: None,
                    tracks,
                }
            }
        },
        Request::Search { seq, query, filter } => Response::Search {
            seq,
            result: api.search(&query, filter).await.map_err(err),
            more: false,
        },
        Request::SearchMore { seq, token } => Response::Search {
            seq,
            result: api.search_more(&token).await.map_err(err),
            more: true,
        },
        Request::Artist { name, lastfm, titles } => {
            let story = async {
                match &lastfm {
                    Some(reader) => Some(reader.artist(&api.http, &name).await),
                    None => None,
                }
            };
            let card = youtube_artist(api, &name, &titles);
            let (lastfm, mut youtube) = tokio::join!(story, card);
            // No songs of theirs on this PC to find them by: Last.fm's most
            // played will do.
            if youtube.is_none()
                && titles.is_empty()
                && let Some(Ok(info)) = &lastfm
            {
                let popular: Vec<String> = info.top_tracks.iter().map(|(title, _)| title.clone()).collect();
                youtube = youtube_artist(api, &name, &popular).await;
            }
            Response::Artist { name, lastfm, youtube }
        }
        Request::FindArt { query, lastfm } => {
            let mut url = None;
            let mut from = "none";
            if let Some(reader) = &lastfm {
                if let Some(album) = &query.album {
                    url = reader.album_art(&api.http, &query.artist, album).await;
                }
                if url.is_none() {
                    url = reader.track_art(&api.http, &query.artist, &query.title).await;
                }
                if url.is_some() {
                    from = "Last.fm";
                }
            }
            if url.is_none() {
                url = youtube_art(api, &query).await;
                if url.is_some() {
                    from = "YouTube Music";
                }
            }
            log::info!("artwork for {:?} by {}: {from}", query.album.as_deref().unwrap_or(&query.title), query.artist);
            Response::ArtFound { key: query.key, url }
        }
        Request::FindSong { seq, query, artist } => Response::SongFound {
            seq,
            track: find_song(api, &query, artist.as_deref(), None).await,
        },
        Request::RadioSeed { seq, artist, title, lastfm } => {
            let query = format!("{artist} {title}");
            let mut track = find_song(api, &query, Some(&artist), Some(&title)).await;
            if track.is_none()
                && let Some(reader) = &lastfm
            {
                for other in reader.similar(&api.http, &artist).await.into_iter().take(4) {
                    track = find_song(api, &other, Some(&other), None).await;
                    if track.is_some() {
                        break;
                    }
                }
            }
            if track.is_none() {
                track = find_song(api, &query, None, Some(&title)).await;
            }
            Response::SongFound { seq, track }
        }
        Request::Suggest { seq, input } => Response::Suggest {
            seq,
            result: api.suggestions(&input).await.unwrap_or_default(),
        },
        Request::Watch {
            seq,
            video_id,
            playlist_id,
            params,
        } => Response::Watch {
            seq,
            result: api
                .watch(video_id.as_deref(), playlist_id.as_deref(), params.as_deref())
                .await
                .map_err(err),
            more: false,
        },
        Request::WatchMore {
            seq,
            token,
            playlist_id,
        } => Response::Watch {
            seq,
            result: api.watch_more(&token, playlist_id.as_deref()).await.map_err(err),
            more: true,
        },
        Request::PlaylistTracks { seq, playlist_id } => Response::PlaylistTracks {
            seq,
            result: api.playlist_tracks(&playlist_id, 1000).await.map_err(err),
        },
        Request::Lyrics {
            video_id,
            browse_id,
            query,
        } => {
            let youtube = async {
                match &browse_id {
                    Some(id) => api.lyrics(id).await.map_err(err),
                    None => Ok(None),
                }
            };
            let open = async {
                match &query {
                    Some(query) => match crate::api::lrclib::find(&api.http, query).await {
                        Ok(found) => (found, true),
                        Err(error) => {
                            log::info!("LRCLIB: {error:#}");
                            (None, false)
                        }
                    },
                    None => (None, true),
                }
            };
            // Better Lyrics' sources, by the song's YouTube Music id: for a
            // song on this PC, its twin there.
            let better = match &query {
                Some(query) => {
                    let id = if crate::local::is_local_id(&video_id) {
                        let search = format!("{} {}", query.title, query.artist);
                        find_song(api, &search, Some(&query.artist), Some(&query.title)).await.map(|t| t.video_id)
                    } else {
                        Some(video_id.clone())
                    };
                    id.map(|id| crate::api::betterlyrics::Query {
                        video_id: id,
                        title: query.title.clone(),
                        artist: query.artist.clone(),
                        album: query.album.clone(),
                        duration: query.duration,
                    })
                }
                None => None,
            };
            let better = better.map(|query| {
                let http = api.http.clone();
                tokio::spawn(async move { crate::api::betterlyrics::find(&http, &query).await })
            });
            let (youtube, (open, answered)) = tokio::join!(youtube, open);
            // LRCLIB (the app's own look, when Better Lyrics' has none) and
            // YouTube's text, in their places among Better Lyrics' sources.
            use crate::api::betterlyrics::{self as bl, Found};
            let mut found: Vec<Found> = Vec::new();
            if let Some(open) = open {
                let rank = if open.lines.is_empty() { bl::LRCLIB_TEXT } else { bl::LRCLIB_SYNCED };
                found.push(Found { rank, lyrics: open });
            }
            if let Ok(Some(text)) = &youtube {
                found.push(Found { rank: bl::YOUTUBE_TEXT, lyrics: text.clone() });
            }
            let mut better_answered = true;
            if let Some(better) = better {
                // Better Lyrics' server can take its time over a song it has
                // not looked up before: what is here shows meanwhile.
                if !better.is_finished()
                    && let Some(meanwhile) = bl::gather(found.clone())
                {
                    early(Response::Lyrics { video_id: video_id.clone(), result: Ok(Some(meanwhile)), complete: false, partial: true });
                }
                match better.await {
                    Ok(Ok(more)) => {
                        // LRCLIB once: Better Lyrics' look at it, if it made one.
                        if more.iter().any(|f| f.lyrics.provider.as_deref() == Some("LRCLIB")) {
                            found.retain(|f| f.lyrics.provider.as_deref() != Some("LRCLIB"));
                        }
                        found.extend(more);
                    }
                    Ok(Err(error)) => {
                        log::info!("Better Lyrics: {error:#}");
                        better_answered = false;
                    }
                    Err(_) => better_answered = false,
                }
            }
            let complete = answered && youtube.is_ok() && better_answered;
            let mut ranked: Vec<&Found> = found.iter().collect();
            ranked.sort_by_key(|f| f.rank);
            let names: Vec<String> = ranked.iter().map(|f| format!("{} ({})", f.lyrics.provider.as_deref().unwrap_or("?"), f.lyrics.timing())).collect();
            log::info!("lyrics found: {}", if names.is_empty() { "none".to_owned() } else { names.join(", ") });
            let result = match bl::gather(found) {
                Some(best) => Ok(Some(best)),
                None => youtube,
            };
            Response::Lyrics { video_id, result, complete, partial: false }
        }
        Request::Resolve {
            token,
            video_id,
            purpose,
            start,
            how,
        } => {
            // The resolver is cheap to copy; the lock is not held across the
            // network.
            let snapshot = resolver.read().clone();
            let started = std::time::Instant::now();
            let result = match how {
                How::Full => snapshot.resolve(api, &video_id).await,
                How::Native => snapshot.resolve_native(api, &video_id).await,
                How::Account => snapshot.resolve_account(api, &video_id).await,
            }
            .map_err(err);
            if let Ok((_, via)) = &result {
                log::info!("resolved {video_id} via {via:?} in {:?}", started.elapsed());
            }
            Response::Resolved {
                token,
                video_id,
                purpose,
                start,
                result,
            }
        }
        Request::Rate { video_id, rating } => {
            let result = api.rate(&video_id, rating).await;
            done(result, match rating {
                Rating::Like => "Added to your likes",
                Rating::Dislike => "Disliked",
                Rating::None => "Rating removed",
            }, None)
        }
        Request::SavePlaylist { playlist_id, save } => done(
            api.save_playlist(&playlist_id, save).await,
            if save { "Saved to library" } else { "Removed from library" },
            None,
        ),
        Request::Subscribe {
            channel_id,
            subscribe,
        } => done(
            api.subscribe(&channel_id, subscribe).await,
            if subscribe { "Subscribed" } else { "Unsubscribed" },
            None,
        ),
        Request::AddToPlaylist {
            playlist_id,
            title,
            video_ids,
        } => done(
            api.add_to_playlist(&playlist_id, &video_ids).await,
            &format!("Saved to {title}"),
            Some(format!("VL{}", playlist_id.trim_start_matches("VL"))),
        ),
        Request::RemoveFromPlaylist {
            key,
            playlist_id,
            video_id,
            set_video_id,
        } => done(
            api.remove_from_playlist(&playlist_id, &video_id, &set_video_id)
                .await,
            "Removed from playlist",
            Some(key),
        ),
        Request::LastFmStats { period, reader } => Response::LastFmStats {
            period,
            result: reader.stats(&api.http, period).await,
        },
        Request::RenamePlaylist { playlist_id, title } => done(
            api.rename_playlist(&playlist_id, &title).await,
            &format!("Renamed to {title}"),
            Some("library-playlists".into()),
        ),
        Request::DeletePlaylist { playlist_id, title } => done(
            api.delete_playlist(&playlist_id).await,
            &format!("Deleted {title}"),
            Some("library-playlists".into()),
        ),
        Request::CreatePlaylist { title, video_ids } => match api.create_playlist(&title, true).await {
            Ok(id) => {
                if !video_ids.is_empty() {
                    let _ = api.add_to_playlist(&id, &video_ids).await;
                }
                Response::Done {
                    message: format!("Created {title}"),
                    ok: true,
                    refresh: Some("library-playlists".into()),
                }
            }
            Err(error) => Response::Done {
                message: format!("Could not create the playlist: {error}"),
                ok: false,
                refresh: None,
            },
        },
        Request::Account => Response::Account(api.account().await.map_err(err)),
        Request::LibraryPlaylists => {
            Response::LibraryPlaylists(api.library_playlists().await.map_err(err))
        }
        Request::History { video_id } => {
            match api.add_to_history(&video_id).await {
                Ok(()) => log::info!("{video_id} added to the history"),
                Err(error) => log::info!("history: {error:#}"),
            }
            return None;
        }
        Request::SetAuth(_)
        | Request::SetLanguage { .. }
        | Request::SetYtDlp { .. }
        | Request::SetPremium(_) => return None,
    })
}

fn done(result: anyhow::Result<()>, message: &str, refresh: Option<PageKey>) -> Response {
    match result {
        Ok(()) => Response::Done {
            message: message.to_owned(),
            ok: true,
            refresh,
        },
        Err(error) => Response::Done {
            message: format!("That did not work: {error}"),
            ok: false,
            refresh: None,
        },
    }
}
