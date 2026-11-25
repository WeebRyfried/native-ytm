//! Lyrics timed by listening to the song, on a click (src/lyrics_sync.rs):
//! the song showing, one at a time, kept with its other lyrics on disk.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use super::App;
use crate::api::model::Lyrics;
use crate::lyrics_sync::{Progress, Step};
use crate::model::{Action, Loadable, ToastKind};

/// How long the finished steps stay on screen once a sync is done.
pub const DONE_SHOWN_SECS: f32 = 4.0;

/// Where a song's timing by listening is.
#[derive(Clone)]
pub enum SyncState {
    /// Under way: the steps as the task reports them.
    Running(Progress),
    /// Done, at this moment: the steps shown finished for a few seconds.
    Done(Instant, Step),
    /// Stopped at the step it was on, and why.
    Failed(Step, String),
}

pub struct LyricSync {
    pub songs: HashMap<String, SyncState>,
    done: (Sender<(String, Result<Lyrics, String>)>, Receiver<(String, Result<Lyrics, String>)>),
}

impl Default for LyricSync {
    fn default() -> Self {
        Self { songs: HashMap::new(), done: channel() }
    }
}

impl App {
    /// Whether a song is being timed now, any song.
    pub fn syncing_lyrics(&self) -> bool {
        self.lyric_sync.songs.values().any(|s| matches!(s, SyncState::Running(_)))
    }

    /// Times the song showing's lyrics by listening to it, or (with none)
    /// writes them down from what is heard.
    pub(super) fn sync_lyrics(&mut self) {
        let Some(track) = self.current().cloned() else {
            return;
        };
        if self.syncing_lyrics() {
            self.toast("Already syncing a song's lyrics", ToastKind::Info);
            return;
        }
        let video_id = track.video_id.clone();
        // The song's audio: its file when there is one, else the stream
        // playing.
        let stream = if track.is_local() {
            self.local.stream_for(&track)
        } else {
            self.audio_cache
                .lookup(&video_id)
                .or_else(|| self.loaded.as_ref().filter(|(t, _)| *t == self.current_token).map(|(_, s)| s.clone()))
        };
        let Some(stream) = stream else {
            let step = Step { step: 0, log: vec!["The song isn't loaded yet".into()] };
            self.lyric_sync.songs.insert(video_id, SyncState::Failed(step, "Play the song first, then sync it".into()));
            return;
        };
        // The plain lyrics to time, when there are some; else lyrics timed
        // by the line, to time by the word.
        let plain = match self.lyrics.get(&video_id) {
            Some(Loadable::Ready(Some(lyrics))) => {
                let all = || std::iter::once(&**lyrics).chain(lyrics.others.iter());
                all()
                    .find(|l| l.lines.is_empty() && !l.text.trim().is_empty())
                    .or_else(|| all().find(|l| !l.text.trim().is_empty() && l.provider.as_deref() != Some(crate::lyrics_sync::PROVIDER)))
                    .cloned()
            }
            _ => None,
        };
        log::info!(
            "syncing the lyrics of {video_id} by listening ({}, {} kB)",
            if plain.is_some() { "timing plain lyrics" } else { "writing them down" },
            stream.content_length / 1024
        );
        let progress = Progress::default();
        self.lyric_sync.songs.insert(video_id.clone(), SyncState::Running(progress.clone()));
        let http = self.backend.http.clone();
        let key = crate::lyrics_sync::key(&self.settings.fish_key);
        let tx = self.lyric_sync.done.0.clone();
        let waker = self.waker.clone();
        self.backend.runtime.spawn(async move {
            let result = async {
                let audio = crate::lyrics_sync::audio(&http, &stream, &progress).await?;
                let extension = crate::cache::file_of(&stream.url).and_then(|p| p.extension()).and_then(|e| e.to_str()).unwrap_or_default().to_owned();
                let heard = crate::lyrics_sync::listen(&http, &key, audio, &stream.mime, &extension, &progress).await?;
                log::info!("Fish Audio heard {} words", heard.len());
                match &plain {
                    Some(plain) => progress.at(3, format!("Matching {} words to {} lines of lyrics", heard.len(), plain.text.lines().filter(|l| !l.trim().is_empty()).count())),
                    None => progress.at(3, format!("Writing down {} words in lines", heard.len())),
                }
                let lyrics = crate::lyrics_sync::lyrics_from(plain.as_ref(), &heard)?;
                progress.update(format!("Timed {} lines, word by word", lyrics.lines.len()));
                Ok::<_, anyhow::Error>(lyrics)
            }
            .await
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send((video_id, result));
            waker.wake();
        });
    }

    pub(super) fn lyric_sync_tick(&mut self) {
        while let Ok((video_id, result)) = self.lyric_sync.done.1.try_recv() {
            let step = match self.lyric_sync.songs.get(&video_id) {
                Some(SyncState::Running(progress)) => progress.get(),
                _ => Step::default(),
            };
            match result {
                Ok(lyrics) => {
                    self.lyric_sync.songs.insert(video_id.clone(), SyncState::Done(Instant::now(), step));
                    let choice = lyrics.choice();
                    let words = lyrics.by_word();
                    // Kept with the song's other lyrics, never in their place.
                    let kept = self
                        .lyrics_cache
                        .put(&video_id, self.settings.synced_lyrics, &Some(lyrics.clone()), false)
                        .unwrap_or(lyrics);
                    self.lyrics.insert(video_id.clone(), Loadable::Ready(Some(Arc::new(kept))));
                    self.actions.push(Action::ChooseLyrics { video_id, choice: Some(choice) });
                    self.toast(if words { "Lyrics synced word by word" } else { "Lyrics synced" }, ToastKind::Info);
                }
                Err(error) => {
                    log::warn!("syncing the lyrics of {video_id}: {error}");
                    self.lyric_sync.songs.insert(video_id, SyncState::Failed(step, capitalised(&error)));
                }
            }
        }
        // The finished steps go once they have been seen.
        self.lyric_sync
            .songs
            .retain(|_, s| !matches!(s, SyncState::Done(at, _) if at.elapsed().as_secs_f32() > DONE_SHOWN_SECS));
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
