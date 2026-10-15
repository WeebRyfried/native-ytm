//! The smart crossfade: where the song ending goes quiet and where the next
//! one gets going, found from their samples, so the two overlap there.
//!
//! The end of the song playing and the start of the next are looked at
//! together, as decoded output (the device's rate and channels):
//!
//! - the ending's silence (below [`SILENCE_DB`]) is cut, and so is the
//!   silence before the next song starts;
//! - the overlap covers the ending's quiet stretch (a fade-out, an outro
//!   [`QUIET_DB`] below its loudest), or the next song's soft intro, at
//!   least [`SHORTEST`] and at most the longest the person allows.

use std::time::Duration;

/// How much of each song's end and start is looked at.
pub const SCAN: Duration = Duration::from_secs(15);

/// Below this, silence (dB of full scale).
const SILENCE_DB: f32 = -50.0;

/// Quieter than the loudest of what is looked at by this much: a fade-out,
/// or a soft intro.
const QUIET_DB: f32 = 15.0;

/// The shortest overlap, for songs that end on a full stop.
const SHORTEST: Duration = Duration::from_millis(1500);

/// The levels are measured over windows this long...
const WINDOW: Duration = Duration::from_millis(20);

/// ...and smoothed over this many, so a beat's gap is not a fade.
const SMOOTH: usize = 10;

/// How the two songs meet, in frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Of the ending's tail: heard alone, before the overlap.
    pub solo: usize,
    /// Heard together, the ending fading out as the next fades in.
    pub overlap: usize,
    /// Of the next song's start: silence, skipped.
    pub skip: usize,
}

impl Plan {
    /// The ending's tail cut at its end: what is neither heard alone nor
    /// in the overlap.
    pub fn cut(&self, tail_frames: usize) -> usize {
        tail_frames.saturating_sub(self.solo + self.overlap)
    }
}

/// Where `tail` (the last of the song ending) and `head` (the first of the
/// next) should meet, overlapping for no longer than `longest`.
pub fn plan(tail: &[f32], head: &[f32], channels: usize, rate: u32, longest: Duration) -> Plan {
    let channels = channels.max(1);
    let window = ((WINDOW.as_secs_f64() * f64::from(rate)).round() as usize).max(1);
    let frames = |d: Duration| (d.as_secs_f64() * f64::from(rate)).round() as usize;
    let tail_frames = tail.len() / channels;
    let head_frames = head.len() / channels;

    // The ending: where it last makes a sound, and where its quiet stretch
    // up to there starts.
    let levels = powers(tail, channels, window);
    let end_w = levels.iter().rposition(|p| db(*p) > SILENCE_DB).map_or(0, |i| i + 1);
    let end = (end_w * window).min(tail_frames);
    let smooth = smoothed(&levels[..end_w]);
    let loudest = smooth.iter().copied().fold(0.0f32, f32::max);
    let quiet_from = if db(loudest) <= SILENCE_DB {
        0
    } else {
        let quiet = db(loudest) - QUIET_DB;
        let run = smooth.iter().rev().take_while(|p| db(**p) < quiet).count();
        (end_w - run) * window
    };
    let fading = end.saturating_sub(quiet_from);

    // The next song: where it first makes a sound, and where it gets going.
    let levels = powers(head, channels, window);
    let start_w = levels.iter().position(|p| db(*p) > SILENCE_DB).unwrap_or(levels.len());
    let skip = (start_w * window).min(head_frames);
    let smooth = smoothed(&levels[start_w..]);
    let loudest = smooth.iter().copied().fold(0.0f32, f32::max);
    let intro = if db(loudest) <= SILENCE_DB {
        0
    } else {
        let going = db(loudest) - QUIET_DB;
        smooth.iter().position(|p| db(*p) >= going).unwrap_or(0) * window
    };

    let longest = frames(longest);
    let overlap = fading.max(intro).clamp(frames(SHORTEST).min(longest), longest).min(end);
    Plan { solo: end - overlap, overlap, skip }
}

/// The mean power of each window, over every channel.
fn powers(samples: &[f32], channels: usize, window: usize) -> Vec<f32> {
    samples
        .chunks(window * channels)
        .map(|chunk| chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len().max(1) as f32)
        .collect()
}

/// Each window's power, averaged with its neighbours'.
fn smoothed(powers: &[f32]) -> Vec<f32> {
    let half = SMOOTH / 2;
    (0..powers.len())
        .map(|i| {
            let around = &powers[i.saturating_sub(half)..(i + half + 1).min(powers.len())];
            around.iter().sum::<f32>() / around.len() as f32
        })
        .collect()
}

fn db(power: f32) -> f32 {
    10.0 * power.max(1e-12).log10()
}

/// The overlap itself: the ending fading out as the next fades in, at
/// equal power.
pub fn mix(ending: &[f32], next: &[f32], channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    let frames = ending.len() / channels;
    let mut out = Vec::with_capacity(frames * channels);
    for f in 0..frames {
        let t = (f as f32 + 0.5) / frames as f32 * std::f32::consts::FRAC_PI_2;
        let (out_gain, in_gain) = (t.cos(), t.sin());
        for c in 0..channels {
            let i = f * channels + c;
            out.push(ending[i] * out_gain + next.get(i).copied().unwrap_or(0.0) * in_gain);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1_000;

    /// `secs` of a tone at `level` (0 to 1), stereo.
    fn tone(secs: f32, level: impl Fn(f32) -> f32) -> Vec<f32> {
        let frames = (secs * RATE as f32) as usize;
        (0..frames)
            .flat_map(|i| {
                let t = i as f32 / RATE as f32;
                let s = level(t) * (t * 110.0 * std::f32::consts::TAU).sin();
                [s, s]
            })
            .collect()
    }

    fn silence(secs: f32) -> Vec<f32> {
        tone(secs, |_| 0.0)
    }

    fn secs(frames: usize) -> f32 {
        frames as f32 / RATE as f32
    }

    #[test]
    fn overlaps_a_fade_out_and_cuts_the_silences() {
        // Loud for 8 s, a 4 s fade, then 3 s of silence.
        let mut tail = tone(8.0, |_| 0.5);
        tail.extend(tone(4.0, |t| 0.5 * (1.0 - t / 4.0).powi(3)));
        tail.extend(silence(3.0));
        // Half a second of silence, then straight in.
        let mut head = silence(0.5);
        head.extend(tone(10.0, |_| 0.5));
        let plan = plan(&tail, &head, 2, RATE, Duration::from_secs(8));
        // The fade, from where it is well down, to where it is gone.
        assert!((1.0..3.5).contains(&secs(plan.overlap)), "{plan:?}");
        assert!((secs(plan.solo + plan.overlap) - 11.5).abs() < 0.6, "{plan:?}");
        assert!((secs(plan.cut(tail.len() / 2)) - 3.5).abs() < 0.6, "{plan:?}");
        assert!((secs(plan.skip) - 0.5).abs() < 0.05, "{plan:?}");
    }

    #[test]
    fn a_full_stop_still_overlaps_a_little() {
        let tail = tone(15.0, |_| 0.5);
        let head = tone(15.0, |_| 0.5);
        let plan = plan(&tail, &head, 2, RATE, Duration::from_secs(6));
        assert_eq!(plan.overlap, 1_500);
        assert_eq!(plan.solo + plan.overlap, 15_000);
        assert_eq!(plan.skip, 0);
    }

    #[test]
    fn a_soft_intro_comes_in_under_the_ending() {
        let tail = tone(15.0, |_| 0.5);
        // Four soft seconds before the song gets going.
        let mut head = tone(4.0, |_| 0.03);
        head.extend(tone(10.0, |_| 0.5));
        let plan = plan(&tail, &head, 2, RATE, Duration::from_secs(12));
        assert!((secs(plan.overlap) - 4.0).abs() < 0.3, "{plan:?}");
    }

    #[test]
    fn never_longer_than_allowed() {
        let mut tail = tone(2.0, |_| 0.5);
        // A long quiet outro.
        tail.extend(tone(13.0, |_| 0.04));
        let head = tone(15.0, |_| 0.5);
        let plan = plan(&tail, &head, 2, RATE, Duration::from_secs(5));
        assert_eq!(plan.overlap, 5_000);
    }

    #[test]
    fn silence_is_cut() {
        // Five seconds of song, then ten of silence: the silence goes.
        let mut tail = tone(5.0, |_| 0.5);
        tail.extend(silence(10.0));
        let plan = plan(&tail, &tone(15.0, |_| 0.5), 2, RATE, Duration::from_secs(6));
        assert_eq!(plan.solo + plan.overlap, 5_000);
        assert_eq!(plan.cut(15_000), 10_000);
        // Nothing but silence: nothing to overlap.
        let plan = super::plan(&silence(15.0), &tone(15.0, |_| 0.5), 2, RATE, Duration::from_secs(6));
        assert_eq!((plan.solo, plan.overlap), (0, 0));
    }
    #[test]
    fn the_mix_keeps_its_power() {
        let a = vec![1.0f32; 200];
        let b = vec![1.0f32; 200];
        let mixed = mix(&a, &b, 2);
        assert_eq!(mixed.len(), 200);
        assert!(mixed[0] > 0.99 && mixed[0] < 1.05);
        assert!(mixed[199] > 0.99 && mixed[199] < 1.05);
        // Halfway, cos + sin of 45 degrees.
        assert!((mixed[100] - std::f32::consts::SQRT_2).abs() < 0.02);
    }
}
