//! A seekable byte source over an HTTP stream.
//!
//! A task on the backend runtime downloads the file in ranges into memory,
//! starting wherever the reader is, and reads block until their bytes arrive.
//! YouTube's audio files are a few megabytes, so the whole file is kept: a
//! seek backwards never touches the network, and a seek forwards jumps the
//! download only when the bytes are not about to arrive anyway.

use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use reqwest::header::{HeaderMap, RANGE};

/// Bytes asked for per request. googlevideo throttles long open-ended
/// responses, so the file is fetched in ranges.
const CHUNK: u64 = 1024 * 1024;

/// A reader waiting this close ahead of the running download keeps it
/// instead of starting another request.
const NEAR: u64 = 512 * 1024;

/// What made a read fail.
#[derive(Clone, Debug)]
pub enum SourceError {
    /// The server refused the URL: it expired or needs to be resolved again.
    Forbidden,
    /// Anything else, after retries.
    Other(String),
}

struct State {
    data: Vec<u8>,
    /// Downloaded spans, sorted and merged.
    have: Vec<Range<u64>>,
    /// Where a blocked reader is waiting.
    want: Option<u64>,
    /// Where the running request will write next.
    cursor: Option<u64>,
    error: Option<SourceError>,
}

pub struct Shared {
    state: Mutex<State>,
    ready: Condvar,
    jumped: tokio::sync::Notify,
    len: u64,
    /// Set when the source is dropped or replaced: the download stops and a
    /// waiting read returns.
    cancelled: AtomicBool,
    /// Set by the player to wake a read blocked on the network, so a seek or
    /// a new track is not held behind a slow connection.
    interrupt: AtomicBool,
}

impl Shared {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.ready.notify_all();
        self.jumped.notify_one();
    }

    pub fn interrupt(&self) {
        self.interrupt.store(true, Ordering::SeqCst);
        self.ready.notify_all();
    }

    pub fn clear_interrupt(&self) {
        self.interrupt.store(false, Ordering::SeqCst);
    }

    pub fn error(&self) -> Option<SourceError> {
        self.state.lock().error.clone()
    }

    /// Fraction of the file downloaded, for the seek bar's buffer shade.
    pub fn downloaded(&self) -> Vec<Range<u64>> {
        self.state.lock().have.clone()
    }

    pub fn len(&self) -> u64 {
        self.len
    }
}

/// Called with the whole file once its download completes.
pub type Complete = Box<dyn FnOnce(Vec<u8>) + Send>;

/// The reader half handed to the demuxer.
pub struct RemoteSource {
    shared: Arc<Shared>,
    pos: u64,
}

impl RemoteSource {
    /// Starts downloading `url` (of `len` bytes) on `runtime` and returns the
    /// reader and a handle to cancel or interrupt it.
    pub fn open(
        runtime: &tokio::runtime::Handle,
        client: reqwest::Client,
        url: String,
        headers: HeaderMap,
        len: u64,
        on_complete: Option<Complete>,
    ) -> (Self, Arc<Shared>) {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                data: vec![0; len as usize],
                have: Vec::new(),
                want: Some(0),
                cursor: None,
                error: None,
            }),
            ready: Condvar::new(),
            jumped: tokio::sync::Notify::new(),
            len,
            cancelled: AtomicBool::new(false),
            interrupt: AtomicBool::new(false),
        });
        runtime.spawn(download(shared.clone(), client, url, headers, on_complete));
        (
            Self {
                shared: shared.clone(),
                pos: 0,
            },
            shared,
        )
    }

    /// A file already whole in memory (a song from the cache): nothing to
    /// download. Empty bytes make a source that fails as if refused, so the
    /// song is looked up again.
    pub fn from_bytes(bytes: Vec<u8>) -> (Self, Arc<Shared>) {
        let len = bytes.len() as u64;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                data: bytes,
                have: if len > 0 { vec![0..len] } else { Vec::new() },
                want: None,
                cursor: None,
                error: (len == 0).then_some(SourceError::Forbidden),
            }),
            ready: Condvar::new(),
            jumped: tokio::sync::Notify::new(),
            len,
            cancelled: AtomicBool::new(false),
            interrupt: AtomicBool::new(false),
        });
        (
            Self {
                shared: shared.clone(),
                pos: 0,
            },
            shared,
        )
    }
}

impl Drop for RemoteSource {
    fn drop(&mut self) {
        self.shared.cancel();
    }
}

impl Read for RemoteSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.shared.len;
        if self.pos >= len || buf.is_empty() {
            return Ok(0);
        }
        let mut state = self.shared.state.lock();
        loop {
            if self.shared.cancelled.load(Ordering::SeqCst) {
                return Err(io::Error::other("cancelled"));
            }
            if self.shared.interrupt.load(Ordering::SeqCst) {
                return Err(io::Error::other("interrupted"));
            }
            if let Some(span) = state.have.iter().find(|span| span.contains(&self.pos)) {
                let end = span.end.min(self.pos + buf.len() as u64);
                let n = (end - self.pos) as usize;
                let start = self.pos as usize;
                buf[..n].copy_from_slice(&state.data[start..start + n]);
                self.pos += n as u64;
                return Ok(n);
            }
            if let Some(error) = &state.error {
                return Err(io::Error::other(match error {
                    SourceError::Forbidden => "forbidden".to_owned(),
                    SourceError::Other(message) => message.clone(),
                }));
            }
            if state.want != Some(self.pos) {
                state.want = Some(self.pos);
                let near = state
                    .cursor
                    .is_some_and(|cursor| cursor <= self.pos && self.pos - cursor < NEAR);
                if !near {
                    self.shared.jumped.notify_one();
                }
            }
            self.shared
                .ready
                .wait_for(&mut state, Duration::from_millis(100));
        }
    }
}

impl Seek for RemoteSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let len = self.shared.len as i64;
        let target = match from {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => len + n,
            SeekFrom::Current(n) => self.pos as i64 + n,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = target as u64;
        Ok(self.pos)
    }
}

impl symphonia::core::io::MediaSource for RemoteSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.shared.len)
    }
}

/// The first byte not yet downloaded at or after `from`, wrapping round to
/// the start so holes left by a seek are filled last.
fn next_missing(have: &[Range<u64>], from: u64, len: u64) -> Option<u64> {
    let after = |from: u64| -> Option<u64> {
        let mut pos = from;
        for span in have {
            if span.end <= pos {
                continue;
            }
            if span.start > pos {
                return Some(pos);
            }
            pos = span.end;
        }
        (pos < len).then_some(pos)
    };
    after(from).or_else(|| after(0))
}

/// The end of the gap starting at `start`: the next downloaded span, or the
/// end of the file.
fn gap_end(have: &[Range<u64>], start: u64, len: u64) -> u64 {
    have.iter()
        .map(|span| span.start)
        .filter(|&s| s > start)
        .min()
        .unwrap_or(len)
}

fn insert_span(have: &mut Vec<Range<u64>>, span: Range<u64>) {
    have.push(span);
    have.sort_by_key(|span| span.start);
    let mut merged: Vec<Range<u64>> = Vec::with_capacity(have.len());
    for span in have.drain(..) {
        match merged.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => merged.push(span),
        }
    }
    *have = merged;
}

async fn download(
    shared: Arc<Shared>,
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    mut on_complete: Option<Complete>,
) {
    let len = shared.len;
    let mut failures = 0u32;
    loop {
        if shared.cancelled.load(Ordering::SeqCst) {
            return;
        }
        let start = {
            let mut state = shared.state.lock();
            let from = state.want.unwrap_or(0);
            match next_missing(&state.have, from, len) {
                Some(start) => {
                    state.cursor = Some(start);
                    start
                }
                None => {
                    state.cursor = None;
                    // All here: hand a copy to whoever keeps it.
                    let whole = on_complete.is_some().then(|| state.data.clone());
                    drop(state);
                    if let (Some(done), Some(bytes)) = (on_complete.take(), whole) {
                        done(bytes);
                    }
                    return;
                }
            }
        };
        let end = {
            let state = shared.state.lock();
            gap_end(&state.have, start, len).min(start + CHUNK)
        };
        match fetch(&shared, &client, &url, &headers, start, end).await {
            Ok(()) => failures = 0,
            Err(Fetch::Jumped) => {}
            Err(Fetch::Cancelled) => return,
            Err(Fetch::Failed(error)) => {
                failures += 1;
                let fatal = matches!(error, SourceError::Forbidden) || failures >= 4;
                log::warn!("stream range {start}-{end} failed ({failures}): {error:?}");
                if fatal {
                    shared.state.lock().error = Some(error);
                    shared.ready.notify_all();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(300 * u64::from(failures))).await;
            }
        }
    }
}

enum Fetch {
    Jumped,
    Cancelled,
    Failed(SourceError),
}

async fn fetch(
    shared: &Shared,
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    start: u64,
    end: u64,
) -> Result<(), Fetch> {
    let request = client
        .get(url)
        .headers(headers.clone())
        .header(RANGE, format!("bytes={start}-{}", end - 1))
        .send();
    let mut response = tokio::select! {
        response = request => response.map_err(|e| Fetch::Failed(SourceError::Other(e.without_url().to_string())))?,
        () = shared.jumped.notified() => return Err(Fetch::Jumped),
    };
    let status = response.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::GONE {
        return Err(Fetch::Failed(SourceError::Forbidden));
    }
    if !status.is_success() {
        return Err(Fetch::Failed(SourceError::Other(format!("HTTP {status}"))));
    }
    // A 200 answer ignored the range and starts at byte 0.
    let mut offset = if status == reqwest::StatusCode::PARTIAL_CONTENT {
        start
    } else {
        0
    };
    loop {
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(Fetch::Cancelled);
        }
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|e| Fetch::Failed(SourceError::Other(e.without_url().to_string())))?,
            () = shared.jumped.notified() => {
                // The reader moved away from this request; plan again.
                let state = shared.state.lock();
                let stay = state.want.is_some_and(|want| want >= start && want < end);
                drop(state);
                if stay { continue } else { return Err(Fetch::Jumped) }
            }
        };
        let Some(chunk) = chunk else {
            return Ok(());
        };
        let mut state = shared.state.lock();
        let room = (shared.len.saturating_sub(offset)) as usize;
        let n = chunk.len().min(room);
        let at = offset as usize;
        state.data[at..at + n].copy_from_slice(&chunk[..n]);
        insert_span(&mut state.have, offset..offset + n as u64);
        offset += n as u64;
        state.cursor = Some(offset);
        drop(state);
        shared.ready.notify_all();
        if offset >= end || n < chunk.len() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_bytes_wrap_round() {
        let have = vec![0..10, 20..30];
        assert_eq!(next_missing(&have, 0, 40), Some(10));
        assert_eq!(next_missing(&have, 25, 40), Some(30));
        assert_eq!(next_missing(&[0..10, 30..40], 35, 40), Some(10));
        assert_eq!(next_missing(&[0..40], 5, 40), None);
    }

    #[test]
    fn spans_merge() {
        let mut have = vec![0..10];
        insert_span(&mut have, 20..30);
        insert_span(&mut have, 10..20);
        assert_eq!(have, vec![0..30]);
        assert_eq!(gap_end(&[0..10, 50..60], 10, 100), 50);
    }
}
