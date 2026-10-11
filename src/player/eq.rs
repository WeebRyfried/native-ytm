//! A ten-band equaliser, in the audio callback: a change is heard at once,
//! not after the seconds already decoded. Bands left at 0 dB cost nothing,
//! and with the equaliser off nothing runs at all.
//!
//! Each band is a peaking filter (RBJ's audio EQ cookbook) an octave wide.
//! When bands are boosted, the whole is turned down by most of the largest
//! boost, so the loudest passages do not clip.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// The bands' centres, in hertz.
pub const BANDS: [f32; 10] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];

/// How far a band goes, either way, in dB.
pub const RANGE: f32 = 12.0;

/// Ready-made curves, in dB per band.
pub const PRESETS: [(&str, [f32; 10]); 10] = [
    ("Flat", [0.0; 10]),
    ("Bass boost", [6.0, 5.0, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Bass reducer", [-6.0, -5.0, -4.0, -2.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble boost", [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 2.0, 3.5, 5.0, 6.0]),
    ("Vocal", [-2.0, -2.0, -1.0, 1.0, 3.0, 3.5, 3.0, 1.5, 0.0, -1.0]),
    ("Rock", [4.5, 3.5, 2.0, 0.0, -1.0, -1.0, 0.5, 2.5, 3.5, 4.0]),
    ("Pop", [-1.0, 0.0, 2.0, 3.0, 3.5, 2.0, 0.5, -0.5, -1.0, -1.0]),
    ("Electronic", [4.5, 4.0, 1.5, 0.0, -2.0, 0.0, 1.0, 2.0, 4.0, 4.5]),
    ("Acoustic", [3.0, 3.0, 2.0, 1.0, 1.5, 1.5, 2.5, 3.0, 2.5, 1.5]),
    ("Late night", [3.5, 3.0, 1.0, 0.0, -1.0, -1.5, -1.0, 0.0, 2.0, 3.0]),
];

/// The settings, as the interface sets them and the callback reads them,
/// without a lock.
#[derive(Default)]
pub struct EqControl {
    on: AtomicBool,
    gains: [AtomicU32; 10],
    /// Bumped on every change, so the callback knows to recompute.
    generation: AtomicU64,
}

impl EqControl {
    pub fn set(&self, on: bool, gains: &[f32; 10]) {
        for (slot, gain) in self.gains.iter().zip(gains) {
            slot.store(gain.clamp(-RANGE, RANGE).to_bits(), Ordering::Relaxed);
        }
        self.on.store(on, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }
}

#[derive(Clone, Copy, Default)]
struct Coefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

/// The callback's side: the filters and what they remember.
#[derive(Default)]
pub struct Equalizer {
    generation: u64,
    on: bool,
    /// The bands in use, by index, and their coefficients.
    active: Vec<(usize, Coefficients)>,
    /// Two values per band per channel (transposed direct form II).
    state: Vec<[f64; 2]>,
    preamp: f32,
    rate: u32,
    channels: usize,
}

impl Equalizer {
    /// Filters interleaved `samples` in place.
    pub fn process(&mut self, control: &EqControl, samples: &mut [f32], rate: u32, channels: usize) {
        let generation = control.generation.load(Ordering::Acquire);
        if generation != self.generation || rate != self.rate || channels != self.channels {
            self.configure(control, generation, rate, channels);
        }
        if !self.on || self.active.is_empty() || channels == 0 {
            return;
        }
        let bands = BANDS.len();
        for frame in samples.chunks_exact_mut(channels) {
            for (c, sample) in frame.iter_mut().enumerate() {
                let mut x = f64::from(*sample * self.preamp);
                for (band, k) in &self.active {
                    let s = &mut self.state[c * bands + band];
                    let y = k.b0 * x + s[0];
                    s[0] = k.b1 * x - k.a1 * y + s[1];
                    s[1] = k.b2 * x - k.a2 * y;
                    x = y;
                }
                *sample = x as f32;
            }
        }
    }

    fn configure(&mut self, control: &EqControl, generation: u64, rate: u32, channels: usize) {
        let was_on = self.on;
        let reshaped = rate != self.rate || channels != self.channels;
        self.generation = generation;
        self.on = control.on.load(Ordering::Relaxed);
        self.rate = rate;
        self.channels = channels;
        let gains: Vec<f32> = control.gains.iter().map(|g| f32::from_bits(g.load(Ordering::Relaxed))).collect();
        let previous: Vec<usize> = self.active.iter().map(|(band, _)| *band).collect();
        self.active = gains
            .iter()
            .enumerate()
            .filter(|(band, gain)| gain.abs() >= 0.05 && BANDS[*band] < rate as f32 * 0.45)
            .map(|(band, gain)| (band, peaking(BANDS[band], *gain, rate)))
            .collect();
        if reshaped || !was_on || self.state.len() != BANDS.len() * channels {
            self.state = vec![[0.0; 2]; BANDS.len() * channels];
        } else {
            // A band coming in starts from rest.
            for (band, _) in &self.active {
                if !previous.contains(band) {
                    for c in 0..channels {
                        self.state[c * BANDS.len() + band] = [0.0; 2];
                    }
                }
            }
        }
        let boost = gains.iter().copied().fold(0.0f32, f32::max);
        self.preamp = 10f32.powf(-boost * 0.85 / 20.0);
    }
}

/// A peaking filter at `freq`, an octave wide, of `gain` dB.
fn peaking(freq: f32, gain: f32, rate: u32) -> Coefficients {
    let a = 10f64.powf(f64::from(gain) / 40.0);
    let w0 = std::f64::consts::TAU * f64::from(freq) / f64::from(rate.max(1));
    let q = std::f64::consts::SQRT_2;
    let alpha = w0.sin() / (2.0 * q);
    let cos = w0.cos();
    let a0 = 1.0 + alpha / a;
    Coefficients {
        b0: (1.0 + alpha * a) / a0,
        b1: (-2.0 * cos) / a0,
        b2: (1.0 - alpha * a) / a0,
        a1: (-2.0 * cos) / a0,
        a2: (1.0 - alpha / a) / a0,
    }
}

/// The response of the curve at `freq`, in dB, for drawing it.
pub fn response_db(gains: &[f32; 10], freq: f32, rate: u32) -> f32 {
    let w = std::f64::consts::TAU * f64::from(freq) / f64::from(rate);
    let (cos1, sin1, cos2, sin2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let mut db = 0.0;
    for (band, gain) in gains.iter().enumerate() {
        if gain.abs() < 0.05 {
            continue;
        }
        let k = peaking(BANDS[band], *gain, rate);
        let (nr, ni) = (k.b0 + k.b1 * cos1 + k.b2 * cos2, -(k.b1 * sin1 + k.b2 * sin2));
        let (dr, di) = (1.0 + k.a1 * cos1 + k.a2 * cos2, -(k.a1 * sin1 + k.a2 * sin2));
        db += 10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10();
    }
    db as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, rate: u32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .flat_map(|i| {
                let s = (i as f32 / rate as f32 * freq * std::f32::consts::TAU).sin() * 0.5;
                [s, s]
            })
            .collect()
    }

    fn level(samples: &[f32]) -> f32 {
        // The second half, once the filters have settled.
        let tail = &samples[samples.len() / 2..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt()
    }

    #[test]
    fn off_or_flat_leaves_the_sound_alone() {
        let control = EqControl::default();
        let mut eq = Equalizer::default();
        let original = tone(440.0, 48_000, 0.2);
        let mut samples = original.clone();
        eq.process(&control, &mut samples, 48_000, 2);
        assert_eq!(samples, original);
        control.set(true, &[0.0; 10]);
        eq.process(&control, &mut samples, 48_000, 2);
        assert_eq!(samples, original);
    }

    #[test]
    fn a_band_moves_its_own_frequencies() {
        let control = EqControl::default();
        let mut gains = [0.0; 10];
        gains[2] = -12.0;
        control.set(true, &gains);
        let mut eq = Equalizer::default();
        let mut low = tone(125.0, 48_000, 0.5);
        let before = level(&low);
        eq.process(&control, &mut low, 48_000, 2);
        let cut = 20.0 * (level(&low) / before).log10();
        assert!((cut + 12.0).abs() < 1.0, "125 Hz cut by {cut} dB");
        // Far away, it barely changes.
        let mut eq = Equalizer::default();
        let mut high = tone(4000.0, 48_000, 0.5);
        let before = level(&high);
        eq.process(&control, &mut high, 48_000, 2);
        let change = 20.0 * (level(&high) / before).log10();
        assert!(change.abs() < 0.5, "4 kHz changed by {change} dB");
        // And the drawn curve agrees.
        assert!((response_db(&gains, 125.0, 48_000) + 12.0).abs() < 0.2);
    }
}
