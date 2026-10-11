//! The playback engine: one thread that downloads, demuxes, decodes and
//! resamples, feeding the device through fastframe-audio.
//!
//! The app hands the player resolved streams under tokens of its own. The
//! player says which token is audible, where it is, and when it ended or
//! failed; the app decides what plays next. A stream given with
//! [`Player::preload`] starts the moment the current one ends, with no gap.

mod opus;
pub mod output;
mod resample;
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
}

impl StreamInfo {
    fn header_map(&self) -> HeaderMap {
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
    },
    ClearPreload,
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
}

impl Player {
    pub fn spawn(
        runtime: tokio::runtime::Handle,
        client: reqwest::Client,
        events: Sender<Event>,
        wake: Arc<dyn Fn() + Send + Sync>,
        normalize: bool,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            status: Mutex::new(Status::default()),
            source: Mutex::new(None),
            seek_to: Mutex::new(None),
        });
        let report = Arc::new(Report::default());
        let volume = Arc::new(AtomicU32::new(curve(0.8).to_bits()));
        let engine = Engine {
            rx,
            pending: VecDeque::new(),
            events,
            wake,
            runtime,
            client,
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
            last_audible: 0,
            ended_sent: false,
            normalize,
            last_maintain: Instant::now(),
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
        }
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

    pub fn preload(&self, token: u64, stream: StreamInfo) {
        let _ = self.tx.send(Command::Preload { token, stream });
    }

    pub fn clear_preload(&self) {
        let _ = self.tx.send(Command::ClearPreload);
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
    shared: Arc<Shared>,
    report: Arc<Report>,
    volume: Arc<AtomicU32>,
    queue: Arc<Mutex<Queue>>,
    output: Option<Output<Renderer>>,
    output_retry: Option<Instant>,
    configured: u64,
    current: Option<Decoding>,
    next: Option<Next>,
    playing: bool,
    last_audible: u64,
    ended_sent: bool,
    normalize: bool,
    last_maintain: Instant,
}

impl Engine {
    fn run(mut self) {
        loop {
            let busy = self.wants_decoding();
            let first = if let Some(command) = self.pending.pop_front() {
                Some(command)
            } else {
                let wait = if busy {
                    Duration::ZERO
                } else {
                    Duration::from_millis(25)
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
        }
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
        let renderer = Renderer::new(self.queue.clone(), self.report.clone(), self.volume.clone());
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
        if playing {
            if self.ensure_output()
                && let Some(output) = &mut self.output
            {
                output.resume();
            }
        } else if let Some(output) = &mut self.output {
            output.pause();
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
                self.queue.lock().clear();
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
            Command::Preload { token, stream } => {
                if self.next.as_ref().is_some_and(|next| next.token == token) {
                    return;
                }
                let opened = RemoteSource::open(
                    &self.runtime,
                    self.client.clone(),
                    stream.url.clone(),
                    stream.header_map(),
                    stream.content_length,
                );
                self.next = Some(Next {
                    token,
                    stream,
                    opened: Some(opened),
                });
                // A stream that had already ended can go on now.
                self.queue.lock().end_of_stream = false;
                self.ended_sent = false;
            }
            Command::ClearPreload => {
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
            Command::Seek(to) => self.seek(to),
            Command::Stop => {
                self.current = None;
                self.next = None;
                self.queue.lock().clear();
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
                decoding.finished = false;
                let mut queue = self.queue.lock();
                queue.clear();
                drop(queue);
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

    fn open(
        &mut self,
        token: u64,
        stream: StreamInfo,
        start: Duration,
        opened: Option<(RemoteSource, Arc<source::Shared>)>,
    ) -> Result<Decoding, OpenError> {
        let (reader, handle) = opened.unwrap_or_else(|| {
            RemoteSource::open(
                &self.runtime,
                self.client.clone(),
                stream.url.clone(),
                stream.header_map(),
                stream.content_length,
            )
        });
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
        hint.with_extension(if stream.mime.contains("webm") { "webm" } else { "m4a" });
        hint.mime_type(&stream.mime);
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| failed(e, &handle))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| OpenError::Failed {
                message: "no audio track".into(),
                expired: false,
            })?;
        let track_id = track.id;
        let time_base = track.time_base;
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

    fn wants_decoding(&self) -> bool {
        if self.output.is_none() {
            return false;
        }
        let work = match &self.current {
            Some(decoding) => !decoding.finished || self.next.is_some(),
            None => false,
        };
        work && self.queued() < AHEAD
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
        let Some(decoding) = &mut self.current else {
            return false;
        };
        if decoding.finished {
            return self.switch_to_next();
        }
        let packet = match decoding.format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) | Err(SymError::ResetRequired) => {
                decoding.finish(&self.queue, out_rate, out_channels);
                if self.next.is_none() {
                    self.queue.lock().end_of_stream = true;
                }
                return true;
            }
            Err(error) => {
                let message = error.to_string();
                if message.contains("interrupted") {
                    return false;
                }
                let expired = matches!(decoding.source.error(), Some(SourceError::Forbidden));
                if decoding.source.error().is_none() && matches!(error, SymError::IoError(_)) {
                    // The demuxer ran past the end of a file whose length
                    // was a little off: treat it as the end.
                    decoding.finish(&self.queue, out_rate, out_channels);
                    if self.next.is_none() {
                        self.queue.lock().end_of_stream = true;
                    }
                    return true;
                }
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
        };
        if packet.track_id != decoding.track_id {
            return true;
        }
        let decoded = match &mut decoding.decoder {
            Decoder::Symphonia(decoder) => match decoder.decode(&packet) {
                Ok(buffer) => {
                    let format = (buffer.spec().rate(), buffer.spec().channels().count().max(1));
                    decoding.scratch.clear();
                    buffer.copy_to_vec_interleaved(&mut decoding.scratch);
                    Ok(format)
                }
                Err(SymError::DecodeError(error)) => Err(format!("decode error: {error}")),
                Err(error) => {
                    log::warn!("decoder: {error}");
                    return true;
                }
            },
            Decoder::Opus(decoder) => decoder
                .decode(&packet.data, &mut decoding.scratch)
                .map(|()| (opus::RATE, decoder.channels)),
        };
        match decoded {
            Ok((in_rate, in_channels)) => {
                let gain = if normalize {
                    decoding.stream.loudness_db.map_or(1.0, |db| {
                        // YouTube only turns loud tracks down.
                        if db > 0.0 { 10f32.powf(-db / 20.0) } else { 1.0 }
                    })
                } else {
                    1.0
                };
                decoding.push(
                    &self.queue,
                    in_rate,
                    in_channels,
                    out_rate,
                    out_channels,
                    gain,
                );
            }
            Err(error) => log::debug!("{error}"),
        }
        true
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
            let duration = [self.current.as_ref()]
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

    fn push(
        &mut self,
        queue: &Mutex<Queue>,
        in_rate: u32,
        in_channels: usize,
        out_rate: u32,
        out_channels: usize,
        gain: f32,
    ) {
        let mut frames = std::mem::take(&mut self.scratch);
        if self.skip_frames > 0 {
            let available = (frames.len() / in_channels) as u64;
            let skip = self.skip_frames.min(available);
            self.skip_frames -= skip;
            frames.drain(..skip as usize * in_channels);
        }
        let mapped = remix(&frames, in_channels, out_channels, gain);
        self.scratch = frames;
        let token = self.token;
        let mut next_time = self.next_time;
        let converter = self.converter(in_rate, out_rate, out_channels);
        converter.process(&mapped, &mut |block| {
            queue
                .lock()
                .push(token, next_time, block, out_channels, out_rate);
            next_time += frames_to_time(block.len() / out_channels, out_rate);
        });
        self.next_time = next_time;
    }

    fn finish(&mut self, queue: &Mutex<Queue>, out_rate: u32, out_channels: usize) {
        self.finished = true;
        let token = self.token;
        let mut next_time = self.next_time;
        if let Some(converter) = &mut self.converter {
            converter.finish(&mut |block| {
                queue
                    .lock()
                    .push(token, next_time, block, out_channels, out_rate);
                next_time += frames_to_time(block.len() / out_channels, out_rate);
            });
        }
        self.next_time = next_time;
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
