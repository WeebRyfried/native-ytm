//! The visualiser's ears: the last moments heard, kept by the audio
//! callback without a lock, and their spectrum, in bands as the eye likes
//! them: spaced by octaves, from 40 Hz up, with the highs lifted (music has
//! less there, so a flat reading leaves the right half asleep).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

/// How the visualiser draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Slim rounded bars, with caps that fall slowly.
    Bars,
    /// A smooth line over a soft fill, an echo of it behind.
    #[default]
    Wave,
    /// The wave from the middle out, the bass at its centre.
    Mirror,
    /// A haze of light rising from the bar.
    Aurora,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Bars, Mode::Wave, Mode::Mirror, Mode::Aurora];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Bars => "Bars",
            Mode::Wave => "Wave",
            Mode::Mirror => "Mirror",
            Mode::Aurora => "Aurora",
        }
    }
}

/// Mono samples kept, at the output's rate: a little under a second at
/// 48 kHz, enough for the largest look behind the speakers' delay.
const LEN: usize = 1 << 15;

/// How far behind what the callback writes the speakers are: about the
/// output's buffer, which is 100 ms.
const LAG_SECS: f32 = 0.09;

/// What the audio callback has just played, for the visualiser. The
/// callback writes; the interface reads; neither waits for the other.
pub struct Scope {
    /// The visualiser wants it: the callback writes only then.
    pub on: AtomicBool,
    samples: Box<[AtomicU32]>,
    /// Frames written in all.
    written: AtomicUsize,
    rate: AtomicU32,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            on: AtomicBool::new(false),
            samples: (0..LEN).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicUsize::new(0),
            rate: AtomicU32::new(48_000),
        }
    }
}

impl Scope {
    /// The callback's block, interleaved, before the volume.
    pub fn write(&self, block: &[f32], channels: usize, rate: u32) {
        if !self.on.load(Ordering::Relaxed) {
            return;
        }
        let channels = channels.max(1);
        let mut at = self.written.load(Ordering::Relaxed);
        for frame in block.chunks_exact(channels) {
            let mono = frame.iter().sum::<f32>() / channels as f32;
            self.samples[at % LEN].store(mono.to_bits(), Ordering::Relaxed);
            at = at.wrapping_add(1);
        }
        self.rate.store(rate, Ordering::Relaxed);
        self.written.store(at, Ordering::Release);
    }

    /// The `out.len()` samples about to be heard, oldest first, and their
    /// rate. What has not been written yet reads as silence.
    pub fn read(&self, out: &mut [f32]) -> u32 {
        let rate = self.rate.load(Ordering::Relaxed).max(1);
        let lag = (rate as f32 * LAG_SECS) as usize;
        let end = self.written.load(Ordering::Acquire).saturating_sub(lag);
        let n = out.len().min(LEN - lag - 1);
        let start = end.saturating_sub(n);
        out.fill(0.0);
        let skip = out.len() - (end - start);
        for (i, slot) in out[skip..].iter_mut().enumerate() {
            *slot = f32::from_bits(self.samples[(start + i) % LEN].load(Ordering::Relaxed));
        }
        rate
    }
}

/// The lowest band's foot, and the most the highest goes to.
const LOWEST_HZ: f32 = 40.0;
const HIGHEST_HZ: f32 = 16_000.0;
/// The range shown, from nothing to full, in dB below the loudest band of
/// late: the scale follows the music, so a quiet song moves as much as a
/// loud one.
const RANGE_DB: f32 = 30.0;
/// The loudest of late falls this fast, in dB a second, when the music
/// quietens; it never goes below `QUIETEST_DB`, so silence stays dark.
const REFERENCE_FALL: f32 = 5.0;
const QUIETEST_DB: f32 = -48.0;
/// The lift for the highs, per octave above 1 kHz (and the cut below).
const TILT_DB: f32 = 3.0;
/// How long a band's power is averaged over, in seconds: one look at
/// 43 ms swings by 10 dB from the next, an average reads the loudness.
const POWER_SECS: f32 = 0.08;
/// How fast a band rises and falls, in seconds to most of the way.
const RISE: f32 = 0.04;
const FALL: f32 = 0.14;
/// How fast a bar's cap and the wave's echo come down, per second.
const PEAK_FALL: f32 = 0.45;

/// The spectrum in `bands`, smoothed from frame to frame.
#[derive(Default)]
pub struct Analyser {
    size: usize,
    window: Vec<f32>,
    twiddles: Vec<(f32, f32)>,
    re: Vec<f32>,
    im: Vec<f32>,
    targets: Vec<f32>,
    /// Each band's power, averaged over `POWER_SECS`.
    power: Vec<f32>,
    levels: Vec<f32>,
    peaks: Vec<f32>,
    /// The loudest band of late, in dB.
    reference: f32,
}

impl Analyser {
    /// Moves on by `dt` seconds: toward what `scope` holds while `live`,
    /// toward silence otherwise. False once all is still and dark.
    pub fn update(&mut self, scope: &Scope, bands: usize, live: bool, dt: f32) -> bool {
        let bands = bands.max(2);
        if self.levels.len() != bands {
            self.levels = vec![0.0; bands];
            self.peaks = vec![0.0; bands];
            self.targets = vec![0.0; bands];
            self.power = vec![0.0; bands];
            self.reference = QUIETEST_DB;
        }
        let dt = dt.clamp(0.0, 0.1);
        if live {
            self.analyse(scope, bands, dt);
        } else {
            self.targets.fill(0.0);
        }
        let (rise, fall) = (1.0 - (-dt / RISE).exp(), 1.0 - (-dt / FALL).exp());
        for ((level, peak), target) in self.levels.iter_mut().zip(&mut self.peaks).zip(&self.targets) {
            let k = if *target > *level { rise } else { fall };
            *level += (target - *level) * k;
            *peak = level.max(*peak - PEAK_FALL * dt);
        }
        self.peaks.iter().any(|p| *p > 0.002)
    }

    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    pub fn peaks(&self) -> &[f32] {
        &self.peaks
    }

    fn analyse(&mut self, scope: &Scope, bands: usize, dt: f32) {
        let rate_now = scope.rate.load(Ordering::Relaxed);
        // About 43 ms however fast the output runs, for bass that tells
        // its notes apart without smearing the beat.
        let size = if rate_now > 64_000 { 4096 } else { 2048 };
        if self.size != size {
            self.size = size;
            self.window = (0..size)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (size - 1) as f32).cos())
                .collect();
            self.twiddles = twiddles(size);
            self.re = vec![0.0; size];
            self.im = vec![0.0; size];
        }
        let rate = scope.read(&mut self.re) as f32;
        for (sample, w) in self.re.iter_mut().zip(&self.window) {
            *sample *= w;
        }
        self.im.fill(0.0);
        fft(&mut self.re, &mut self.im, &self.twiddles);
        // A full-scale sine reads 1: the Hann window keeps half of it, and
        // half of that lands in the positive bin.
        let scale = 4.0 / size as f32;
        let bin_hz = rate / size as f32;
        let magnitude = |bin: usize| {
            let bin = bin.min(size / 2);
            (self.re[bin] * self.re[bin] + self.im[bin] * self.im[bin]).sqrt() * scale
        };
        let highest = HIGHEST_HZ.min(rate * 0.45);
        let span = (highest / LOWEST_HZ).ln();
        let keep = 1.0 - (-dt / POWER_SECS).exp();
        for (b, (target, power)) in self.targets.iter_mut().zip(&mut self.power).enumerate() {
            let f0 = LOWEST_HZ * (span * b as f32 / bands as f32).exp();
            let f1 = LOWEST_HZ * (span * (b + 1) as f32 / bands as f32).exp();
            let (i0, i1) = (f0 / bin_hz, f1 / bin_hz);
            let now = if i1 - i0 < 1.0 {
                // Narrower than a bin, low down: read between bins.
                let at = (i0 + i1) / 2.0;
                let (below, t) = (at.floor() as usize, at.fract());
                (magnitude(below) * (1.0 - t) + magnitude(below + 1) * t).powi(2)
            } else {
                // Wider, higher up: the bins' power, averaged.
                let bins = i0.floor() as usize..=i1.ceil() as usize;
                let count = bins.clone().count() as f32;
                bins.map(|b| magnitude(b).powi(2)).sum::<f32>() / count
            };
            *power += (now - *power) * keep;
            let centre = (f0 * f1).sqrt();
            *target = 10.0 * power.max(1e-18).log10() + TILT_DB * (centre / 1000.0).log2();
        }
        // The scale: up at once to anything louder, down slowly.
        let loudest = self.targets.iter().copied().fold(f32::MIN, f32::max);
        self.reference = if loudest > self.reference { loudest } else { self.reference - REFERENCE_FALL * dt }.max(QUIETEST_DB);
        let top = self.reference;
        for target in &mut self.targets {
            let level = ((*target - (top - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0);
            // Eased, so the quieter parts stay low and the loud ones stand
            // out.
            *target = level.powf(1.8);
        }
        // Neighbours lean on each other a little: smooth, not jagged.
        let raw = self.targets.clone();
        for (i, target) in self.targets.iter_mut().enumerate() {
            let left = raw[i.saturating_sub(1)];
            let right = raw[(i + 1).min(raw.len() - 1)];
            *target = 0.15 * left + 0.7 * raw[i] + 0.15 * right;
        }
    }
}

/// The FFT's turns for `size`: cos and sin of -2πk/size, for k up to half.
fn twiddles(size: usize) -> Vec<(f32, f32)> {
    (0..size / 2)
        .map(|k| {
            let (sin, cos) = (-std::f64::consts::TAU * k as f64 / size as f64).sin_cos();
            (cos as f32, sin as f32)
        })
        .collect()
}

/// An in-place radix-2 FFT; `re.len()` is a power of two, and `turns` its
/// `twiddles`.
fn fft(re: &mut [f32], im: &mut [f32], turns: &[(f32, f32)]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let stride = n / len;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = turns[k * stride];
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_lands_in_its_bin() {
        let n = 1024;
        let mut re: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * 64.0 * i as f32 / n as f32).sin()).collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im, &twiddles(n));
        let mags: Vec<f32> = re.iter().zip(&im).map(|(r, i)| (r * r + i * i).sqrt()).collect();
        let loudest = (0..n / 2).max_by(|a, b| mags[*a].total_cmp(&mags[*b])).unwrap();
        assert_eq!(loudest, 64);
        assert!((mags[64] - n as f32 / 2.0).abs() < 0.5);
    }

    #[test]
    fn a_tone_lights_its_band_and_silence_fades() {
        let scope = Scope::default();
        scope.on.store(true, Ordering::Relaxed);
        // A second of 1 kHz at half scale, stereo, at 48 kHz.
        let block: Vec<f32> = (0..48_000)
            .flat_map(|i| {
                let s = 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect();
        scope.write(&block, 2, 48_000);
        let mut analyser = Analyser::default();
        for _ in 0..30 {
            analyser.update(&scope, 32, true, 1.0 / 60.0);
        }
        let levels = analyser.levels();
        let loudest = (0..32).max_by(|a, b| levels[*a].total_cmp(&levels[*b])).unwrap();
        // 1 kHz sits about halfway up a 40 Hz to 16 kHz scale by octaves.
        assert!((15..=18).contains(&loudest), "loudest band {loudest}");
        assert!(levels[loudest] > 0.6, "lit: {}", levels[loudest]);
        assert!(levels[2] < 0.05, "bass stays dark: {}", levels[2]);
        for _ in 0..240 {
            analyser.update(&scope, 32, false, 1.0 / 60.0);
        }
        assert!(!analyser.update(&scope, 32, false, 1.0 / 60.0), "dark again");
    }

    #[test]
    fn off_it_writes_nothing() {
        let scope = Scope::default();
        scope.write(&[1.0; 64], 2, 48_000);
        let mut out = [0.5; 16];
        scope.read(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
    }
}
