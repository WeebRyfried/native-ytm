//! A console tool for when YouTube changes something: prints what the
//! parsers make of live pages, and resolves and plays a stream.
//!
//! ```text
//! ytm-probe dump home
//! ytm-probe dump browse MPREb_...      (an album, artist or playlist id)
//! ytm-probe dump search "daft punk" songs
//! ytm-probe dump suggest daft
//! ytm-probe dump watch dQw4w9WgXcQ
//! ytm-probe play dQw4w9WgXcQ [--seek 120]
//! ytm-probe icon assets/app.ico         (the executable's icon, drawn)
//! ytm-probe lyrics TITLE ARTIST [SECONDS]  (synced lyrics from LRCLIB)
//! ```

use std::sync::Arc;
use std::time::Duration;

use image::ImageEncoder;
use native_ytm::{api, player, stream};

fn main() -> anyhow::Result<()> {
    env_logger_lite();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("dump") if args.len() > 2 => return dump(&args[2..]),
        Some("icon") => return write_icon(args.get(2).map_or("assets/app.ico", String::as_str)),
        Some("lyrics") if args.len() > 3 => return lyrics(&args[2..]),
        Some("play") => {}
        _ => {
            eprintln!("usage: ytm-probe dump home|browse ID|search QUERY [songs|albums|artists]|suggest TEXT|watch ID
       ytm-probe play VIDEO_ID [--seek SECONDS]");
            return Ok(());
        }
    }
    let id = args.get(2).cloned().unwrap_or_else(|| "fJ9rUzIMcZQ".into());
    let seek = args.iter().position(|a| a == "--seek").and_then(|i| args.get(i + 1)).and_then(|s| s.parse::<u64>().ok());
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build()?;
    let http = reqwest::Client::builder().gzip(true).build()?;
    let api = Arc::new(api::client::Innertube::new(http.clone(), "en"));
    // `--cookies FILE` signs in with a cookies.txt (or a pasted header), to
    // check the account's streams; `--premium` asks for 256 kb/s.
    if let Some(path) = args.iter().position(|a| a == "--cookies").and_then(|i| args.get(i + 1)) {
        let text = std::fs::read_to_string(path)?;
        let auth = api::client::Auth { cookies: api::client::parse_cookies(&text), auth_user: 0 };
        println!("signed in: {}", auth.is_usable());
        api.set_auth(Some(auth));
    }
    let scratch = std::env::temp_dir().join("ytm-probe");
    let mut resolver = stream::Resolver::new(None, true, args.iter().any(|a| a == "--premium"), scratch);
    // `--ytdlp` skips the native clients, to check the fallback.
    if args.iter().any(|a| a == "--ytdlp") {
        resolver.clients.clear();
    }
    let started = std::time::Instant::now();
    let (info, via) = runtime.block_on(async { api.bootstrap().await; resolver.resolve(&api, &id).await })?;
    println!("resolved via {via:?} in {:?}: {} ({}), {} bytes, {:?}, loudness {:?}", started.elapsed(), info.quality, info.mime, info.content_length, info.duration, info.loudness_db);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut player = player::Player::spawn(runtime.handle().clone(), http, tx, Arc::new(|| {}), true); player.set_volume(if std::env::var_os("NATIVE_YTM_MUTE").is_some() { 0.0 } else { 0.35 });
    let t0 = std::time::Instant::now();
    player.load(1, info, Duration::ZERO, true);
    for i in 0..60 {
        std::thread::sleep(Duration::from_millis(250));
        while let Ok(ev) = rx.try_recv() { println!("{:?} event {ev:?}", t0.elapsed()); }
        let s = player.status();
        if i % 4 == 0 { println!("{:?} {:?} pos {:?} dl {:.2}", t0.elapsed(), s.state, player.position(), s.downloaded); }
        if i == 24 { if let Some(to) = seek { println!("seek to {to}"); player.seek(Duration::from_secs(to)); } }
    }
    Ok(())
}

fn env_logger_lite() {
    struct L;
    impl log::Log for L {
        fn enabled(&self, m: &log::Metadata) -> bool { m.level() <= log::Level::Info }
        fn log(&self, r: &log::Record) { if self.enabled(r.metadata()) && !r.target().starts_with("symphonia") { eprintln!("[{}] {}: {}", r.level(), r.target(), r.args()); } }
        fn flush(&self) {}
    }
    let _ = log::set_boxed_logger(Box::new(L)).map(|()| log::set_max_level(log::LevelFilter::Info));
}

fn dump(args: &[String]) -> anyhow::Result<()> {
    use api::model::*;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let http = reqwest::Client::builder().gzip(true).build()?;
    let api = api::client::Innertube::new(http, "en");
    runtime.block_on(async {
        api.bootstrap().await;
        let show_items = |items: &[Item]| for i in items.iter().take(6) {
            println!("    - [{:?}] {} | {} | target={:?} | track={:?} | thumb={}", i.kind, i.title, i.subtitle, i.target.as_ref().map(|t| format!("{t:?}").chars().take(70).collect::<String>()), i.track.as_ref().map(|t| (t.video_id.clone(), t.artist_line(), t.album.as_ref().map(|a| a.name.clone()), t.duration)), i.thumbnail.is_some());
        };
        let show_page = |p: &Page| {
            println!("header: {:?}", p.header.as_ref().map(|h| (h.title.clone(), h.subtitle.clone(), h.second_subtitle.clone(), h.author.clone(), h.playlist_id.clone(), h.channel_id.clone(), h.play.is_some(), h.radio.is_some(), h.thumbnail.is_some())));
            println!("tracks: {} cont={}", p.tracks.len(), p.tracks_continuation.is_some());
            for t in p.tracks.iter().take(4) { println!("    * {} | {} | {:?} | {:?} | {}", t.title, t.artist_line(), t.album.as_ref().map(|a| &a.name), t.duration, t.video_id); }
            println!("chips: {:?}", p.chips.iter().map(|c| c.title.clone()).collect::<Vec<_>>());
            for s in &p.sections { println!("  section {:?} '{}' ({} items) more={} cont={}", s.layout, s.title, s.items.len(), s.more.is_some(), s.continuation.is_some()); show_items(&s.items); if let Some(t) = &s.text { println!("    text: {}", t.chars().take(80).collect::<String>()); } }
            println!("page continuation: {}", p.continuation.is_some());
        };
        match args[0].as_str() {
            "home" => { let p = api.home(None).await?; show_page(&p); if let Some(c) = &p.continuation { let (s, _, more) = api.more(c).await?; println!("-- continuation: {} sections, more={}", s.len(), more.is_some()); for s in &s { println!("  section {:?} '{}' ({} items)", s.layout, s.title, s.items.len()); } } }
            "browse" => { let p = api.browse(&args[1], args.get(2).map(String::as_str)).await?; show_page(&p); }
            "search" => { let f = match args.get(2).map(String::as_str) { Some("songs") => api::music::Filter::Songs, Some("albums") => api::music::Filter::Albums, Some("artists") => api::music::Filter::Artists, _ => api::music::Filter::All }; let (s, c) = api.search(&args[1], f).await?; for s in &s { println!("  section {:?} '{}' ({} items)", s.layout, s.title, s.items.len()); show_items(&s.items); } println!("cont {}", c.is_some()); if let Some(c) = c { let (s, c2) = api.search_more(&c).await?; println!("-- more: {} sections {} items, cont {}", s.len(), s.iter().map(|s| s.items.len()).sum::<usize>(), c2.is_some()); } }
            "suggest" => { let s = api.suggestions(&args[1]).await?; println!("{:?}", s.queries); show_items(&s.items); }
            "watch" => { let q = api.watch(Some(&args[1]), args.get(2).map(String::as_str), None).await?; println!("lyrics {:?} related {:?} playlist {:?} cont {}", q.lyrics_id, q.related_id, q.playlist_id, q.continuation.is_some()); for t in q.tracks.iter().take(5) { println!("  * {} | {} | {:?} | {:?} | thumb={}", t.title, t.artist_line(), t.album.as_ref().map(|a| &a.name), t.duration, t.thumbnail.is_some()); } println!("{} tracks", q.tracks.len()); if let Some(id) = &q.lyrics_id { let l = api.lyrics(id).await?; println!("lyrics: {:?}", l.map(|l| (l.text.chars().take(60).collect::<String>(), l.source))); } if let Some(c) = &q.continuation { let m = api.watch_more(c, q.playlist_id.as_deref()).await?; println!("more: {} tracks", m.tracks.len()); } }
            _ => println!("?"),
        }
        anyhow::Ok(())
    })
}

/// Writes the app's drawn icon as a Windows .ico, PNG images inside.
fn write_icon(path: &str) -> anyhow::Result<()> {
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let mut images = Vec::new();
    for &size in &sizes {
        let rgba = native_ytm::theme::app_icon_rgba(size as usize);
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(&rgba, size, size, image::ExtendedColorType::Rgba8)?;
        images.push((size, png));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in &images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &images {
        out.extend_from_slice(png);
    }
    std::fs::write(path, out)?;
    println!("wrote {path}");
    Ok(())
}

fn lyrics(args: &[String]) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let http = reqwest::Client::builder().gzip(true).build()?;
    let query = api::lrclib::Query {
        title: args[0].clone(),
        artist: args[1].clone(),
        album: None,
        duration: args.get(2).and_then(|s| s.parse().ok()),
    };
    match runtime.block_on(api::lrclib::find(&http, &query))? {
        Some(lyrics) => {
            println!("{:?}: {} timed lines", lyrics.source, lyrics.lines.len());
            for line in lyrics.lines.iter().take(6) {
                println!("  {:>7.2}  {}", line.time, line.text);
            }
        }
        None => println!("none"),
    }
    Ok(())
}
