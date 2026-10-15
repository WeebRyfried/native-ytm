//! YouTube Music's requests, each returning the model.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::client::Innertube;
use super::model::*;
use super::parse;

/// Search filters, as the web app's chips send them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    All,
    Songs,
    Videos,
    Albums,
    Artists,
    CommunityPlaylists,
    FeaturedPlaylists,
    Podcasts,
}

impl Filter {
    pub const ALL: [Filter; 8] = [
        Filter::All,
        Filter::Songs,
        Filter::Videos,
        Filter::Albums,
        Filter::Artists,
        Filter::CommunityPlaylists,
        Filter::FeaturedPlaylists,
        Filter::Podcasts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Songs => "Songs",
            Filter::Videos => "Videos",
            Filter::Albums => "Albums",
            Filter::Artists => "Artists",
            Filter::CommunityPlaylists => "Community playlists",
            Filter::FeaturedPlaylists => "Featured playlists",
            Filter::Podcasts => "Podcasts",
        }
    }

    /// The `params` the search endpoint takes for this filter.
    fn params(self) -> Option<&'static str> {
        match self {
            Filter::All => None,
            Filter::Songs => Some("EgWKAQIIAWoSEAUQAxAJEAoQBBAQEA4QFRAR"),
            Filter::Videos => Some("EgWKAQIQAWoSEAUQAxAJEAoQBBAQEA4QFRAR"),
            Filter::Albums => Some("EgWKAQIYAWoSEAUQAxAJEAoQBBAQEA4QFRAR"),
            Filter::Artists => Some("EgWKAQIgAWoSEAUQAxAJEAoQBBAQEA4QFRAR"),
            Filter::CommunityPlaylists => Some("EgeKAQQoAEABahIQBRADEAkQChAEEBAQDhAVEBE%3D"),
            Filter::FeaturedPlaylists => Some("EgeKAQQoADgBahIQBRADEAkQChAEEBAQDhAVEBE%3D"),
            Filter::Podcasts => Some("EgWKAQJQAWoSEAUQAxAJEAoQBBAQEA4QFRAR"),
        }
    }
}

impl Innertube {
    pub async fn browse(&self, browse_id: &str, params: Option<&str>) -> Result<Page> {
        let mut body = json!({ "browseId": browse_id });
        if let Some(params) = params {
            body["params"] = json!(params);
        }
        let response = self.post("browse", body).await?;
        let mut page = parse::page(&response);
        if browse_id.starts_with("MPRE") {
            for track in &mut page.tracks {
                if let Some(album) = &mut track.album
                    && album.id.is_none()
                {
                    album.id = Some(browse_id.to_owned());
                }
            }
        }
        // A library page reached without an account answers with a sign-in
        // prompt and nothing else.
        if page.sections.is_empty() && page.tracks.is_empty() && page.header.is_none() {
            if let Some(message) = parse::find_key(&response, "musicNotifierShelfRenderer", 10)
                .or_else(|| parse::find_key(&response, "messageRenderer", 10))
            {
                let text = parse::text(&message["description"]);
                if !text.is_empty() {
                    bail!("{text}");
                }
            }
        }
        Ok(page)
    }

    pub async fn home(&self, params: Option<&str>) -> Result<Page> {
        self.browse("FEmusic_home", params).await
    }

    /// More sections of a page, or more items of a shelf.
    pub async fn more(&self, token: &str) -> Result<(Vec<Section>, Vec<Item>, Option<String>)> {
        let response = self.post("browse", json!({ "continuation": token })).await?;
        let mut parsed = parse::continuation_page(&response);
        if parsed.0.is_empty() && parsed.1.is_empty() {
            // Older shelves still take the token in the query.
            let q = urlencode(token);
            let response = self
                .post_query("browse", &format!("&ctoken={q}&continuation={q}&type=next"), json!({}))
                .await?;
            parsed = parse::continuation_page(&response);
        }
        Ok(parsed)
    }

    pub async fn search(
        &self,
        query: &str,
        filter: Filter,
    ) -> Result<(Vec<Section>, Option<String>)> {
        let mut body = json!({ "query": query });
        if let Some(params) = filter.params() {
            body["params"] = json!(params);
        }
        let response = self.post("search", body).await?;
        let (sections, _chips, token) = parse::search(&response);
        Ok((sections, token))
    }

    pub async fn search_more(&self, token: &str) -> Result<(Vec<Section>, Option<String>)> {
        let q = urlencode(token);
        let response = self
            .post_query("search", &format!("&ctoken={q}&continuation={q}&type=next"), json!({}))
            .await?;
        let (mut sections, items, token) = parse::continuation_page(&response);
        if let Some(shelf) = response.pointer("/continuationContents/musicShelfContinuation") {
            let contents = shelf["contents"].as_array().map_or(&[][..], Vec::as_slice);
            let found = parse::items(contents);
            if !found.is_empty() && items.is_empty() {
                sections.push(Section {
                    title: String::new(),
                    strapline: None,
                    layout: Layout::List,
                    items: found,
                    more: None,
                    text: None,
                    continuation: None,
                });
            }
        }
        if !items.is_empty() {
            sections.push(Section {
                title: String::new(),
                strapline: None,
                layout: Layout::List,
                items,
                more: None,
                text: None,
                continuation: None,
            });
        }
        Ok((sections, token))
    }

    pub async fn suggestions(&self, input: &str) -> Result<Suggestions> {
        let response = self
            .post("music/get_search_suggestions", json!({ "input": input }))
            .await?;
        Ok(parse::suggestions(&response))
    }

    /// The up-next queue for a video: its playlist when it plays from one,
    /// its radio otherwise.
    pub async fn watch(
        &self,
        video_id: Option<&str>,
        playlist_id: Option<&str>,
        params: Option<&str>,
    ) -> Result<WatchQueue> {
        let mut body = json!({
            "isAudioOnly": true,
            "enablePersistentPlaylistPanel": true,
            "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
        });
        if let Some(video_id) = video_id {
            body["videoId"] = json!(video_id);
        }
        match playlist_id {
            Some(playlist_id) => body["playlistId"] = json!(playlist_id),
            None => {
                if let Some(video_id) = video_id {
                    body["playlistId"] = json!(format!("RDAMVM{video_id}"));
                    body["params"] = json!("wAEB");
                }
            }
        }
        if let Some(params) = params {
            body["params"] = json!(params);
        }
        let response = self.post("next", body).await?;
        Ok(parse::watch_queue(&response))
    }

    pub async fn watch_more(&self, token: &str, playlist_id: Option<&str>) -> Result<WatchQueue> {
        let mut body = json!({
            "continuation": token,
            "isAudioOnly": true,
            "enablePersistentPlaylistPanel": true,
        });
        if let Some(playlist_id) = playlist_id {
            body["playlistId"] = json!(playlist_id);
        }
        let response = self.post("next", body).await?;
        Ok(parse::watch_queue(&response))
    }

    pub async fn lyrics(&self, browse_id: &str) -> Result<Option<Lyrics>> {
        let response = self.post("browse", json!({ "browseId": browse_id })).await?;
        Ok(parse::lyrics(&response))
    }

    /// The songs of a playlist in full, following its continuations.
    pub async fn playlist_tracks(&self, playlist_id: &str, limit: usize) -> Result<Vec<Track>> {
        let browse_id = if playlist_id.starts_with("VL") {
            playlist_id.to_owned()
        } else {
            format!("VL{playlist_id}")
        };
        let page = self.browse(&browse_id, None).await?;
        let mut tracks = page.tracks;
        let mut token = page.tracks_continuation;
        while let Some(t) = token.take() {
            if tracks.len() >= limit {
                break;
            }
            let (_, items, next) = self.more(&t).await?;
            let before = tracks.len();
            tracks.extend(items.into_iter().filter_map(|i| i.track));
            if tracks.len() == before {
                break;
            }
            token = next;
        }
        Ok(tracks)
    }

    pub async fn rate(&self, video_id: &str, rating: Rating) -> Result<()> {
        let endpoint = match rating {
            Rating::Like => "like/like",
            Rating::Dislike => "like/dislike",
            Rating::None => "like/removelike",
        };
        self.post(endpoint, json!({ "target": { "videoId": video_id } }))
            .await?;
        Ok(())
    }

    pub async fn save_playlist(&self, playlist_id: &str, save: bool) -> Result<()> {
        let endpoint = if save { "like/like" } else { "like/removelike" };
        let id = playlist_id.trim_start_matches("VL");
        self.post(endpoint, json!({ "target": { "playlistId": id } }))
            .await?;
        Ok(())
    }

    pub async fn subscribe(&self, channel_id: &str, subscribe: bool) -> Result<()> {
        let endpoint = if subscribe {
            "subscription/subscribe"
        } else {
            "subscription/unsubscribe"
        };
        self.post(endpoint, json!({ "channelIds": [channel_id] }))
            .await?;
        Ok(())
    }

    pub async fn add_to_playlist(&self, playlist_id: &str, video_ids: &[String]) -> Result<()> {
        let actions: Vec<Value> = video_ids
            .iter()
            .map(|id| {
                json!({
                    "action": "ACTION_ADD_VIDEO",
                    "addedVideoId": id,
                    "dedupeOption": "DEDUPE_OPTION_SKIP",
                })
            })
            .collect();
        let response = self
            .post(
                "browse/edit_playlist",
                json!({ "playlistId": playlist_id.trim_start_matches("VL"), "actions": actions }),
            )
            .await?;
        match response["status"].as_str() {
            Some("STATUS_SUCCEEDED") | None => Ok(()),
            Some(status) => bail!("{status}"),
        }
    }

    pub async fn remove_from_playlist(
        &self,
        playlist_id: &str,
        video_id: &str,
        set_video_id: &str,
    ) -> Result<()> {
        self.post(
            "browse/edit_playlist",
            json!({
                "playlistId": playlist_id.trim_start_matches("VL"),
                "actions": [{
                    "action": "ACTION_REMOVE_VIDEO",
                    "removedVideoId": video_id,
                    "setVideoId": set_video_id,
                }],
            }),
        )
        .await?;
        Ok(())
    }

    /// Renames one of the account's own playlists.
    pub async fn rename_playlist(&self, playlist_id: &str, title: &str) -> Result<()> {
        let response = self
            .post(
                "browse/edit_playlist",
                json!({
                    "playlistId": playlist_id.trim_start_matches("VL"),
                    "actions": [{ "action": "ACTION_SET_PLAYLIST_NAME", "playlistName": title }],
                }),
            )
            .await?;
        match response["status"].as_str() {
            Some("STATUS_SUCCEEDED") | None => Ok(()),
            Some(status) => bail!("{status}"),
        }
    }

    /// Deletes one of the account's own playlists, for good.
    pub async fn delete_playlist(&self, playlist_id: &str) -> Result<()> {
        self.post("playlist/delete", json!({ "playlistId": playlist_id.trim_start_matches("VL") }))
            .await?;
        Ok(())
    }

    pub async fn create_playlist(&self, title: &str, private: bool) -> Result<String> {
        let response = self
            .post(
                "playlist/create",
                json!({
                    "title": title,
                    "description": "",
                    "privacyStatus": if private { "PRIVATE" } else { "UNLISTED" },
                }),
            )
            .await?;
        response["playlistId"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("no playlist id"))
    }

    pub async fn account(&self) -> Result<Account> {
        let response = self.post("account/account_menu", json!({})).await?;
        parse::account(&response).ok_or_else(|| anyhow::anyhow!("not signed in"))
    }

    /// The account's playlists, for the sidebar and "Save to playlist".
    pub async fn library_playlists(&self) -> Result<Vec<Item>> {
        let page = self.browse("FEmusic_liked_playlists", None).await?;
        let mut out: Vec<Item> = page
            .sections
            .into_iter()
            .flat_map(|s| s.items)
            .filter(|i| matches!(i.target, Some(Target::Browse { .. })))
            .collect();
        // The first card is "New playlist".
        out.retain(|i| i.kind == Kind::Playlist || i.title != "New playlist");
        Ok(out)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rating {
    Like,
    Dislike,
    None,
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
