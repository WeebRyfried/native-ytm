//! Artwork: fetched and decoded off the interface thread, kept on disk
//! between runs, and held as textures only while something draws them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use egui::{ColorImage, TextureHandle, TextureOptions};

use crate::api::model::sized_thumbnail;

/// Textures left undrawn this long may go once the textures held pass
/// BUDGET (they come back from the disk cache when drawn again)...
const IDLE: Duration = Duration::from_secs(30);
const BUDGET: usize = 48 * 1024 * 1024;
/// ...and any left undrawn this long go regardless.
const STALE: Duration = Duration::from_secs(300);

enum Entry {
    Loading,
    Ready {
        texture: TextureHandle,
        file: PathBuf,
        used: Instant,
        /// Its size in memory (and on the GPU).
        bytes: usize,
    },
    Failed(Instant),
    /// On disk only, for the media overlay, which reads the file itself.
    File(PathBuf),
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
        let key = sized_key(url, px);
        let source = if url.starts_with("file:") { url.to_owned() } else { key.clone() };
        self.entry(key, source, px, Make::Texture)
    }

    /// The artwork at `px`, or while that loads, a smaller size already
    /// here, so a big view never flashes empty.
    pub fn get_or_smaller(&mut self, url: &str, px: u32) -> Option<TextureHandle> {
        if let Some(texture) = self.get(url, px) {
            return Some(texture);
        }
        [512, 256, 128, 64, 32].into_iter().find_map(|size| match self.entries.get_mut(&sized_key(url, size)) {
            Some(Entry::Ready { texture, used, .. }) => {
                *used = Instant::now();
                Some(texture.clone())
            }
            _ => None,
        })
    }

    /// The artwork blurred to a wash of its colours, for a backdrop.
    pub fn backdrop(&mut self, url: &str) -> Option<TextureHandle> {
        let source = sized_thumbnail(url, 128);
        self.entry(format!("backdrop:{source}"), source, 128, Make::Blurred)
    }

    fn entry(&mut self, key: String, source: String, px: u32, make: Make) -> Option<TextureHandle> {
        match self.entries.get_mut(&key) {
            Some(Entry::Ready { texture, used, .. }) => {
                *used = Instant::now();
                Some(texture.clone())
            }
            Some(Entry::Loading | Entry::File(_)) => None,
            Some(Entry::Failed(at)) => {
                if at.elapsed() > Duration::from_secs(30) {
                    self.entries.remove(&key);
                }
                None
            }
            None => {
                self.entries.insert(key.clone(), Entry::Loading);
                self.fetch(key, source, px, make);
                None
            }
        }
    }

    /// The artwork's file on disk, for the Windows media overlay.
    /// No texture is made for it: the overlay reads the file.
    pub fn file(&mut self, url: &str, px: u32) -> Option<PathBuf> {
        // A picture on this PC is its own file.
        if let Some(path) = url.strip_prefix("file:") {
            return Some(PathBuf::from(path));
        }
        let px = px.clamp(32, 1200).next_power_of_two().min(1200);
        let source = sized_thumbnail(url, px);
        if let Some(Entry::Ready { file, .. }) = self.entries.get(&source) {
            return Some(file.clone());
        }
        let key = format!("file:{source}");
        match self.entries.get(&key) {
            Some(Entry::File(path)) => Some(path.clone()),
            Some(Entry::Loading) => None,
            Some(Entry::Failed(at)) => {
                if at.elapsed() > Duration::from_secs(30) {
                    self.entries.remove(&key);
                }
                None
            }
            _ => {
                let path = self.path_of(&source);
                if path.is_file() {
                    self.entries.insert(key, Entry::File(path.clone()));
                    return Some(path);
                }
                self.entries.insert(key.clone(), Entry::Loading);
                self.fetch(key, source, px, Make::FileOnly);
                None
            }
        }
    }

    fn path_of(&self, source: &str) -> PathBuf {
        // The file is the source image's, whatever is made of it.
        self.dir.join(format!("{:016x}.img", fnv(source)))
    }

    fn fetch(&self, key: String, source: String, px: u32, make: Make) {
        let tx = self.tx.clone();
        let http = self.http.clone();
        let path = self.path_of(&source);
        let permits = self.permits.clone();
        let ctx = self.ctx.clone();
        self.runtime.spawn(async move {
            let local = source.strip_prefix("file:").map(PathBuf::from);
            let bytes = match local {
                // A picture on this PC (a song's own artwork) is read where
                // it is, never copied into the cache.
                Some(local) => tokio::fs::read(&local).await.ok(),
                None => match tokio::fs::read(&path).await {
                    Ok(bytes) => Some(bytes),
                    Err(_) => {
                        let _permit = permits.acquire().await.ok();
                        download(&http, &source).await.inspect(|bytes| {
                            crate::settings::write_atomic(&path, bytes);
                        })
                    }
                },
            };
            let texture = match bytes {
                Some(_) if make == Make::FileOnly => {
                    let _ = tx.send(Loaded { key, texture: None, file: Some(path) });
                    return;
                }
                Some(bytes) => {
                    let name = key.clone();
                    let ctx = ctx.clone();
                    tokio::task::spawn_blocking(move || {
                        if make == Make::Blurred { decode_blurred(&ctx, &name, &bytes) } else { decode(&ctx, &name, &bytes, px) }
                    })
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
                (None, Some(file)) => Entry::File(file),
                (Some(texture), Some(file)) => Entry::Ready {
                    bytes: texture.size()[0] * texture.size()[1] * 4,
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

    /// Drops textures nothing has drawn lately: the stale ones always, and
    /// the idle ones, oldest first, while the rest are over budget.
    fn evict(&mut self) {
        self.entries.retain(|_, entry| match entry {
            Entry::Ready { used, .. } => used.elapsed() < STALE,
            _ => true,
        });
        let mut total: usize = self
            .entries
            .values()
            .map(|e| match e {
                Entry::Ready { bytes, .. } => *bytes,
                _ => 0,
            })
            .sum();
        if total <= BUDGET {
            return;
        }
        let mut idle: Vec<(Instant, String, usize)> = self
            .entries
            .iter()
            .filter_map(|(key, e)| match e {
                Entry::Ready { used, bytes, .. } if used.elapsed() >= IDLE => Some((*used, key.clone(), *bytes)),
                _ => None,
            })
            .collect();
        idle.sort_by_key(|(used, _, _)| *used);
        for (_, key, bytes) in idle {
            if total <= BUDGET {
                break;
            }
            self.entries.remove(&key);
            total -= bytes;
        }
    }
}

/// What a fetch makes of the image.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Make {
    Texture,
    Blurred,
    FileOnly,
}

/// The key of artwork at a size. A picture on this PC has one file for
/// every size, so the size goes in its key.
fn sized_key(url: &str, px: u32) -> String {
    if url.starts_with("file:") { format!("{url}#{px}") } else { sized_thumbnail(url, px) }
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

/// The image shrunk to 32 by 32 and blurred: drawn stretched across a
/// window, the GPU's smoothing turns it into soft fields of its colours.
fn decode_blurred(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<TextureHandle> {
    const N: usize = 32;
    let image = image::load_from_memory(bytes).ok()?;
    let small = image
        .resize_exact(N as u32, N as u32, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mut pixels: Vec<[f32; 3]> = small
        .pixels()
        .map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
        .collect();
    // Three box blurs come close to a Gaussian.
    for _ in 0..3 {
        pixels = box_blur(&pixels, N, 4);
    }
    let mut rgba = Vec::with_capacity(N * N * 4);
    for [r, g, b] in pixels {
        // A little more colour, as blurring greys it.
        let mean = (r + g + b) / 3.0;
        let boost = |c: f32| (mean + (c - mean) * 1.35).clamp(0.0, 255.0) as u8;
        rgba.extend_from_slice(&[boost(r), boost(g), boost(b), 255]);
    }
    let color = ColorImage::from_rgba_unmultiplied([N, N], &rgba);
    Some(ctx.load_texture(name, color, TextureOptions::LINEAR))
}

fn box_blur(pixels: &[[f32; 3]], n: usize, radius: usize) -> Vec<[f32; 3]> {
    let pass = |input: &[[f32; 3]], horizontal: bool| -> Vec<[f32; 3]> {
        let mut out = vec![[0.0; 3]; n * n];
        for y in 0..n {
            for x in 0..n {
                let mut sum = [0.0f32; 3];
                let mut count = 0.0;
                for d in 0..=radius * 2 {
                    let offset = d as isize - radius as isize;
                    let (sx, sy) = if horizontal { (x as isize + offset, y as isize) } else { (x as isize, y as isize + offset) };
                    // Edges repeat, so the colours do not darken there.
                    let sx = sx.clamp(0, n as isize - 1) as usize;
                    let sy = sy.clamp(0, n as isize - 1) as usize;
                    let p = input[sy * n + sx];
                    for c in 0..3 {
                        sum[c] += p[c];
                    }
                    count += 1.0;
                }
                out[y * n + x] = [sum[0] / count, sum[1] / count, sum[2] / count];
            }
        }
        out
    };
    pass(&pass(pixels, true), false)
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
