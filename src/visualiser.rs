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

/// What the visualiser follows most.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Focus {
    /// The whole mix, the hits in it standing out.
    #[default]
    All,
    /// The beat: kicks, snares and cymbals leap, held notes stay low.
    Drums,
    /// The voice: the middle of the range, and what sits in the centre of
    /// the stereo picture, where singers are mixed.
    Vocals,
}

impl Focus {
    pub const ALL: [Focus; 3] = [Focus::All, Focus::Drums, Focus::Vocals];

    pub fn name(self) -> &'static str {
        match self {
            Focus::All => "Everything",
            Focus::Drums => "Drums",
            Focus::Vocals => "Vocals",
        }
    }
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

/// Frames kept, at the output's rate: a little under a second at 48 kHz,
/// enough for the largest look behind the speakers' delay.
const LEN: usize = 1 << 15;

/// How far behind what the callback writes the speakers are: about the
/// output's buffer, which is 100 ms.
const LAG_SECS: f32 = 0.09;

/// What the audio callback has just played, for the visualiser. The
/// callback writes; the interface reads; neither waits for the other.
pub struct Scope {
    /// The visualiser wants it: the callback writes only then.
    pub on: AtomicBool,
    /// The middle of the stereo picture, left and right together, and its
    /// sides, left less right: a voice mixed in the centre is all middle.
    mid: Box<[AtomicU32]>,
    side: Box<[AtomicU32]>,
    /// Frames written in all.
    written: AtomicUsize,
    rate: AtomicU32,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            on: AtomicBool::new(false),
            mid: (0..LEN).map(|_| AtomicU32::new(0)).collect(),
            side: (0..LEN).map(|_| AtomicU32::new(0)).collect(),
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
            let mid = frame.iter().sum::<f32>() / channels as f32;
            let side = if channels >= 2 { (frame[0] - frame[1]) / 2.0 } else { 0.0 };
            self.mid[at % LEN].store(mid.to_bits(), Ordering::Relaxed);
            self.side[at % LEN].store(side.to_bits(), Ordering::Relaxed);
            at = at.wrapping_add(1);
        }
        self.rate.store(rate, Ordering::Relaxed);
        self.written.store(at, Ordering::Release);
    }

    /// The `out.len()` samples about to be heard, oldest first, of the
    /// middle (or with `side`, the sides), and their rate. What has not been
    /// written yet reads as silence.
    pub fn read(&self, out: &mut [f32], side: bool) -> u32 {
        let ring = if side { &self.side } else { &self.mid };
        let rate = self.rate.load(Ordering::Relaxed).max(1);
        let lag = (rate as f32 * LAG_SECS) as usize;
        let end = self.written.load(Ordering::Acquire).saturating_sub(lag);
        let n = out.len().min(LEN - lag - 1);
        let start = end.saturating_sub(n);
        out.fill(0.0);
        let skip = out.len() - (end - start);
        for (i, slot) in out[skip..].iter_mut().enumerate() {
            *slot = f32::from_bits(ring[(start + i) % LEN].load(Ordering::Relaxed));
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
const REFERENCE_FALL: f32 = 6.0;
const QUIETEST_DB: f32 = -48.0;
/// The lift for the highs, per octave above 1 kHz (and the cut below).
const TILT_DB: f32 = 3.0;
/// Each band's power is followed twice: closely, for what is heard now
/// (one look at 43 ms swings too much to read alone), and loosely, for
/// what the band has been doing. A hit is the first well above the second.
const NOW_SECS: f32 = 0.025;
const BACKGROUND_SECS: f32 = 0.35;
/// A hit this far above the background, in dB, shows in full.
const HIT_DB: f32 = 9.0;
/// How fast a band rises, in seconds to most of the way.
const RISE: f32 = 0.018;
/// How fast a bar's cap and the wave's echo come down, per second.
const PEAK_FALL: f32 = 0.5;

/// How each focus reads the bands.
struct Weights {
    /// How much of a band's height is its loudness, and how much its hits.
    loudness: f32,
    hits: f32,
    /// How much a kick lifts every band at once.
    pulse: f32,
    /// How fast a band falls, in seconds to most of the way.
    fall: f32,
}

impl Focus {
    fn weights(self) -> Weights {
        match self {
            Focus::All => Weights { loudness: 0.85, hits: 0.4, pulse: 0.08, fall: 0.09 },
            Focus::Drums => Weights { loudness: 0.45, hits: 1.0, pulse: 0.22, fall: 0.065 },
            Focus::Vocals => Weights { loudness: 1.0, hits: 0.2, pulse: 0.0, fall: 0.11 },
        }
    }

    /// The lift or cut, in dB, for a band centred on `hz`.
    fn tilt(self, hz: f32) -> f32 {
        let bell = |at: f32, width: f32| (-((hz / at).log2() / width).powi(2)).exp();
        match self {
            Focus::All => 0.0,
            // The kick's low thump and the snare's and cymbals' crack up,
            // the voice and pads between them down a little.
            Focus::Drums => 4.0 * bell(70.0, 1.1) + 3.0 * bell(4500.0, 1.4) - 4.0 * bell(700.0, 1.0),
            // A voice, fundamentals to the bright edge of its consonants:
            // a broad hill around 900 Hz, and the rest well down.
            Focus::Vocals => -14.0 * (1.0 - bell(900.0, 1.5)),
        }
    }
}

/// The spectrum in `bands`, smoothed from frame to frame.
#[derive(Default)]
pub struct Analyser {
    size: usize,
    window: Vec<f32>,
    twiddles: Vec<(f32, f32)>,
    re: Vec<f32>,
    im: Vec<f32>,
    side_re: Vec<f32>,
    side_im: Vec<f32>,
    targets: Vec<f32>,
    hits: Vec<f32>,
    /// Each band's power now, and its background, as in `NOW_SECS` and
    /// `BACKGROUND_SECS`.
    now: Vec<f32>,
    background: Vec<f32>,
    levels: Vec<f32>,
    peaks: Vec<f32>,
    /// The loudest band of late, in dB.
    reference: f32,
}

impl Analyser {
    /// Moves on by `dt` seconds: toward what `scope` holds while `live`,
    /// toward silence otherwise, following `focus`. False once all is still
    /// and dark.
    pub fn update(&mut self, scope: &Scope, bands: usize, live: bool, dt: f32, focus: Focus) -> bool {
        let bands = bands.max(2);
        if self.levels.len() != bands {
            self.levels = vec![0.0; bands];
            self.peaks = vec![0.0; bands];
            self.targets = vec![0.0; bands];
            self.hits = vec![0.0; bands];
            self.now = vec![0.0; bands];
            self.background = vec![0.0; bands];
            self.reference = QUIETEST_DB;
        }
        let dt = dt.clamp(0.0, 0.1);
        if live {
            self.analyse(scope, bands, dt, focus);
        } else {
            self.targets.fill(0.0);
        }
        let (rise, fall) = (1.0 - (-dt / RISE).exp(), 1.0 - (-dt / focus.weights().fall).exp());
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

    /// One look at the scope, windowed and through the FFT, into `re` and
    /// `im` (or, with `side`, the sides' own); the rate it ran at.
    fn transform(&mut self, scope: &Scope, side: bool) -> f32 {
        let (re, im) = if side { (&mut self.side_re, &mut self.side_im) } else { (&mut self.re, &mut self.im) };
        let rate = scope.read(re, side) as f32;
        for (sample, w) in re.iter_mut().zip(&self.window) {
            *sample *= w;
        }
        im.fill(0.0);
        fft(re, im, &self.twiddles);
        rate
    }

    fn analyse(&mut self, scope: &Scope, bands: usize, dt: f32, focus: Focus) {
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
            self.side_re = vec![0.0; size];
            self.side_im = vec![0.0; size];
        }
        let rate = self.transform(scope, false);
        let vocals = focus == Focus::Vocals;
        if vocals {
            self.transform(scope, true);
        }
        // A full-scale sine reads 1: the Hann window keeps half of it, and
        // half of that lands in the positive bin.
        let scale = (4.0 / size as f32).powi(2);
        let (re, im, side_re, side_im) = (&self.re, &self.im, &self.side_re, &self.side_im);
        let power = |bin: usize| {
            let bin = bin.min(size / 2);
            let middle = (re[bin] * re[bin] + im[bin] * im[bin]) * scale;
            if !vocals {
                return middle;
            }
            // What is also in the sides is spread across the stereo picture:
            // guitars, keys, reverb. A voice in the centre is not, and stays.
            let sides = (side_re[bin] * side_re[bin] + side_im[bin] * side_im[bin]) * scale;
            (middle - 1.5 * sides).max(middle * 0.02)
        };
        let bin_hz = rate / size as f32;
        let highest = HIGHEST_HZ.min(rate * 0.45);
        let span = (highest / LOWEST_HZ).ln();
        let (close, loose) = (1.0 - (-dt / NOW_SECS).exp(), 1.0 - (-dt / BACKGROUND_SECS).exp());
        let db = |p: f32| 10.0 * p.max(1e-18).log10();
        // Each band's loudness (in dB, lifted or cut for the focus) into
        // `targets` for now, and its hit, from 0 to 1, into `hits`.
        let mut kick = (0.0f32, 0usize);
        for b in 0..bands {
            let f0 = LOWEST_HZ * (span * b as f32 / bands as f32).exp();
            let f1 = LOWEST_HZ * (span * (b + 1) as f32 / bands as f32).exp();
            let (i0, i1) = (f0 / bin_hz, f1 / bin_hz);
            let heard = if i1 - i0 < 1.0 {
                // Narrower than a bin, low down: read between bins.
                let at = (i0 + i1) / 2.0;
                let (below, t) = (at.floor() as usize, at.fract());
                power(below) * (1.0 - t) + power(below + 1) * t
            } else {
                // Wider, higher up: the bins' power, averaged.
                let bins = i0.floor() as usize..=i1.ceil() as usize;
                let count = bins.clone().count() as f32;
                bins.map(power).sum::<f32>() / count
            };
            let (now, background) = (&mut self.now[b], &mut self.background[b]);
            *now += (heard - *now) * close;
            *background += (heard - *background) * loose;
            let centre = (f0 * f1).sqrt();
            self.targets[b] = db(*now) + TILT_DB * (centre / 1000.0).log2() + focus.tilt(centre);
            self.hits[b] = ((db(*now) - db(*background)) / HIT_DB).clamp(0.0, 1.0);
            if centre < 150.0 {
                kick = (kick.0 + self.hits[b], kick.1 + 1);
            }
        }
        // The scale: up at once to anything louder, down slowly.
        let loudest = self.targets.iter().copied().fold(f32::MIN, f32::max);
        self.reference = if loudest > self.reference { loudest } else { self.reference - REFERENCE_FALL * dt }.max(QUIETEST_DB);
        let top = self.reference;
        let weights = focus.weights();
        let pulse = if kick.1 > 0 { kick.0 / kick.1 as f32 } else { 0.0 };
        for (target, hit) in self.targets.iter_mut().zip(&self.hits) {
            // Eased, so the quieter parts stay low and the loud ones stand
            // out; hits and the kick only where something is heard at all.
            let loudness = ((*target - (top - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0);
            let heard = (loudness * 3.0).min(1.0);
            let level = weights.loudness * loudness.powf(1.6) + (weights.hits * hit + weights.pulse * pulse) * heard;
            *target = level.min(1.0);
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
            analyser.update(&scope, 32, true, 1.0 / 60.0, Focus::All);
        }
        let levels = analyser.levels();
        let loudest = (0..32).max_by(|a, b| levels[*a].total_cmp(&levels[*b])).unwrap();
        // 1 kHz sits about halfway up a 40 Hz to 16 kHz scale by octaves.
        assert!((15..=18).contains(&loudest), "loudest band {loudest}");
        assert!(levels[loudest] > 0.6, "lit: {}", levels[loudest]);
        assert!(levels[2] < 0.05, "bass stays dark: {}", levels[2]);
        for _ in 0..240 {
            analyser.update(&scope, 32, false, 1.0 / 60.0, Focus::All);
        }
        assert!(!analyser.update(&scope, 32, false, 1.0 / 60.0, Focus::All), "dark again");
    }

    /// `secs` of stereo at 48 kHz from `sample(t) -> (left, right)`, fed
    /// in at 30 frames a second, as the app does; each frame's levels.
    fn run(focus: Focus, secs: f32, sample: impl Fn(f32) -> (f32, f32)) -> Vec<Vec<f32>> {
        let scope = Scope::default();
        scope.on.store(true, Ordering::Relaxed);
        let mut analyser = Analyser::default();
        let mut at = 0usize;
        let mut frames = Vec::new();
        for _ in 0..(secs * 30.0) as usize {
            let block: Vec<f32> = (0..1600)
                .flat_map(|_| {
                    let (l, r) = sample(at as f32 / 48_000.0);
                    at += 1;
                    [l, r]
                })
                .collect();
            scope.write(&block, 2, 48_000);
            analyser.update(&scope, 32, true, 1.0 / 30.0, focus);
            frames.push(analyser.levels().to_vec());
        }
        frames
    }

    fn band(hz: f32) -> usize {
        ((hz / LOWEST_HZ).ln() / (HIGHEST_HZ / LOWEST_HZ).ln() * 32.0) as usize
    }

    fn sine(hz: f32, t: f32) -> f32 {
        (std::f32::consts::TAU * hz * t).sin()
    }

    #[test]
    fn vocals_leave_out_what_is_off_to_one_side() {
        // A voice in the middle at 500 Hz, a guitar hard left at 1.5 kHz.
        let mix = |t: f32| {
            let (voice, guitar) = (0.3 * sine(500.0, t), 0.6 * sine(1500.0, t));
            (voice + guitar, voice)
        };
        let ratio = |focus| {
            let frames = run(focus, 3.0, mix);
            let mean = |b: usize| frames[60..].iter().map(|f| f[b]).sum::<f32>() / 30.0;
            mean(band(1500.0)) / mean(band(500.0)).max(1e-3)
        };
        let (all, vocals) = (ratio(Focus::All), ratio(Focus::Vocals));
        assert!(all > 0.6, "both heard in the mix: {all}");
        assert!(vocals < all * 0.4, "the guitar drops away: {vocals} against {all}");
    }

    #[test]
    fn drums_leap_on_the_kick_and_let_held_notes_sink() {
        // A held chord at 700 Hz, and a kick at 60 Hz twice a second.
        let song = |t: f32| {
            let since = t % 0.5;
            let kick = if since < 0.08 { 0.9 * sine(60.0, t) * (1.0 - since / 0.08) } else { 0.0 };
            let s = 0.3 * sine(700.0, t) + kick;
            (s, s)
        };
        let (all, drums) = (run(Focus::All, 4.0, song), run(Focus::Drums, 4.0, song));
        let swing = |frames: &[Vec<f32>], b: usize| {
            let values: Vec<f32> = frames[30..].iter().map(|f| f[b]).collect();
            values.iter().copied().fold(0.0, f32::max) - values.iter().copied().fold(1.0, f32::min)
        };
        let mean = |frames: &[Vec<f32>], b: usize| frames[30..].iter().map(|f| f[b]).sum::<f32>() / (frames.len() - 30) as f32;
        let kick = band(60.0);
        assert!(swing(&drums, kick) > 0.6, "the kick leaps: {}", swing(&drums, kick));
        assert!(swing(&drums, kick) >= swing(&all, kick), "more than in the mix");
        let chord = band(700.0);
        assert!(mean(&drums, chord) < mean(&all, chord) * 0.8, "the chord sinks: {} against {}", mean(&drums, chord), mean(&all, chord));
    }

    #[test]
    fn off_it_writes_nothing() {
        let scope = Scope::default();
        scope.write(&[1.0; 64], 2, 48_000);
        let mut out = [0.5; 16];
        scope.read(&mut out, false);
        assert!(out.iter().all(|s| *s == 0.0));
    }
}
