//! The audio callback's side: decoded samples waiting to be heard, the run
//! of each track among them, and what the callback reports back.
//!
//! The engine thread pushes resampled samples; the device callback pops them.
//! They share a mutex that the engine holds only to copy a block in, and the
//! callback waits for it at most a millisecond before playing silence.
//! Everything the interface reads (the position, what is audible) is in
//! atomics, so the interface never touches the callback's lock.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

/// A stretch of samples from one track, starting at a known point in it.
pub struct Run {
    pub token: u64,
    /// The track time of the run's first frame.
    pub start: Duration,
    /// Frames still in the queue.
    pub frames: usize,
    /// Frames already played.
    pub played: usize,
}

#[derive(Default)]
pub struct Queue {
    pub samples: VecDeque<f32>,
    pub runs: VecDeque<Run>,
    /// The engine has nothing more to push: once the queue drains, playback
    /// has ended.
    pub end_of_stream: bool,
}

impl Queue {
    pub fn clear(&mut self) {
        self.samples.clear();
        self.runs.clear();
        self.end_of_stream = false;
    }

    /// Frames queued, at the output's channel count.
    pub fn frames(&self, channels: usize) -> usize {
        self.samples.len() / channels.max(1)
    }

    /// Appends frames of `token` that continue at `start` in the track. A run
    /// that continues the last one extends it.
    pub fn push(&mut self, token: u64, start: Duration, samples: &[f32], channels: usize, rate: u32) {
        let frames = samples.len() / channels.max(1);
        if frames == 0 {
            return;
        }
        self.samples.extend(samples.iter().copied());
        if let Some(last) = self.runs.back_mut()
            && last.token == token
        {
            let end = last.start + frames_to_time(last.played + last.frames, rate);
            let gap = start.abs_diff(end);
            if gap < Duration::from_millis(5) {
                last.frames += frames;
                return;
            }
        }
        self.runs.push_back(Run {
            token,
            start,
            frames,
            played: 0,
        });
    }
}

pub fn frames_to_time(frames: usize, rate: u32) -> Duration {
    Duration::from_nanos((frames as u128 * 1_000_000_000 / u128::from(rate.max(1))) as u64)
}

/// What the callback tells the rest of the player, lock-free.
#[derive(Default)]
pub struct Report {
    /// The token of the track being heard, or 0.
    pub audible: AtomicU64,
    /// Its position in nanoseconds.
    pub position_ns: AtomicU64,
    /// The queue ran dry with the engine still promising more.
    pub starved: AtomicBool,
    /// The queue drained after the end of the stream.
    pub drained: AtomicBool,
    /// The output's rate and channels, from `configure`.
    pub rate: AtomicU32,
    pub channels: AtomicU32,
    /// Bumped on every `configure`, so the engine notices a reopen at
    /// another rate.
    pub configured: AtomicU64,
}

pub struct Renderer {
    pub queue: Arc<Mutex<Queue>>,
    pub report: Arc<Report>,
    /// The volume knob, as f32 bits, already on its perceptual curve.
    pub volume: Arc<AtomicU32>,
    gain: f32,
    channels: usize,
    rate: u32,
}

impl Renderer {
    pub fn new(queue: Arc<Mutex<Queue>>, report: Arc<Report>, volume: Arc<AtomicU32>) -> Self {
        Self {
            queue,
            report,
            volume,
            gain: 0.0,
            channels: 2,
            rate: 48_000,
        }
    }
}

impl fastframe_audio::Render for Renderer {
    fn configure(&mut self, sample_rate: u32, channels: u16) {
        self.rate = sample_rate;
        self.channels = usize::from(channels.max(1));
        self.report.rate.store(sample_rate, Ordering::SeqCst);
        self.report.channels.store(u32::from(channels), Ordering::SeqCst);
        self.report.configured.fetch_add(1, Ordering::SeqCst);
    }

    fn render(&mut self, out: &mut [f32]) {
        let target = f32::from_bits(self.volume.load(Ordering::Relaxed));
        let Some(mut queue) = self.queue.try_lock_for(Duration::from_millis(1)) else {
            out.fill(0.0);
            return;
        };
        let channels = self.channels;
        let wanted = out.len() / channels;
        let mut written = 0;
        while written < wanted {
            let Some(run) = queue.runs.front_mut() else {
                break;
            };
            let take = run.frames.min(wanted - written);
            run.frames -= take;
            run.played += take;
            let position = run.start + frames_to_time(run.played, self.rate);
            self.report.audible.store(run.token, Ordering::Relaxed);
            self.report
                .position_ns
                .store(position.as_nanos() as u64, Ordering::Relaxed);
            if run.frames == 0 {
                queue.runs.pop_front();
            }
            let samples = take * channels;
            for (slot, sample) in out[written * channels..written * channels + samples]
                .iter_mut()
                .zip(queue.samples.drain(..samples))
            {
                *slot = sample;
            }
            written += take;
        }
        let drained = queue.runs.is_empty();
        let end = queue.end_of_stream;
        drop(queue);
        out[written * channels..].fill(0.0);
        if written < wanted && drained {
            if end {
                self.report.drained.store(true, Ordering::Relaxed);
            } else {
                self.report.starved.store(true, Ordering::Relaxed);
            }
        } else {
            self.report.starved.store(false, Ordering::Relaxed);
        }
        // Ramp the volume across the block so a change never clicks.
        let frames = wanted.max(1) as f32;
        let step = (target - self.gain) / frames;
        for frame in out.chunks_mut(channels) {
            self.gain += step;
            for sample in frame {
                *sample *= self.gain;
            }
        }
        self.gain = target;
    }
}
