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
    /// A better stream of the song already starting, to switch to.
    Upgrade,
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
    /// YouTube's lyrics (by their browse id), and synced ones from LRCLIB
    /// (by the song's names).
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
    CreatePlaylist {
        title: String,
        video_ids: Vec<String>,
    },
    Account,
    LibraryPlaylists,
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
    Lyrics {
        video_id: String,
        result: Result<Option<Lyrics>, String>,
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
            if let Some(response) = run(&api, &resolver, request).await {
                let _ = tx.send(response);
                wake();
            }
        });
    }
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

async fn run(
    api: &Innertube,
    resolver: &parking_lot::RwLock<Resolver>,
    request: Request,
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
                    Some(query) => crate::api::lrclib::find(&api.http, query).await.unwrap_or_else(|error| {
                        log::info!("LRCLIB: {error:#}");
                        None
                    }),
                    None => None,
                }
            };
            let (youtube, open) = tokio::join!(youtube, open);
            // Synced first; then YouTube's own text; then LRCLIB's.
            let result = match (open, youtube) {
                (Some(synced), _) if !synced.lines.is_empty() => Ok(Some(synced)),
                (_, Ok(Some(text))) => Ok(Some(text)),
                (Some(plain), _) => Ok(Some(plain)),
                (None, other) => other,
            };
            Response::Lyrics { video_id, result }
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
