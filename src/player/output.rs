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
    /// Frames still to fade in, of `fade_total`: what is pushed after a cut
    /// starts from silence rather than with a step.
    fade_in: usize,
    fade_total: usize,
}

/// How long a cut fades out, and what follows it fades in, in frames: 10 ms.
pub fn fade_frames(rate: u32) -> usize {
    (rate as usize / 100).max(1)
}

impl Queue {
    pub fn clear(&mut self) {
        self.samples.clear();
        self.runs.clear();
        self.end_of_stream = false;
        self.fade_in = 0;
    }

    /// Stops what is queued without a click: its first `fade_out` frames
    /// stay, fading to silence, under no token (0), and the rest goes. What
    /// is pushed next fades in over `fade_in` frames.
    pub fn cut(&mut self, fade_out: usize, fade_in: usize, channels: usize) {
        let channels = channels.max(1);
        // Still fading out from an earlier cut: that fade stands (and a
        // silence between songs is cut short like anything else).
        let keep = match self.runs.front().filter(|run| run.token == 0) {
            Some(run) => run.frames.min(fade_out),
            None => {
                let keep = fade_out.min(self.frames(channels));
                for i in 0..keep {
                    let w = 1.0 - (i + 1) as f32 / (keep + 1) as f32;
                    for c in 0..channels {
                        self.samples[i * channels + c] *= w;
                    }
                }
                keep
            }
        };
        self.samples.truncate(keep * channels);
        self.runs.clear();
        if keep > 0 {
            self.runs.push_back(Run {
                token: 0,
                start: Duration::ZERO,
                frames: keep,
                played: 0,
            });
        }
        self.end_of_stream = false;
        self.fade_in = fade_in;
        self.fade_total = fade_in;
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
        if self.fade_in > 0 {
            let total = self.fade_total.max(1);
            for (i, frame) in samples.chunks_exact(channels.max(1)).enumerate() {
                let left = self.fade_in.saturating_sub(i);
                let w = if left > 0 { (total - left + 1) as f32 / (total + 1) as f32 } else { 1.0 };
                self.samples.extend(frame.iter().map(|s| s * w));
            }
            self.fade_in = self.fade_in.saturating_sub(frames);
        } else {
            self.samples.extend(samples.iter().copied());
        }
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

impl Queue {
    /// Mixes `samples`, another stream's version of the same stretch of
    /// `token`, into the queue's last `left` frames, fading from what is
    /// queued to them: frame `i` of the fade is `total - left + i` of
    /// `total`. False when those frames are no longer queued.
    pub fn blend(&mut self, token: u64, samples: &[f32], channels: usize, left: usize, total: usize) -> bool {
        let frames = samples.len() / channels.max(1);
        let Some(last) = self.runs.back() else {
            return false;
        };
        if last.token != token || last.frames < left || frames > left {
            return false;
        }
        let start = self.samples.len() - left * channels;
        for i in 0..frames {
            // Linear: the two are the same recording, so their sum keeps
            // its level all through the fade.
            let w = (total - left + i + 1) as f32 / (total + 1) as f32;
            for c in 0..channels {
                let slot = &mut self.samples[start + i * channels + c];
                *slot += (samples[i * channels + c] - *slot) * w;
            }
        }
        true
    }

    /// Frames still queued at the end for `token`.
    pub fn tail_frames(&self, token: u64) -> usize {
        self.runs
            .back()
            .filter(|run| run.token == token)
            .map_or(0, |run| run.frames)
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
    /// Paused: the callback fades out, then plays silence and takes nothing
    /// from the queue, until the device itself is stopped a moment later.
    /// Stopped at once, the device would keep the last tenth of a second it
    /// was given and play it when it starts again, into the next song.
    pub hold: AtomicBool,
}

pub struct Renderer {
    pub queue: Arc<Mutex<Queue>>,
    pub report: Arc<Report>,
    /// The volume knob, as f32 bits, already on its perceptual curve.
    pub volume: Arc<AtomicU32>,
    gain: f32,
    channels: usize,
    rate: u32,
    /// Held and already faded out.
    silent: bool,
    eq_control: Arc<super::eq::EqControl>,
    eq: super::eq::Equalizer,
}

impl Renderer {
    pub fn new(queue: Arc<Mutex<Queue>>, report: Arc<Report>, volume: Arc<AtomicU32>, eq_control: Arc<super::eq::EqControl>) -> Self {
        Self {
            queue,
            report,
            volume,
            gain: 0.0,
            channels: 2,
            rate: 48_000,
            silent: false,
            eq_control,
            eq: super::eq::Equalizer::default(),
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
        let hold = self.report.hold.load(Ordering::Relaxed);
        if hold && self.silent {
            out.fill(0.0);
            return;
        }
        let Some(mut queue) = self.queue.try_lock_for(Duration::from_millis(1)) else {
            out.fill(0.0);
            return;
        };
        let channels = self.channels;
        // Held: a short fade of what comes next, then nothing more taken.
        let wanted = if hold {
            (out.len() / channels).min(fade_frames(self.rate))
        } else {
            out.len() / channels
        };
        let mut written = 0;
        while written < wanted {
            let Some(run) = queue.runs.front_mut() else {
                break;
            };
            let take = run.frames.min(wanted - written);
            run.frames -= take;
            run.played += take;
            // Token 0 is a cut's fade: heard, but no track's.
            if run.token != 0 {
                let position = run.start + frames_to_time(run.played, self.rate);
                self.report.audible.store(run.token, Ordering::Relaxed);
                self.report
                    .position_ns
                    .store(position.as_nanos() as u64, Ordering::Relaxed);
            }
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
        if hold {
            // What is left of a cut's fade is not wanted after the pause.
            while let Some(frames) = queue.runs.front().filter(|run| run.token == 0).map(|run| run.frames) {
                queue.samples.drain(..frames * channels);
                queue.runs.pop_front();
            }
        }
        let drained = queue.runs.is_empty();
        let end = queue.end_of_stream;
        drop(queue);
        out[written * channels..].fill(0.0);
        // The equaliser, on what is heard now: a change is heard at once.
        self.eq.process(&self.eq_control, &mut out[..written * channels], self.rate, channels);
        if hold {
            // From where the volume was down to silence.
            let n = written.max(1) as f32;
            for (i, frame) in out[..written * channels].chunks_mut(channels).enumerate() {
                let gain = self.gain * (1.0 - (i + 1) as f32 / n);
                for sample in frame {
                    *sample *= gain;
                }
            }
            self.gain = 0.0;
            self.silent = true;
            return;
        }
        self.silent = false;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_fades_the_tail_into_the_new_stream() {
        let mut queue = Queue::default();
        queue.push(7, Duration::ZERO, &[1.0; 20], 2, 48_000);
        // The new stream's version of the last four frames, in two blocks.
        assert!(queue.blend(7, &[0.0; 4], 2, 4, 4));
        assert!(queue.blend(7, &[0.0; 4], 2, 2, 4));
        let tail: Vec<f32> = queue.samples.iter().skip(12).step_by(2).copied().collect();
        // From all old toward all new, never a jump.
        assert_eq!(tail.len(), 4);
        assert!(tail.windows(2).all(|w| w[1] < w[0]));
        assert!((tail[0] - 0.8).abs() < 1e-6 && (tail[3] - 0.2).abs() < 1e-6);
        // Another token, or more than is queued, is refused.
        assert!(!queue.blend(8, &[0.0; 2], 2, 1, 1));
        assert!(!queue.blend(7, &[0.0; 2], 2, 11, 11));
    }

    #[test]
    fn a_cut_fades_out_and_what_follows_fades_in() {
        let mut queue = Queue::default();
        queue.push(1, Duration::ZERO, &[1.0; 40], 2, 48_000);
        queue.cut(4, 4, 2);
        // Four frames stay, fading toward silence, under no track.
        assert_eq!(queue.runs.len(), 1);
        assert_eq!(queue.runs[0].token, 0);
        let fading: Vec<f32> = queue.samples.iter().step_by(2).copied().collect();
        assert_eq!(fading.len(), 4);
        assert!(fading.windows(2).all(|w| w[1] < w[0]) && fading[3] < 0.25);
        // A second cut keeps that fade as it is.
        queue.cut(4, 4, 2);
        assert_eq!(queue.frames(2), 4);
        queue.push(2, Duration::ZERO, &[1.0; 12], 2, 48_000);
        let next: Vec<f32> = queue.samples.iter().skip(8).step_by(2).copied().collect();
        assert!(next.windows(2).all(|w| w[1] >= w[0]));
        assert!(next[0] < 0.25 && next[5] == 1.0);
    }

    #[test]
    fn held_it_fades_then_takes_nothing() {
        use fastframe_audio::Render;
        let queue = Arc::new(Mutex::new(Queue::default()));
        let report = Arc::new(Report::default());
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let mut renderer = Renderer::new(queue.clone(), report.clone(), volume, Arc::default());
        // Ten frames of fade at 1 kHz.
        renderer.configure(1000, 1);
        queue.lock().push(1, Duration::ZERO, &[1.0; 100], 1, 1000);
        let mut out = [0.0f32; 20];
        renderer.render(&mut out);
        report.hold.store(true, Ordering::Relaxed);
        renderer.render(&mut out);
        assert!(out[..10].windows(2).all(|w| w[1] <= w[0]) && out[9] == 0.0);
        assert!(out[10..].iter().all(|s| *s == 0.0));
        assert_eq!(queue.lock().frames(1), 70);
        renderer.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(queue.lock().frames(1), 70);
        // Resumed, it fades in from silence and carries on.
        report.hold.store(false, Ordering::Relaxed);
        renderer.render(&mut out);
        assert!(out[0] < 0.1 && (out[19] - 1.0).abs() < 1e-5);
        assert_eq!(queue.lock().frames(1), 50);
    }
}
