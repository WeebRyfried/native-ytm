//! Artwork: fetched and decoded off the interface thread, kept on disk
//! between runs, and held as textures only while something draws them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use egui::{ColorImage, TextureHandle, TextureOptions};

use crate::api::model::sized_thumbnail;

/// Textures left undrawn this long are dropped, once there are many.
const IDLE: Duration = Duration::from_secs(90);
const KEEP: usize = 240;

enum Entry {
    Loading,
    Ready {
        texture: TextureHandle,
        file: PathBuf,
        used: Instant,
    },
    Failed(Instant),
}

struct Loaded {
    key: String,
    texture: Option<TextureHandle>,
    file: Option<PathBuf>,
}

pub struct Images {
    entries: HashMap<String, Entry>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
    runtime: tokio::runtime::Handle,
    http: reqwest::Client,
    dir: PathBuf,
    permits: Arc<tokio::sync::Semaphore>,
    ctx: egui::Context,
    last_evict: Instant,
}

impl Images {
    pub fn new(
        ctx: egui::Context,
        runtime: tokio::runtime::Handle,
        http: reqwest::Client,
        dir: PathBuf,
    ) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let (tx, rx) = channel();
        Self {
            entries: HashMap::new(),
            tx,
            rx,
            runtime,
            http,
            dir,
            permits: Arc::new(tokio::sync::Semaphore::new(6)),
            ctx,
            last_evict: Instant::now(),
        }
    }

    /// Artwork at about `px` pixels square, if it is ready; otherwise starts
    /// fetching it.
    pub fn get(&mut self, url: &str, px: u32) -> Option<TextureHandle> {
        let px = px.clamp(32, 1200).next_power_of_two().min(1200);
        let key = sized_thumbnail(url, px);
        match self.entries.get_mut(&key) {
            Some(Entry::Ready { texture, used, .. }) => {
                *used = Instant::now();
                Some(texture.clone())
            }
            Some(Entry::Loading) => None,
            Some(Entry::Failed(at)) => {
                if at.elapsed() > Duration::from_secs(30) {
                    self.entries.remove(&key);
                }
                None
            }
            None => {
                self.entries.insert(key.clone(), Entry::Loading);
                self.fetch(key, px);
                None
            }
        }
    }

    /// The artwork's file on disk, for the Windows media overlay.
    pub fn file(&mut self, url: &str, px: u32) -> Option<PathBuf> {
        let px = px.clamp(32, 1200).next_power_of_two().min(1200);
        let key = sized_thumbnail(url, px);
        match self.entries.get(&key) {
            Some(Entry::Ready { file, .. }) => Some(file.clone()),
            _ => {
                let _ = self.get(url, px);
                None
            }
        }
    }

    fn fetch(&self, key: String, px: u32) {
        let tx = self.tx.clone();
        let http = self.http.clone();
        let path = self.dir.join(format!("{:016x}.img", fnv(&key)));
        let permits = self.permits.clone();
        let ctx = self.ctx.clone();
        self.runtime.spawn(async move {
            let bytes = match tokio::fs::read(&path).await {
                Ok(bytes) => Some(bytes),
                Err(_) => {
                    let _permit = permits.acquire().await.ok();
                    download(&http, &key).await.inspect(|bytes| {
                        crate::settings::write_atomic(&path, bytes);
                    })
                }
            };
            let texture = match bytes {
                Some(bytes) => {
                    let name = key.clone();
                    let ctx = ctx.clone();
                    tokio::task::spawn_blocking(move || decode(&ctx, &name, &bytes, px))
                        .await
                        .ok()
                        .flatten()
                }
                None => None,
            };
            let file = texture.as_ref().map(|_| path);
            let _ = tx.send(Loaded { key, texture, file });
            ctx.request_repaint();
        });
    }

    /// Takes in what finished loading. Call once a frame.
    pub fn poll(&mut self) {
        while let Ok(loaded) = self.rx.try_recv() {
            let entry = match (loaded.texture, loaded.file) {
                (Some(texture), Some(file)) => Entry::Ready {
                    texture,
                    file,
                    used: Instant::now(),
                },
                _ => Entry::Failed(Instant::now()),
            };
            self.entries.insert(loaded.key, entry);
        }
        if self.last_evict.elapsed() > Duration::from_secs(10) {
            self.last_evict = Instant::now();
            self.evict();
        }
    }

    fn evict(&mut self) {
        let ready = self
            .entries
            .values()
            .filter(|e| matches!(e, Entry::Ready { .. }))
            .count();
        if ready <= KEEP {
            return;
        }
        self.entries.retain(|_, entry| match entry {
            Entry::Ready { used, .. } => used.elapsed() < IDLE,
            _ => true,
        });
    }
}

async fn download(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let response = http
        .get(url)
        .timeout(Duration::from_secs(20))
        // JPEG or PNG, which the media overlay can show too.
        .header("Accept", "image/jpeg,image/png,*/*;q=0.5")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.bytes().await.ok().map(|b| b.to_vec())
}

fn decode(ctx: &egui::Context, name: &str, bytes: &[u8], px: u32) -> Option<TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?;
    let (w, h) = (image.width(), image.height());
    // Down to the size drawn, keeping the shape; never up.
    let scale = (px as f32 / w.min(h).max(1) as f32).min(1.0);
    let image = if scale < 0.95 {
        image.resize(
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
            image::imageops::FilterType::CatmullRom,
        )
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(name, color, TextureOptions::LINEAR))
}

fn fnv(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Clears the disk cache, for Settings.
pub fn clear_cache(dir: &Path) -> u64 {
    let mut freed = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                freed += meta.len();
            }
            let _ = std::fs::remove_file(entry.path());
        }
    }
    freed
}

pub fn cache_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}
