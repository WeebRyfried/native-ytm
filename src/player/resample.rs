//! Sample-rate conversion from a track's rate to the device's, with the
//! resampler's delay trimmed so a track starts and ends where it should.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

const CHUNK: usize = 1024;

pub struct Converter {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    fft: Option<Fft<f32>>,
    pending: Vec<f32>,
    out: Vec<f32>,
    /// Output frames still to drop: the resampler's start-up delay.
    delay_left: usize,
    in_total: u64,
    out_total: u64,
}

impl Converter {
    pub fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        let fft = (in_rate != out_rate)
            .then(|| {
                Fft::<f32>::new(
                    in_rate as usize,
                    out_rate as usize,
                    CHUNK,
                    channels,
                    FixedSync::Input,
                )
                .map_err(|error| log::warn!("no resampler {in_rate}->{out_rate}: {error}"))
                .ok()
            })
            .flatten();
        let mut converter = Self {
            in_rate,
            out_rate,
            channels,
            out: Vec::new(),
            pending: Vec::new(),
            delay_left: 0,
            in_total: 0,
            out_total: 0,
            fft,
        };
        converter.reset();
        converter
    }

    pub fn matches(&self, in_rate: u32, out_rate: u32, channels: usize) -> bool {
        self.in_rate == in_rate && self.out_rate == out_rate && self.channels == channels
    }

    /// Forgets everything in flight, as after a seek.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.in_total = 0;
        self.out_total = 0;
        if let Some(fft) = &mut self.fft {
            fft.reset();
            self.delay_left = fft.output_delay();
            self.out = vec![0.0; fft.output_frames_max() * self.channels];
        }
    }

    /// Converts interleaved `input` and hands each converted block to `emit`.
    pub fn process(&mut self, input: &[f32], emit: &mut dyn FnMut(&[f32])) {
        self.in_total += (input.len() / self.channels) as u64;
        let Some(fft) = &mut self.fft else {
            self.out_total += (input.len() / self.channels) as u64;
            emit(input);
            return;
        };
        self.pending.extend_from_slice(input);
        loop {
            let need = fft.input_frames_next();
            if self.pending.len() < need * self.channels {
                break;
            }
            let produced = run(fft, &self.pending, need, None, &mut self.out, self.channels);
            self.pending.drain(..need * self.channels);
            Self::emit_trimmed(
                &self.out[..produced * self.channels],
                self.channels,
                &mut self.delay_left,
                &mut self.out_total,
                u64::MAX,
                emit,
            );
        }
    }

    /// Flushes what the resampler still holds at the end of a track, so the
    /// output is exactly as long as the input at the new rate.
    pub fn finish(&mut self, emit: &mut dyn FnMut(&[f32])) {
        let Some(fft) = &mut self.fft else {
            return;
        };
        let expected = self.in_total * u64::from(self.out_rate) / u64::from(self.in_rate);
        let mut rounds = 0;
        while self.out_total < expected && rounds < 8 {
            rounds += 1;
            let need = fft.input_frames_next();
            let have = (self.pending.len() / self.channels).min(need);
            let mut chunk = vec![0.0; need * self.channels];
            chunk[..have * self.channels].copy_from_slice(&self.pending[..have * self.channels]);
            self.pending.drain(..have * self.channels);
            let produced = run(fft, &chunk, need, Some(have), &mut self.out, self.channels);
            Self::emit_trimmed(
                &self.out[..produced * self.channels],
                self.channels,
                &mut self.delay_left,
                &mut self.out_total,
                expected,
                emit,
            );
        }
        self.pending.clear();
    }

    fn emit_trimmed(
        block: &[f32],
        channels: usize,
        delay_left: &mut usize,
        out_total: &mut u64,
        limit: u64,
        emit: &mut dyn FnMut(&[f32]),
    ) {
        let frames = block.len() / channels;
        let skip = (*delay_left).min(frames);
        *delay_left -= skip;
        let mut keep = frames - skip;
        let room = limit.saturating_sub(*out_total);
        keep = keep.min(usize::try_from(room).unwrap_or(usize::MAX));
        if keep > 0 {
            *out_total += keep as u64;
            emit(&block[skip * channels..(skip + keep) * channels]);
        }
    }
}

fn run(
    fft: &mut Fft<f32>,
    input: &[f32],
    frames: usize,
    partial: Option<usize>,
    out: &mut [f32],
    channels: usize,
) -> usize {
    let Ok(input) = InterleavedSlice::new(&input[..frames * channels], channels, frames) else {
        return 0;
    };
    let capacity = out.len() / channels;
    let Ok(mut output) = InterleavedSlice::new_mut(out, channels, capacity) else {
        return 0;
    };
    let indexing = Indexing {
        input_offset: 0,
        output_offset: 0,
        partial_len: partial,
        active_channels_mask: None,
    };
    match fft.process_into_buffer(&input, &mut output, Some(&indexing)) {
        Ok((_, produced)) => produced,
        Err(error) => {
            log::warn!("resampling failed: {error}");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_length_follows_the_ratio() {
        let mut converter = Converter::new(44_100, 48_000, 2);
        let input = vec![0.25f32; 44_100 * 2];
        let mut frames = 0;
        let mut count = |block: &[f32]| frames += block.len() / 2;
        for chunk in input.chunks(2 * 1000) {
            converter.process(chunk, &mut count);
        }
        converter.finish(&mut count);
        assert_eq!(frames, 48_000);
    }

    #[test]
    fn same_rate_passes_through() {
        let mut converter = Converter::new(48_000, 48_000, 2);
        let mut seen = Vec::new();
        converter.process(&[0.1, 0.2, 0.3, 0.4], &mut |block| seen.extend_from_slice(block));
        assert_eq!(seen, vec![0.1, 0.2, 0.3, 0.4]);
    }
}
