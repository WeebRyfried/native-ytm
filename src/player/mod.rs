//! The playback engine: one thread that downloads, demuxes, decodes and
//! resamples, feeding the device through fastframe-audio.
//!
//! The app hands the player resolved streams under tokens of its own. The
//! player says which token is audible, where it is, and when it ended or
//! failed; the app decides what plays next. A stream given with
//! [`Player::preload`] starts the moment the current one ends, with no gap.

pub mod eq;
mod opus;
pub mod output;
mod resample;
pub mod smart;
pub mod source;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use fastframe_audio::{Buffer, BufferSize, Maintained, Output, OutputOptions};
use parking_lot::Mutex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

use output::{Queue, Renderer, Report, frames_to_time};
use resample::Converter;
use source::{RemoteSource, SourceError};

/// Decoded audio kept ahead of the device.
const AHEAD: Duration = Duration::from_secs(3);

/// Decoding starts again once less than this is queued, and fills the queue
/// back to AHEAD in one go: a burst every three quarters of a second rather
/// than a packet at a time.
const LOW_WATER: Duration = Duration::from_millis(2250);

/// How long the old and the new stream overlap when a better one replaces
/// a stream mid-song.
const BLEND: Duration = Duration::from_millis(60);

/// How long after a pause the device itself stops: time for it to play out
/// the fade and fill its own buffer (a tenth of a second) with silence.
const SETTLE: Duration = Duration::from_millis(300);

/// A stream the resolver found: where the bytes are and how to ask for them.
#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub content_length: u64,
    pub duration: Option<Duration>,
    /// YouTube's loudness for the track, in dB relative to its target.
    pub loudness_db: Option<f32>,
    /// The container's MIME type: `audio/mp4` (AAC) or `audio/webm` (Opus).
    pub mime: String,
    /// What it is, for people: "Opus 256 kb/s".
    pub quality: String,
    /// Its bitrate, to tell a better copy from a worse.
    pub kbps: Option<u32>,
    /// The song's video id, to keep the file in the cache once it is all
    /// downloaded; none for a file that comes from the cache.
    pub cache_key: Option<String>,
}

impl StreamInfo {
    pub fn header_map(&self) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in &self.headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                map.insert(name, value);
            }
        }
        map
    }
}

enum Command {
    Load {
        token: u64,
        stream: StreamInfo,
        start: Duration,
        play: bool,
    },
    Preload {
        token: u64,
        stream: StreamInfo,
        /// Whether it may fade in over the end of the song before it.
        crossfade: bool,
    },
    ClearPreload,
    /// How songs follow one another: a crossfade this long (zero: none),
    /// or else silence this long between them (zero: gapless).
    Transitions {
        crossfade: Duration,
        gap: Duration,
    },
    /// A better stream of the token playing, to carry on from without a
    /// break.
    Upgrade {
        token: u64,
        stream: StreamInfo,
    },
    Play,
    Pause,
    Seek(Duration),
    Stop,
    Normalize(bool),
}

/// What happened, for the app.
#[derive(Clone, Debug)]
pub enum Event {
    /// The token is now heard: a load started, or a preloaded stream took
    /// over without a gap.
    Audible(u64),
    /// The token played to its end and nothing was preloaded.
    Ended(u64),
    /// The better stream took over decoding; it is heard from `at` on.
    Upgraded { token: u64, at: Duration },
    /// The better stream could not take over; the first one plays on.
    UpgradeFailed(u64),
    /// The token cannot play. When `expired`, the stream's address stopped
    /// working and resolving it again may help; `position` is where it was.
    Failed {
        token: u64,
        message: String,
        expired: bool,
        position: Duration,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Buffering,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    /// The token loaded (or heard, once a preloaded one takes over).
    pub token: u64,
    pub state: State,
    pub duration: Option<Duration>,
    /// Fraction of the file downloaded.
    pub downloaded: f32,
}

struct Shared {
    status: Mutex<Status>,
    /// The source being read, so a new load or a seek can wake a read that
    /// is waiting on the network.
    source: Mutex<Option<Arc<source::Shared>>>,
    /// Where a seek is going, shown until the device catches up.
    seek_to: Mutex<Option<Duration>>,
}

/// The app's handle on the engine thread. Cheap to call every frame.
pub struct Player {
    tx: Sender<Command>,
    shared: Arc<Shared>,
    report: Arc<Report>,
    volume: Arc<AtomicU32>,
    knob: f32,
    eq: Arc<eq::EqControl>,
}

impl Player {
    pub fn spawn(
        runtime: tokio::runtime::Handle,
        client: reqwest::Client,
        events: Sender<Event>,
        wake: Arc<dyn Fn() + Send + Sync>,
        normalize: bool,
        cache: Arc<crate::cache::AudioCache>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            status: Mutex::new(Status::default()),
            source: Mutex::new(None),
            seek_to: Mutex::new(None),
        });
        let report = Arc::new(Report::default());
        let volume = Arc::new(AtomicU32::new(curve(0.8).to_bits()));
        let eq = Arc::new(eq::EqControl::default());
        let engine = Engine {
            eq: eq.clone(),
            rx,
            pending: VecDeque::new(),
            events,
            wake,
            runtime,
            client,
            cache,
            shared: shared.clone(),
            report: report.clone(),
            volume: volume.clone(),
            queue: Arc::new(Mutex::new(Queue::default())),
            output: None,
            output_retry: None,
            configured: 0,
            current: None,
            next: None,
            playing: false,
            refilling: true,
            last_audible: 0,
            ended_sent: false,
            normalize,
            last_maintain: Instant::now(),
            pause_due: None,
            crossfade: Duration::ZERO,
            gap: Duration::ZERO,
            incoming: None,
        };
        std::thread::Builder::new()
            .name("player".into())
            .spawn(move || engine.run())
            .expect("player thread");
        Self {
            tx,
            shared,
            report,
            volume,
            knob: 0.8,
            eq,
        }
    }

    /// The equaliser: on or off, and its bands in dB.
    pub fn set_eq(&self, on: bool, gains: &[f32; 10]) {
        self.eq.set(on, gains);
    }

    fn interrupt(&self) {
        if let Some(source) = &*self.shared.source.lock() {
            source.interrupt();
        }
    }

    pub fn load(&self, token: u64, stream: StreamInfo, start: Duration, play: bool) {
        {
            let mut status = self.shared.status.lock();
            status.token = token;
            status.state = State::Loading;
            status.duration = stream.duration;
            status.downloaded = 0.0;
        }
        *self.shared.seek_to.lock() = Some(start);
        self.interrupt();
        let _ = self.tx.send(Command::Load {
            token,
            stream,
            start,
            play,
        });
    }

    /// The song after this one, to start without a gap, or fading in over
    /// this one's end when `crossfade` allows and crossfades are on.
    pub fn preload(&self, token: u64, stream: StreamInfo, crossfade: bool) {
        let _ = self.tx.send(Command::Preload { token, stream, crossfade });
    }

    /// Crossfades of `crossfade` (zero: none); otherwise `gap` of silence
    /// between songs (zero: gapless).
    pub fn set_transitions(&self, crossfade: Duration, gap: Duration) {
        let _ = self.tx.send(Command::Transitions { crossfade, gap });
    }

    pub fn clear_preload(&self) {
        let _ = self.tx.send(Command::ClearPreload);
    }

    /// Switches the token playing to a better stream of the same song, at
    /// the end of what is already decoded, crossfaded.
    pub fn upgrade(&self, token: u64, stream: StreamInfo) {
        let _ = self.tx.send(Command::Upgrade { token, stream });
    }

    pub fn play(&self) {
        let mut status = self.shared.status.lock();
        if status.state == State::Paused {
            status.state = State::Playing;
        }
        drop(status);
        let _ = self.tx.send(Command::Play);
    }

    pub fn pause(&self) {
        let mut status = self.shared.status.lock();
        if matches!(status.state, State::Playing | State::Buffering) {
            status.state = State::Paused;
        }
        drop(status);
        let _ = self.tx.send(Command::Pause);
    }

    pub fn seek(&self, to: Duration) {
        *self.shared.seek_to.lock() = Some(to);
        self.interrupt();
        let _ = self.tx.send(Command::Seek(to));
    }

    pub fn stop(&self) {
        {
            let mut status = self.shared.status.lock();
            *status = Status::default();
        }
        self.interrupt();
        let _ = self.tx.send(Command::Stop);
    }

    pub fn set_normalize(&self, on: bool) {
        let _ = self.tx.send(Command::Normalize(on));
    }

    /// The volume knob, from 0 to 1.
    pub fn set_volume(&mut self, knob: f32) {
        self.knob = knob.clamp(0.0, 1.0);
        self.volume
            .store(curve(self.knob).to_bits(), Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        self.knob
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().clone()
    }

    /// Where the heard track is.
    pub fn position(&self) -> Duration {
        if let Some(to) = *self.shared.seek_to.lock() {
            return to;
        }
        let status = self.shared.status.lock();
        if self.report.audible.load(Ordering::Relaxed) == status.token && status.token != 0 {
            Duration::from_nanos(self.report.position_ns.load(Ordering::Relaxed))
        } else {
            Duration::ZERO
        }
    }
}

/// The knob's travel to gain: a cube keeps the low end usable.
fn curve(knob: f32) -> f32 {
    knob.clamp(0.0, 1.0).powi(3)
}

struct Decoding {
    token: u64,
    format: Box<dyn FormatReader>,
    decoder: Decoder,
    track_id: u32,
    time_base: Option<TimeBase>,
    source: Arc<source::Shared>,
    stream: StreamInfo,
    converter: Option<Converter>,
    /// The track time of the next frame pushed.
    next_time: Duration,
    /// Input frames still to drop after an accurate seek.
    skip_frames: u64,
    /// Output frames still to mix into the queue's tail rather than append:
    /// the overlap with the stream this one replaced.
    blend_left: usize,
    blend_total: usize,
    finished: bool,
    scratch: Vec<f32>,
}

/// Symphonia's decoders (AAC), or libopus for Opus, which symphonia lacks.
enum Decoder {
    Symphonia(Box<dyn AudioDecoder>),
    Opus(opus::Opus),
}

impl Decoder {
    fn reset(&mut self) {
        match self {
            Decoder::Symphonia(decoder) => decoder.reset(),
            Decoder::Opus(decoder) => decoder.reset(),
        }
    }

    fn sample_rate(&self) -> u32 {
        match self {
            Decoder::Symphonia(decoder) => decoder.codec_params().sample_rate.unwrap_or(44_100),
            Decoder::Opus(_) => opus::RATE,
        }
    }

    /// How far before a seek target decoding starts, so the decoder has
    /// settled by the target: 80 ms for Opus, as RFC 7845 advises.
    fn preroll(&self) -> Duration {
        match self {
            Decoder::Symphonia(_) => Duration::ZERO,
            Decoder::Opus(_) => Duration::from_millis(80),
        }
    }
}

struct Next {
    token: u64,
    stream: StreamInfo,
    /// The download, started early so the switch does not wait for it.
    opened: Option<(RemoteSource, Arc<source::Shared>)>,
    crossfade: bool,
}

/// The next song, about to fade in over the end of the one playing: the
/// ending's last seconds and the next song's first, decoded and held back
/// while the smart crossfade finds where they meet (see [`smart`]).
struct Incoming {
    token: u64,
    stream: StreamInfo,
    decoding: Decoding,
    /// The song ending, from its decoder's time on: not yet queued.
    held: Vec<f32>,
    /// The song ending has no more: all that is left of it is held.
    ended: bool,
    /// The next song's start.
    head: Vec<f32>,
}

/// What one packet of a stream gave.
enum Block {
    Samples(Vec<f32>),
    /// The stream's end, with the resampler's last samples.
    End(Vec<f32>),
    Nothing,
    Interrupted,
    Failed { message: String, expired: bool },
}

enum OpenError {
    Interrupted,
    Failed { message: String, expired: bool },
}

struct Engine {
    rx: Receiver<Command>,
    pending: VecDeque<Command>,
    events: Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
    runtime: tokio::runtime::Handle,
    client: reqwest::Client,
    cache: Arc<crate::cache::AudioCache>,
    shared: Arc<Shared>,
    report: Arc<Report>,
    volume: Arc<AtomicU32>,
    eq: Arc<eq::EqControl>,
    queue: Arc<Mutex<Queue>>,
    output: Option<Output<Renderer>>,
    output_retry: Option<Instant>,
    configured: u64,
    current: Option<Decoding>,
    next: Option<Next>,
    playing: bool,
    /// Filling the queue back up to AHEAD, having fallen below LOW_WATER.
    refilling: bool,
    last_audible: u64,
    ended_sent: bool,
    normalize: bool,
    last_maintain: Instant,
    /// When the device stops, once a pause has settled (SETTLE).
    pause_due: Option<Instant>,
    crossfade: Duration,
    gap: Duration,
    incoming: Option<Incoming>,
}

impl Engine {
    fn run(mut self) {
        loop {
            let busy = self.wants_decoding();
            let first = if let Some(command) = self.pending.pop_front() {
                Some(command)
            } else {
                // Sleep until there is decoding to do, waking often enough
                // while playing to notice what the device reports (a song
                // ending, the next one heard). A command wakes it at once.
                let wait = if busy {
                    Duration::ZERO
                } else if self.playing {
                    self.queued()
                        .saturating_sub(LOW_WATER)
                        .clamp(Duration::from_millis(10), Duration::from_millis(100))
                } else if self.current.is_some() {
                    Duration::from_millis(250)
                } else {
                    Duration::from_secs(1)
                };
                let wait = match self.pause_due {
                    Some(due) => wait.min(due.saturating_duration_since(Instant::now())),
                    None => wait,
                };
                match self.rx.recv_timeout(wait) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            if let Some(command) = first {
                self.pending.push_front(command);
                while let Ok(command) = self.rx.try_recv() {
                    self.pending.push_back(command);
                }
                while let Some(command) = self.pending.pop_front() {
                    self.handle(command);
                }
            }
            self.decode_for(Duration::from_millis(30));
            self.poll();
            self.maintain();
            self.settle_pause();
        }
    }

    /// Stops the device once a pause has had time to settle.
    fn settle_pause(&mut self) {
        if self.pause_due.is_some_and(|due| Instant::now() >= due) {
            self.pause_due = None;
            if !self.playing
                && let Some(output) = &mut self.output
            {
                output.pause();
            }
        }
    }

    /// Empties the queue without a click: what the device was about to play
    /// fades out (when it is playing), and what comes next fades in.
    fn cut_params(&self) -> (usize, usize, usize) {
        let (rate, channels) = self.out_format();
        let fade = output::fade_frames(rate);
        (if self.playing { fade } else { 0 }, fade, channels)
    }

    fn cut(&mut self) {
        let (out, fade_in, channels) = self.cut_params();
        self.queue.lock().cut(out, fade_in, channels);
    }

    fn send(&self, event: Event) {
        let _ = self.events.send(event);
        (self.wake)();
    }

    fn out_format(&self) -> (u32, usize) {
        let rate = self.report.rate.load(Ordering::SeqCst).max(8_000);
        let channels = self.report.channels.load(Ordering::SeqCst).max(1) as usize;
        (rate, channels)
    }

    fn ensure_output(&mut self) -> bool {
        if self.output.is_some() {
            return true;
        }
        if self.output_retry.is_some_and(|at| Instant::now() < at) {
            return false;
        }
        let renderer = Renderer::new(self.queue.clone(), self.report.clone(), self.volume.clone(), self.eq.clone());
        let options = OutputOptions {
            buffer: Buffer::FixedOnWindows(BufferSize::Duration(Duration::from_millis(100))),
            ..OutputOptions::default()
        };
        match Output::open(options, renderer) {
            Ok(mut output) => {
                log::info!(
                    "audio output: {} at {} Hz, {} channels",
                    output.device_name(),
                    output.sample_rate(),
                    output.channels()
                );
                self.configured = self.report.configured.load(Ordering::SeqCst);
                self.report.hold.store(!self.playing, Ordering::SeqCst);
                if !self.playing {
                    output.pause();
                }
                self.output = Some(output);
                self.output_retry = None;
                true
            }
            Err(error) => {
                log::warn!("cannot open the audio output: {error}");
                self.output_retry = Some(Instant::now() + Duration::from_secs(2));
                false
            }
        }
    }

    fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        self.report.hold.store(!playing, Ordering::SeqCst);
        if playing {
            self.pause_due = None;
            if self.ensure_output()
                && let Some(output) = &mut self.output
            {
                output.resume();
            }
        } else if self.output.as_ref().is_some_and(|output| !output.is_paused()) {
            // Not at once: the callback fades out and plays silence first
            // (see Report::hold), so the device keeps nothing to replay.
            self.pause_due.get_or_insert(Instant::now() + SETTLE);
        }
    }

    fn set_state(&self, state: State) {
        self.shared.status.lock().state = state;
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Load {
                token,
                stream,
                start,
                play,
            } => {
                self.current = None;
                self.next = None;
                self.incoming = None;
                self.cut();
                self.report.drained.store(false, Ordering::SeqCst);
                self.report.audible.store(0, Ordering::SeqCst);
                self.last_audible = 0;
                self.ended_sent = false;
                self.ensure_output();
                match self.open(token, stream.clone(), start, None) {
                    Ok(decoding) => {
                        self.current = Some(decoding);
                        *self.shared.seek_to.lock() = None;
                        self.report
                            .position_ns
                            .store(start.as_nanos() as u64, Ordering::SeqCst);
                        self.set_state(if play { State::Buffering } else { State::Paused });
                        self.set_playing(play);
                    }
                    Err(OpenError::Interrupted) => {
                        // A newer command replaces this one, unless it was
                        // only a seek: then load again from there.
                        let replaced = self
                            .pending
                            .iter()
                            .any(|c| matches!(c, Command::Load { .. } | Command::Stop));
                        if !replaced {
                            let start = self.take_seek().unwrap_or(start);
                            self.pending.push_front(Command::Load {
                                token,
                                stream,
                                start,
                                play,
                            });
                        }
                    }
                    Err(OpenError::Failed { message, expired }) => {
                        *self.shared.seek_to.lock() = None;
                        self.set_state(State::Idle);
                        self.send(Event::Failed {
                            token,
                            message,
                            expired,
                            position: start,
                        });
                    }
                }
            }
            Command::Preload { token, stream, crossfade } => {
                if self.next.as_ref().is_some_and(|next| next.token == token)
                    || self.incoming.as_ref().is_some_and(|incoming| incoming.token == token)
                {
                    return;
                }
                let opened = self.source_for(&stream);
                self.next = Some(Next {
                    token,
                    stream,
                    opened: Some(opened),
                    crossfade,
                });
                // A stream that had already ended can go on now.
                self.queue.lock().end_of_stream = false;
                self.ended_sent = false;
            }
            Command::ClearPreload => {
                if let Some(incoming) = &self.incoming {
                    if self.report.audible.load(Ordering::SeqCst) == incoming.token {
                        // Already heard: the app has moved on to it.
                        return;
                    }
                    // Not heard yet: the song playing goes on alone, decoded
                    // again from where it is heard.
                    self.incoming = None;
                    self.next = None;
                    let at = Duration::from_nanos(self.report.position_ns.load(Ordering::SeqCst));
                    self.seek(at);
                    return;
                }
                self.next = None;
                let finished = self.current.as_ref().is_none_or(|d| d.finished);
                if finished {
                    self.queue.lock().end_of_stream = true;
                }
            }
            Command::Play => {
                if self.current.is_some() || !self.queue.lock().runs.is_empty() {
                    self.set_playing(true);
                    self.set_state(State::Buffering);
                }
            }
            Command::Pause => {
                self.set_playing(false);
                // A pause that arrives while the load was still going on
                // must not leave the load's state behind.
                if self.current.is_some() || !self.queue.lock().runs.is_empty() {
                    self.set_state(State::Paused);
                }
            }
            Command::Upgrade { token, stream } => self.upgrade(token, stream),
            Command::Seek(to) => self.seek(to),
            Command::Transitions { crossfade, gap } => {
                self.crossfade = crossfade;
                self.gap = gap;
            }
            Command::Stop => {
                self.current = None;
                self.next = None;
                self.incoming = None;
                self.cut();
                self.report.audible.store(0, Ordering::SeqCst);
                *self.shared.source.lock() = None;
                *self.shared.seek_to.lock() = None;
                self.set_playing(false);
                self.set_state(State::Idle);
            }
            Command::Normalize(on) => self.normalize = on,
        }
    }

    /// A seek still waiting in the queue, taken out of it.
    fn take_seek(&mut self) -> Option<Duration> {
        let mut to = None;
        self.pending.retain(|command| match command {
            Command::Seek(at) => {
                to = Some(*at);
                false
            }
            _ => true,
        });
        to
    }

    fn seek(&mut self, to: Duration) {
        // The heard track may be a preloaded one the decoder already
        // finished; seeking means going back to it.
        let audible = self.report.audible.load(Ordering::SeqCst);
        // Mid-crossfade: the song heard is the one to seek in; the other
        // goes (the next one waiting its turn again).
        if let Some(incoming) = self.incoming.take() {
            if audible == incoming.token {
                self.current = Some(incoming.decoding);
            } else {
                self.next = Some(Next {
                    token: incoming.token,
                    stream: incoming.stream,
                    opened: None,
                    crossfade: true,
                });
            }
        }
        let (fade_out, fade_in, channels) = self.cut_params();
        let Some(decoding) = &mut self.current else {
            *self.shared.seek_to.lock() = None;
            return;
        };
        if audible != 0 && audible != decoding.token {
            log::debug!("seek in a track already decoded; ignoring");
            *self.shared.seek_to.lock() = None;
            return;
        }
        decoding.source.clear_interrupt();
        let from = to.saturating_sub(decoding.decoder.preroll());
        let time = Time::try_from_secs_f64(from.as_secs_f64()).unwrap_or(Time::ZERO);
        let result = decoding.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time,
                track_id: Some(decoding.track_id),
            },
        );
        match result {
            Ok(seeked) => {
                decoding.decoder.reset();
                if let Some(converter) = &mut decoding.converter {
                    converter.reset();
                }
                let skip = seeked.required_ts.get() - seeked.actual_ts.get();
                let rate = decoding.sample_rate();
                decoding.skip_frames = ts_to_frames(skip.max(0), decoding.time_base, rate)
                    + duration_to_frames(to - from, rate);
                decoding.next_time = to;
                decoding.blend_left = 0;
                decoding.finished = false;
                self.queue.lock().cut(fade_out, fade_in, channels);
                self.report.drained.store(false, Ordering::SeqCst);
                self.report
                    .position_ns
                    .store(to.as_nanos() as u64, Ordering::SeqCst);
                self.ended_sent = false;
                if self.playing {
                    self.set_state(State::Buffering);
                }
            }
            Err(error) => {
                if let Some(SourceError::Forbidden) = decoding.source.error() {
                    let token = decoding.token;
                    self.current = None;
                    self.send(Event::Failed {
                        token,
                        message: "stream expired".into(),
                        expired: true,
                        position: to,
                    });
                } else {
                    log::warn!("seek failed: {error}");
                }
            }
        }
        *self.shared.seek_to.lock() = None;
    }

    /// The bytes of a stream: read from a file on disk (the cache's, or
    /// the person's own), or downloaded, and then kept in the cache once
    /// whole.
    fn source_for(&self, stream: &StreamInfo) -> (RemoteSource, Arc<source::Shared>) {
        if let Some(path) = crate::cache::file_of(&stream.url) {
            return RemoteSource::from_file(path);
        }
        let complete = stream.cache_key.clone().filter(|_| self.cache.enabled()).map(|video_id| {
            let (cache, stream) = (self.cache.clone(), stream.clone());
            Box::new(move |bytes: Vec<u8>| cache.store(&video_id, &stream, bytes)) as source::Complete
        });
        RemoteSource::open(
            &self.runtime,
            self.client.clone(),
            stream.url.clone(),
            stream.header_map(),
            stream.content_length,
            complete,
        )
    }

    /// Takes over decoding from a better stream of the same song, at the end
    /// of what is queued. The queue's last moment is decoded again from the
    /// new stream and crossfaded, so the join cannot click.
    fn upgrade(&mut self, token: u64, mut stream: StreamInfo) {
        let (out_rate, _) = self.out_format();
        let Some(old) = self.current.as_ref().filter(|d| d.token == token && !d.finished && self.incoming.is_none()) else {
            self.send(Event::UpgradeFailed(token));
            return;
        };
        // The same loudness, or the level would jump at the join.
        if stream.loudness_db.is_none() {
            stream.loudness_db = old.stream.loudness_db;
        }
        let at = old.next_time;
        let queued = self.queue.lock().tail_frames(token);
        let blend = frames_to_time(queued, out_rate).min(BLEND);
        let from = at.saturating_sub(blend);
        match self.open(token, stream, from, None) {
            Ok(mut decoding) => {
                let total = (blend.as_secs_f64() * f64::from(out_rate)).round() as usize;
                decoding.blend_left = total;
                decoding.blend_total = total;
                decoding.next_time = from;
                log::info!("switching to {} at {:.1} s", decoding.stream.quality, at.as_secs_f64());
                self.current = Some(decoding);
                self.send(Event::Upgraded { token, at });
            }
            Err(error) => {
                // The first stream's source was replaced as the one to
                // interrupt; put it back.
                if let Some(old) = &self.current {
                    *self.shared.source.lock() = Some(old.source.clone());
                }
                let message = match error {
                    OpenError::Interrupted => "interrupted".to_owned(),
                    OpenError::Failed { message, .. } => message,
                };
                log::warn!("the better stream could not take over: {message}");
                self.send(Event::UpgradeFailed(token));
            }
        }
    }

    fn open(
        &mut self,
        token: u64,
        stream: StreamInfo,
        start: Duration,
        opened: Option<(RemoteSource, Arc<source::Shared>)>,
    ) -> Result<Decoding, OpenError> {
        let (reader, handle) = opened.unwrap_or_else(|| self.source_for(&stream));
        *self.shared.source.lock() = Some(handle.clone());
        handle.clear_interrupt();
        let failed = |error: SymError, handle: &source::Shared| {
            let message = error.to_string();
            if message.contains("interrupted") {
                OpenError::Interrupted
            } else {
                OpenError::Failed {
                    expired: matches!(handle.error(), Some(SourceError::Forbidden)),
                    message,
                }
            }
        };
        let mss = MediaSourceStream::new(Box::new(reader), Default::default());
        let mut hint = Hint::new();
        let extension = match stream.mime.as_str() {
            mime if mime.contains("webm") => "webm",
            mime if mime.contains("mp4") => "m4a",
            // A file of the person's own: whatever it is called.
            _ => crate::cache::file_of(&stream.url)
                .and_then(|path| path.extension())
                .and_then(|e| e.to_str())
                .unwrap_or_default(),
        };
        hint.with_extension(extension);
        hint.mime_type(&stream.mime);
        // Playing needs none of the pictures a file may carry, some of
        // them megabytes.
        let metadata = MetadataOptions::default().limit_visual_bytes(symphonia::core::common::Limit::Maximum(0));
        let format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), metadata)
            .map_err(|e| failed(e, &handle))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| OpenError::Failed {
                message: "no audio track".into(),
                expired: false,
            })?;
        let track_id = track.id;
        let time_base = track.time_base;
        // A file of the person's own may not say how long it is; its
        // container does.
        let mut stream = stream;
        if stream.duration.is_none() {
            stream.duration = crate::local::length(track);
        }
        let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
            return Err(OpenError::Failed {
                message: "not an audio track".into(),
                expired: false,
            });
        };
        let decoder = if params.codec == CODEC_ID_OPUS {
            Decoder::Opus(opus::Opus::new(&params).map_err(|message| OpenError::Failed {
                message,
                expired: false,
            })?)
        } else {
            Decoder::Symphonia(
                symphonia::default::get_codecs()
                    .make_audio_decoder(&params, &AudioDecoderOptions::default())
                    .map_err(|e| failed(e, &handle))?,
            )
        };
        // An Opus stream starts with the encoder's lead-in, which is not
        // meant to be heard.
        let lead_in = match &decoder {
            Decoder::Opus(opus) => opus.pre_skip,
            Decoder::Symphonia(_) => 0,
        };
        let mut decoding = Decoding {
            token,
            format,
            decoder,
            track_id,
            time_base,
            source: handle.clone(),
            stream,
            converter: None,
            next_time: Duration::ZERO,
            skip_frames: lead_in,
            blend_left: 0,
            blend_total: 0,
            finished: false,
            scratch: Vec::new(),
        };
        if !start.is_zero() {
            let from = start.saturating_sub(decoding.decoder.preroll());
            let time = Time::try_from_secs_f64(from.as_secs_f64()).unwrap_or(Time::ZERO);
            match decoding.format.seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(track_id),
                },
            ) {
                Ok(seeked) => {
                    let skip = seeked.required_ts.get() - seeked.actual_ts.get();
                    let rate = decoding.sample_rate();
                    decoding.skip_frames = ts_to_frames(skip.max(0), time_base, rate)
                        + duration_to_frames(start - from, rate);
                    decoding.next_time = start;
                }
                Err(error) => return Err(failed(error, &handle)),
            }
        }
        Ok(decoding)
    }

    fn queued(&self) -> Duration {
        let (rate, channels) = self.out_format();
        frames_to_time(self.queue.lock().frames(channels), rate)
    }

    fn wants_decoding(&mut self) -> bool {
        if self.output.is_none() {
            return false;
        }
        if let Some(incoming) = &self.incoming {
            // Looking ahead goes on whatever is queued, until as much of
            // the ending is held as is looked at.
            let (rate, channels) = self.out_format();
            let held = frames_to_time(incoming.held.len() / channels, rate);
            return incoming.ended || held < smart::SCAN || self.queued() < AHEAD;
        }
        let work = match &self.current {
            Some(decoding) => !decoding.finished || self.next.is_some(),
            None => false,
        };
        let queued = self.queued();
        if queued < LOW_WATER {
            self.refilling = true;
        } else if queued >= AHEAD {
            self.refilling = false;
        }
        work && self.refilling
    }

    /// Decodes until the queue is full or `budget` has passed, so commands
    /// are never held for long.
    fn decode_for(&mut self, budget: Duration) {
        let deadline = Instant::now() + budget;
        while self.wants_decoding() && Instant::now() < deadline {
            if !self.decode_one() {
                break;
            }
            if !self.pending.is_empty() {
                break;
            }
            if let Ok(command) = self.rx.try_recv() {
                self.pending.push_back(command);
                break;
            }
        }
    }

    /// Decodes one packet. False when nothing more can be done right now.
    fn decode_one(&mut self) -> bool {
        let (out_rate, out_channels) = self.out_format();
        let normalize = self.normalize;
        if self.incoming.is_some() {
            return self.crossfade_step(out_rate, out_channels, normalize);
        }
        if self.crossfade_due() {
            self.start_crossfade();
            if self.incoming.is_some() {
                return true;
            }
        }
        let Some(decoding) = &mut self.current else {
            return false;
        };
        if decoding.finished {
            return self.switch_to_next();
        }
        match decoding.next_block(out_rate, out_channels, normalize) {
            Block::Samples(samples) => decoding.deliver(&self.queue, &samples, out_rate, out_channels),
            Block::End(samples) => {
                decoding.deliver(&self.queue, &samples, out_rate, out_channels);
                if self.next.is_none() {
                    self.queue.lock().end_of_stream = true;
                }
            }
            Block::Nothing => {}
            Block::Interrupted => return false,
            Block::Failed { message, expired } => {
                let token = decoding.token;
                let position = decoding.next_time;
                log::warn!("stream failed: {message}");
                self.current = None;
                self.send(Event::Failed {
                    token,
                    message,
                    expired,
                    position,
                });
                return false;
            }
        }
        true
    }

    /// The song playing is within the stretch the smart crossfade looks at,
    /// and the next may fade in over it.
    fn crossfade_due(&self) -> bool {
        if self.crossfade.is_zero() || !self.next.as_ref().is_some_and(|next| next.crossfade) {
            return false;
        }
        let Some(current) = &self.current else {
            return false;
        };
        let Some(duration) = current.stream.duration.filter(|_| !current.finished && current.blend_left == 0) else {
            return false;
        };
        let left = duration.saturating_sub(current.next_time);
        // Too little left to look at is not worth it.
        left <= smart::SCAN && left >= Duration::from_secs(1)
    }

    /// Opens the next song, to look at its start beside this one's end.
    fn start_crossfade(&mut self) {
        let Some(mut next) = self.next.take() else {
            return;
        };
        let opened = next.opened.take();
        match self.open(next.token, next.stream.clone(), Duration::ZERO, opened) {
            Ok(decoding) => {
                self.incoming = Some(Incoming {
                    token: next.token,
                    stream: next.stream,
                    decoding,
                    held: Vec::new(),
                    ended: false,
                    head: Vec::new(),
                });
                // Opening made the next song the source to interrupt; the
                // song playing is still the one being read.
                if let Some(current) = &self.current {
                    *self.shared.source.lock() = Some(current.source.clone());
                }
            }
            Err(OpenError::Interrupted) => self.next = Some(next),
            Err(OpenError::Failed { message, expired }) => {
                // The song playing ends on its own; the app hears why the
                // next could not follow.
                self.send(Event::Failed {
                    token: next.token,
                    message,
                    expired,
                    position: Duration::ZERO,
                });
            }
        }
    }

    /// One packet of looking ahead: the rest of the song ending, then the
    /// start of the next; once both are in hand, they are joined.
    fn crossfade_step(&mut self, out_rate: u32, out_channels: usize, normalize: bool) -> bool {
        let channels = out_channels.max(1);
        let Some(mut incoming) = self.incoming.take() else {
            return false;
        };
        let scan = duration_to_frames(smart::SCAN, out_rate) as usize * channels;
        if !incoming.ended {
            match self.current.as_mut().map(|c| c.next_block(out_rate, out_channels, normalize)) {
                Some(Block::Samples(samples)) => incoming.held.extend(samples),
                Some(Block::End(samples)) => {
                    incoming.held.extend(samples);
                    incoming.ended = true;
                }
                Some(Block::Nothing) => {}
                Some(Block::Interrupted) => {
                    self.incoming = Some(incoming);
                    return false;
                }
                Some(Block::Failed { message, .. }) => {
                    // The song ending gives out early: the next follows on.
                    log::warn!("stream failed before a crossfade: {message}");
                    incoming.ended = true;
                }
                None => incoming.ended = true,
            }
            // More of the ending than is looked at goes to the device.
            if incoming.held.len() > scan {
                let extra = (incoming.held.len() - scan) / channels * channels;
                self.release(&mut incoming, extra, out_rate, channels);
            }
        } else if incoming.head.len() < scan && !incoming.decoding.finished {
            match incoming.decoding.next_block(out_rate, out_channels, normalize) {
                Block::Samples(samples) | Block::End(samples) => incoming.head.extend(samples),
                Block::Nothing => {}
                Block::Interrupted => {
                    self.incoming = Some(incoming);
                    return false;
                }
                Block::Failed { message, expired } => {
                    // The next song cannot play: the ending is heard as it
                    // is, and the app hears why.
                    log::warn!("the song to fade in failed: {message}");
                    let held = std::mem::take(&mut incoming.held);
                    self.release(&mut incoming, held.len(), out_rate, channels);
                    if self.current.as_ref().is_none_or(|c| c.finished) && self.next.is_none() {
                        self.queue.lock().end_of_stream = true;
                    }
                    self.send(Event::Failed {
                        token: incoming.token,
                        message,
                        expired,
                        position: Duration::ZERO,
                    });
                    return true;
                }
            }
        } else {
            self.join(incoming, out_rate, channels);
            return true;
        }
        // The device is not left to run dry while this goes on.
        if self.queued() < Duration::from_secs(1) {
            let second = (out_rate as usize * channels).min(incoming.held.len());
            self.release(&mut incoming, second, out_rate, channels);
        }
        self.incoming = Some(incoming);
        true
    }

    /// The first `samples` of the ending held back, to the device after all.
    fn release(&mut self, incoming: &mut Incoming, samples: usize, out_rate: u32, channels: usize) {
        let samples = samples.min(incoming.held.len());
        if samples == 0 {
            return;
        }
        if let Some(current) = &mut self.current {
            current.deliver(&self.queue, &incoming.held[..samples], out_rate, channels);
        }
        incoming.held.drain(..samples);
    }

    /// The two songs joined where they meet: the ending alone, then the
    /// overlap (queued as the next song's, so it shows from there on), then
    /// the next song alone. The silences around the join are left out.
    fn join(&mut self, incoming: Incoming, out_rate: u32, channels: usize) {
        let Incoming { token, mut decoding, held, head, .. } = incoming;
        let plan = smart::plan(&held, &head, channels, out_rate, self.crossfade);
        let secs = |frames: usize| frames as f64 / f64::from(out_rate);
        log::info!(
            "smart crossfade: {:.1} s overlapped, {:.1} s of silence cut from the end and {:.1} s from the start",
            secs(plan.overlap),
            secs(plan.cut(held.len() / channels)),
            secs(plan.skip)
        );
        let solo = plan.solo * channels;
        let overlap = plan.overlap * channels;
        if let Some(current) = &mut self.current {
            current.deliver(&self.queue, &held[..solo], out_rate, channels);
        }
        let skip = (plan.skip * channels).min(head.len());
        let next = &head[skip..];
        let mixed = smart::mix(&held[solo..solo + overlap], &next[..overlap.min(next.len())], channels);
        let start = frames_to_time(plan.skip, out_rate);
        self.queue.lock().push(token, start, &mixed, channels, out_rate);
        decoding.next_time = start + frames_to_time(plan.overlap, out_rate);
        if next.len() > overlap {
            decoding.deliver(&self.queue, &next[overlap..], out_rate, channels);
        }
        *self.shared.source.lock() = Some(decoding.source.clone());
        let finished = decoding.finished;
        self.current = Some(decoding);
        if finished && self.next.is_none() {
            self.queue.lock().end_of_stream = true;
        }
    }

    /// The current stream is fully decoded: carry on with the preloaded
    /// one, into the same queue, so the device hears no gap.
    fn switch_to_next(&mut self) -> bool {
        let Some(mut next) = self.next.take() else {
            return false;
        };
        let opened = next.opened.take();
        match self.open(next.token, next.stream.clone(), Duration::ZERO, opened) {
            Ok(decoding) => {
                if !self.gap.is_zero() {
                    // Gapless playback off: a moment of silence first.
                    let (rate, channels) = self.out_format();
                    let frames = duration_to_frames(self.gap, rate) as usize;
                    self.queue.lock().push(0, Duration::ZERO, &vec![0.0; frames * channels], channels, rate);
                }
                self.current = Some(decoding);
                true
            }
            Err(OpenError::Interrupted) => {
                self.next = Some(next);
                false
            }
            Err(OpenError::Failed { message, expired }) => {
                self.queue.lock().end_of_stream = true;
                self.send(Event::Failed {
                    token: next.token,
                    message,
                    expired,
                    position: Duration::ZERO,
                });
                false
            }
        }
    }

    /// Reads what the callback reported and tells the app.
    fn poll(&mut self) {
        let audible = self.report.audible.load(Ordering::SeqCst);
        if audible != 0 && audible != self.last_audible {
            self.last_audible = audible;
            let duration = [self.current.as_ref(), self.incoming.as_ref().map(|i| &i.decoding)]
                .into_iter()
                .flatten()
                .find(|d| d.token == audible)
                .map(|d| d.stream.duration);
            {
                let mut status = self.shared.status.lock();
                status.token = audible;
                if let Some(duration) = duration {
                    status.duration = duration;
                }
            }
            self.send(Event::Audible(audible));
        }
        let state = if !self.playing {
            None
        } else if self.report.starved.load(Ordering::Relaxed) {
            Some(State::Buffering)
        } else if self.last_audible != 0 {
            Some(State::Playing)
        } else {
            None
        };
        if let Some(state) = state {
            let mut status = self.shared.status.lock();
            if status.state != State::Loading || state == State::Playing {
                status.state = state;
            }
        }
        if let Some(decoding) = &self.current {
            let len = decoding.source.len().max(1) as f32;
            let have: u64 = decoding
                .source
                .downloaded()
                .iter()
                .map(|span| span.end - span.start)
                .sum();
            self.shared.status.lock().downloaded = have as f32 / len;
        }
        if self.report.drained.swap(false, Ordering::SeqCst) && !self.ended_sent {
            self.ended_sent = true;
            let token = self.last_audible;
            self.set_playing(false);
            self.set_state(State::Idle);
            self.current = None;
            self.send(Event::Ended(token));
        }
    }

    fn maintain(&mut self) {
        if self.last_maintain.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.last_maintain = Instant::now();
        let Some(output) = &mut self.output else {
            return;
        };
        match output.maintain() {
            Maintained::Reopened { sample_rate, .. } => {
                log::info!("audio output reopened at {sample_rate} Hz");
            }
            Maintained::Failed(error) => log::warn!("audio output failed: {error}"),
            _ => {}
        }
        for error in output.take_errors() {
            log::warn!("audio output: {error}");
        }
        let configured = self.report.configured.load(Ordering::SeqCst);
        if configured != self.configured {
            self.configured = configured;
            // Another rate or channel count: what is queued was made for the
            // old one. Decode again from where the device is.
            let audible = self.report.audible.load(Ordering::SeqCst);
            if self.current.as_ref().is_some_and(|d| d.token == audible) {
                let at = Duration::from_nanos(self.report.position_ns.load(Ordering::SeqCst));
                self.seek(at);
            }
        }
    }
}

impl Decoding {
    fn sample_rate(&self) -> u32 {
        self.decoder.sample_rate()
    }

    fn converter(&mut self, in_rate: u32, out_rate: u32, channels: usize) -> &mut Converter {
        if !self
            .converter
            .as_ref()
            .is_some_and(|c| c.matches(in_rate, out_rate, channels))
        {
            self.converter = Some(Converter::new(in_rate, out_rate, channels));
        }
        self.converter.as_mut().expect("just made")
    }

    /// Decodes one packet into output samples (the output's rate and
    /// channels). The time is left for whoever takes the samples to move.
    fn next_block(&mut self, out_rate: u32, out_channels: usize, normalize: bool) -> Block {
        if self.finished {
            return Block::End(Vec::new());
        }
        let packet = match self.format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) | Err(SymError::ResetRequired) => return Block::End(self.flush(out_channels)),
            Err(error) => {
                let message = error.to_string();
                if message.contains("interrupted") {
                    return Block::Interrupted;
                }
                if self.source.error().is_none() && matches!(error, SymError::IoError(_)) {
                    // The demuxer ran past the end of a file whose length
                    // was a little off: treat it as the end.
                    return Block::End(self.flush(out_channels));
                }
                return Block::Failed {
                    expired: matches!(self.source.error(), Some(SourceError::Forbidden)),
                    message,
                };
            }
        };
        if packet.track_id != self.track_id {
            return Block::Nothing;
        }
        let decoded = match &mut self.decoder {
            Decoder::Symphonia(decoder) => match decoder.decode(&packet) {
                Ok(buffer) => {
                    let format = (buffer.spec().rate(), buffer.spec().channels().count().max(1));
                    self.scratch.clear();
                    buffer.copy_to_vec_interleaved(&mut self.scratch);
                    Ok(format)
                }
                Err(SymError::DecodeError(error)) => Err(format!("decode error: {error}")),
                Err(error) => {
                    log::warn!("decoder: {error}");
                    return Block::Nothing;
                }
            },
            Decoder::Opus(decoder) => decoder
                .decode(&packet.data, &mut self.scratch)
                .map(|()| (opus::RATE, decoder.channels)),
        };
        match decoded {
            Ok((in_rate, in_channels)) => {
                let gain = if normalize {
                    self.stream.loudness_db.map_or(1.0, |db| {
                        // YouTube only turns loud tracks down.
                        if db > 0.0 { 10f32.powf(-db / 20.0) } else { 1.0 }
                    })
                } else {
                    1.0
                };
                Block::Samples(self.produce(in_rate, in_channels, out_rate, out_channels, gain))
            }
            Err(error) => {
                log::debug!("{error}");
                Block::Nothing
            }
        }
    }

    /// The packet just decoded, past any frames to skip, mapped to the
    /// output's channels and resampled to its rate.
    fn produce(&mut self, in_rate: u32, in_channels: usize, out_rate: u32, out_channels: usize, gain: f32) -> Vec<f32> {
        let mut frames = std::mem::take(&mut self.scratch);
        if self.skip_frames > 0 {
            let available = (frames.len() / in_channels) as u64;
            let skip = self.skip_frames.min(available);
            self.skip_frames -= skip;
            frames.drain(..skip as usize * in_channels);
        }
        let mapped = remix(&frames, in_channels, out_channels, gain);
        self.scratch = frames;
        let mut out = Vec::with_capacity(mapped.len() * 2);
        self.converter(in_rate, out_rate, out_channels)
            .process(&mapped, &mut |block| out.extend_from_slice(block));
        out
    }

    /// The end: what the resampler still holds.
    fn flush(&mut self, out_channels: usize) -> Vec<f32> {
        self.finished = true;
        let mut out = Vec::new();
        if let Some(converter) = &mut self.converter {
            converter.finish(&mut |block| out.extend_from_slice(block));
        }
        let _ = out_channels;
        out
    }

    /// Into the queue, the time moving on with it. While a better stream is
    /// taking over, its first frames are mixed into the queue's tail
    /// instead (or dropped if that was heard): never appended twice.
    fn deliver(&mut self, queue: &Mutex<Queue>, samples: &[f32], out_rate: u32, out_channels: usize) {
        let channels = out_channels.max(1);
        let mut block = samples;
        let mut queue = queue.lock();
        if self.blend_left > 0 {
            let n = (block.len() / channels).min(self.blend_left);
            let (overlap, rest) = block.split_at(n * channels);
            queue.blend(self.token, overlap, channels, self.blend_left, self.blend_total);
            self.blend_left -= n;
            self.next_time += frames_to_time(n, out_rate);
            block = rest;
        }
        queue.push(self.token, self.next_time, block, channels, out_rate);
        self.next_time += frames_to_time(block.len() / channels, out_rate);
    }
}

/// Maps interleaved frames to the output's channels, applying `gain`.
fn remix(input: &[f32], from: usize, to: usize, gain: f32) -> Vec<f32> {
    let frames = input.len() / from.max(1);
    let mut out = Vec::with_capacity(frames * to);
    for frame in input.chunks_exact(from) {
        for channel in 0..to {
            let sample = match (from, channel) {
                (1, _) => frame[0],
                (_, c) if c < from => frame[c],
                _ => 0.0,
            };
            out.push(sample * gain);
        }
    }
    out
}

fn duration_to_frames(duration: Duration, rate: u32) -> u64 {
    (duration.as_secs_f64() * f64::from(rate)).round() as u64
}

fn ts_to_frames(ts: i64, time_base: Option<TimeBase>, rate: u32) -> u64 {
    let Some(tb) = time_base else {
        return ts.max(0) as u64;
    };
    let secs = tb
        .calc_time(Timestamp::new(ts))
        .map_or(0.0, |time| time.as_secs_f64());
    (secs * f64::from(rate)).round().max(0.0) as u64
}

#[cfg(test)]
mod bench {
    use super::*;

    /// Run with `cargo test --release --lib bench -- --ignored --nocapture`:
    /// what decoding and resampling cost per second of audio.
    #[test]
    #[ignore = "a timing, not a test"]
    fn decode_and_resample_cost() {
        const SECONDS: usize = 30;
        // Something music-like to encode: chords with some noise.
        let mut signal = Vec::with_capacity(48_000 * SECONDS * 2);
        for i in 0..48_000 * SECONDS {
            let t = i as f32 / 48_000.0;
            let s = 0.2 * (t * 220.0 * std::f32::consts::TAU).sin()
                + 0.15 * (t * 277.2 * std::f32::consts::TAU).sin()
                + 0.1 * (t * 329.6 * std::f32::consts::TAU).sin()
                + 0.05 * (fastrand::f32() - 0.5);
            signal.push(s);
            signal.push(s * 0.9);
        }
        let mut error = 0;
        let packets: Vec<Vec<u8>> = unsafe {
            let encoder = unsafe_libopus::opus_encoder_create(48_000, 2, unsafe_libopus::OPUS_APPLICATION_AUDIO, &mut error);
            let out: Vec<Vec<u8>> = signal
                .chunks_exact(960 * 2)
                .map(|frame| {
                    let mut packet = vec![0u8; 1500];
                    let n = unsafe_libopus::opus_encode_float(encoder, frame.as_ptr(), 960, packet.as_mut_ptr(), 1500);
                    packet.truncate(n.max(0) as usize);
                    packet
                })
                .collect();
            unsafe_libopus::opus_encoder_destroy(encoder);
            out
        };
        let mut params = symphonia::core::codecs::audio::AudioCodecParameters::new();
        params.with_channels(symphonia::core::audio::Channels::Discrete(2));
        let mut decoder = opus::Opus::new(&params).expect("decoder");
        let mut pcm = Vec::new();
        let mut decoded = Vec::with_capacity(signal.len());
        let started = Instant::now();
        for packet in &packets {
            decoder.decode(packet, &mut pcm).expect("decodes");
            decoded.extend_from_slice(&pcm);
        }
        let decode = started.elapsed();
        let mut converter = Converter::new(48_000, 96_000, 2);
        let started = Instant::now();
        let mut frames = 0usize;
        for chunk in decoded.chunks(960 * 2) {
            converter.process(chunk, &mut |block| frames += block.len() / 2);
        }
        let resample = started.elapsed();
        let started = Instant::now();
        let mut total = 0usize;
        for chunk in decoded.chunks(960 * 2) {
            total += remix(chunk, 2, 2, 0.8).len();
        }
        let remixing = started.elapsed();
        let per = |d: Duration| d.as_secs_f64() / SECONDS as f64 * 100.0;
        println!(
            "per second of audio, as % of a core: opus decode {:.3}%, resample 48k->96k {:.3}%, remix {:.3}% ({frames} frames, {total})",
            per(decode),
            per(resample),
            per(remixing)
        );
    }
}
