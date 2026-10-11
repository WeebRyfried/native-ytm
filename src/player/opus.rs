//! Opus, the codec of YouTube's WebM audio (itag 251, and 774 for Premium),
//! decoded by libopus itself: `unsafe-libopus` is its C translated to Rust,
//! so the build needs no C toolchain. Symphonia demuxes the WebM and hands
//! the packets here.

use symphonia::core::codecs::audio::AudioCodecParameters;

/// Opus always decodes at 48 kHz.
pub const RATE: u32 = 48_000;

/// The longest packet Opus allows: 120 ms.
const MAX_FRAMES: usize = 5760;

pub struct Opus {
    state: *mut unsafe_libopus::OpusDecoder,
    pub channels: usize,
    /// The encoder's lead-in at the start of the stream, in frames.
    pub pre_skip: u64,
    gain: f32,
    pcm: Vec<f32>,
}

// SAFETY: the decoder state belongs to this value alone, which the engine
// thread uses; nothing else holds the pointer.
unsafe impl Send for Opus {}

impl Opus {
    pub fn new(params: &AudioCodecParameters) -> Result<Self, String> {
        // The identification header (RFC 7845, section 5.1), which WebM
        // keeps as the track's codec private data.
        let head = params
            .extra_data
            .as_deref()
            .filter(|h| h.len() >= 19 && h.starts_with(b"OpusHead"));
        let (channels, pre_skip, gain_q8, mapping) = match head {
            Some(h) => (
                usize::from(h[9]),
                u64::from(u16::from_le_bytes([h[10], h[11]])),
                i16::from_le_bytes([h[16], h[17]]),
                h[18],
            ),
            None => (params.channels.as_ref().map_or(2, |c| c.count()), 312, 0, 0),
        };
        if mapping != 0 || !(1..=2).contains(&channels) {
            return Err(format!("opus: {channels} channels in mapping family {mapping} are not supported"));
        }
        let state = create(channels)?;
        Ok(Self {
            state,
            channels,
            pre_skip,
            gain: 10f32.powf(f32::from(gain_q8) / 256.0 / 20.0),
            pcm: vec![0.0; MAX_FRAMES * channels],
        })
    }

    /// Decodes one packet into `out`, interleaved.
    pub fn decode(&mut self, data: &[u8], out: &mut Vec<f32>) -> Result<(), String> {
        out.clear();
        if data.is_empty() {
            return Ok(());
        }
        let len = i32::try_from(data.len()).map_err(|_| "opus: packet too large".to_owned())?;
        // SAFETY: `state` is a live decoder; `data` is `len` bytes and `pcm`
        // holds MAX_FRAMES frames of `channels` samples, the most a packet
        // decodes to.
        let frames = unsafe {
            unsafe_libopus::opus_decode_float(
                self.state,
                data.as_ptr(),
                len,
                self.pcm.as_mut_ptr(),
                MAX_FRAMES as i32,
                0,
            )
        };
        if frames < 0 {
            return Err(format!("opus: decode error {frames}"));
        }
        let samples = frames as usize * self.channels;
        out.extend(self.pcm[..samples].iter().map(|s| s * self.gain));
        Ok(())
    }

    /// Forgets what came before, after a seek.
    pub fn reset(&mut self) {
        if let Ok(state) = create(self.channels) {
            // SAFETY: the old state came from `opus_decoder_create` and is
            // not used again.
            unsafe { unsafe_libopus::opus_decoder_destroy(self.state) };
            self.state = state;
        }
    }
}

impl Drop for Opus {
    fn drop(&mut self) {
        // SAFETY: created by `opus_decoder_create`, destroyed once.
        unsafe { unsafe_libopus::opus_decoder_destroy(self.state) };
    }
}

fn create(channels: usize) -> Result<*mut unsafe_libopus::OpusDecoder, String> {
    let mut error = 0;
    // SAFETY: valid rate and channel count; `error` outlives the call.
    let state = unsafe { unsafe_libopus::opus_decoder_create(RATE as i32, channels as i32, &mut error) };
    if state.is_null() || error != 0 {
        return Err(format!("opus: cannot make a decoder ({error})"));
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_silent_frame() {
        let mut params = AudioCodecParameters::new();
        params.with_channels(symphonia::core::audio::Channels::Discrete(2));
        let mut opus = Opus::new(&params).expect("decoder");
        // A 20 ms CELT-only packet of silence (TOC 0xFC, stereo, one frame,
        // then a zero byte that decodes as silence).
        let mut out = Vec::new();
        opus.decode(&[0xFC, 0xFF, 0xFE], &mut out).expect("decodes");
        assert_eq!(out.len(), 960 * 2);
    }
}
