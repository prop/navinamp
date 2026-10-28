//! Producer-side rate/channel conversion: channel mix-down/up first (cheap), then
//! FFT resampling with [`rubato`] in fixed chunks.
//!
//! # Policy (documented at crate level too)
//!
//! The engine tries to open the output stream at the *source* sample rate so most
//! playback is bit-exact with zero resampling. When the device (or test sink)
//! cannot run at the source rate, the track is resampled on the **decode thread**
//! (never in the audio callback) by an FFT-based synchronous resampler
//! (`rubato::Fft`, `FixedSync::Both`, chunk hint 1024). Resampling adds a small
//! constant latency (`output_delay`, roughly one chunk) plus up to one chunk of
//! silence-padded tail at the very end of a track; the padded tail is silence and
//! the position counter clamps to the track duration, so neither is audible.
//!
//! Gapless transitions between tracks whose rates differ from the stream rate are
//! sample-continuous but not sample-exact (each track's length is rounded to a
//! resampler chunk boundary, ≤ ~20 ms).

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

const CHUNK_HINT: usize = 1024;

/// Channel layout conversion (1↔2 and generic N→M by simple mapping/averaging).
/// Returns a borrowed slice: either `input` itself (no conversion needed) or
/// `scratch`.
pub(crate) fn convert_channels<'a>(
    input: &'a [f32],
    in_ch: usize,
    out_ch: usize,
    scratch: &'a mut Vec<f32>,
) -> &'a [f32] {
    if in_ch == out_ch {
        return input;
    }
    scratch.clear();
    scratch.reserve(input.len() / in_ch * out_ch);
    match (in_ch, out_ch) {
        (1, 2) => {
            for frame in input.chunks_exact(in_ch) {
                scratch.push(frame[0]);
                scratch.push(frame[0]);
            }
        }
        (2, 1) => {
            for frame in input.chunks_exact(in_ch) {
                scratch.push(0.5 * (frame[0] + frame[1]));
            }
        }
        _ => {
            // Generic: map output channel i to input channel min(i, n-1).
            for frame in input.chunks_exact(in_ch) {
                for c in 0..out_ch {
                    scratch.push(frame[c.min(in_ch - 1)]);
                }
            }
        }
    }
    scratch
}

/// Streaming FFT resampler. Fed arbitrary-size blocks on the decode thread;
/// internally stages whole input chunks.
pub(crate) struct ResampleStage {
    res: Fft<f32>,
    channels: usize,
    chunk_in: usize,
    chunk_out: usize,
    /// Staged input samples awaiting a full chunk.
    pend: Vec<f32>,
    out_scratch: Vec<f32>,
    /// Silence (input frames) pushed through at flush time to expel the
    /// resampler's intrinsic delay.
    flush_frames: usize,
}

impl ResampleStage {
    pub(crate) fn new(in_rate: u32, out_rate: u32, channels: usize) -> Result<Self, String> {
        let res = Fft::<f32>::new(
            in_rate as usize,
            out_rate as usize,
            CHUNK_HINT,
            channels,
            FixedSync::Both,
        )
        .map_err(|e| format!("resampler init failed: {e}"))?;
        let chunk_in = res.input_frames_next();
        let chunk_out = res.output_frames_next();
        let delay_out = res.output_delay();
        // Enough input silence to push the delayed tail out, plus a margin chunk.
        let flush_frames = (delay_out as f64 * f64::from(in_rate) / f64::from(out_rate)).ceil()
            as usize
            + chunk_in;
        Ok(Self {
            res,
            channels,
            chunk_in,
            chunk_out,
            pend: Vec::with_capacity(chunk_in * channels),
            out_scratch: vec![0.0; chunk_out * channels],
            flush_frames,
        })
    }

    /// Push interleaved input frames; appends resampled output to `out`.
    pub(crate) fn feed(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), String> {
        self.pend.extend_from_slice(input);
        let chunk_bytes = self.chunk_in * self.channels;
        while self.pend.len() >= chunk_bytes {
            self.process_chunk(Some(chunk_bytes), out)?;
            self.pend.drain(..chunk_bytes);
        }
        Ok(())
    }

    /// Flush the tail at end-of-track: pushes delay-compensation silence, then
    /// processes any partial chunk padded with silence. The padding appears as
    /// (harmless) trailing silence.
    pub(crate) fn flush(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        let silence = vec![0.0; self.flush_frames * self.channels];
        self.feed(&silence, out)?;
        let remaining = self.pend.len();
        if remaining > 0 {
            self.process_chunk(Some(remaining), out)?;
            self.pend.clear();
        }
        Ok(())
    }

    /// Process one chunk from `pend`. When `len < chunk_in*channels` the input is
    /// padded with silence via `Indexing::partial_len`.
    fn process_chunk(&mut self, len: Option<usize>, out: &mut Vec<f32>) -> Result<(), String> {
        let frames = len.map_or(self.chunk_in, |l| l / self.channels);
        let indexing = if len.is_some_and(|l| l < self.chunk_in * self.channels) {
            Some(Indexing::new().partial_len(frames))
        } else {
            None
        };
        let src = match len {
            Some(l) => &self.pend[..l],
            None => &self.pend[..self.chunk_in * self.channels],
        };
        let input = InterleavedSlice::new(src, self.channels, frames)
            .map_err(|e| format!("resampler input adapter: {e}"))?;
        let mut output =
            InterleavedSlice::new_mut(&mut self.out_scratch, self.channels, self.chunk_out)
                .map_err(|e| format!("resampler output adapter: {e}"))?;
        let (_, written) = self
            .res
            .process_into_buffer(&input, &mut output, indexing.as_ref())
            .map_err(|e| format!("resampler process: {e}"))?;
        out.extend_from_slice(&self.out_scratch[..written * self.channels]);
        Ok(())
    }
}

/// The producer's per-track conversion stage: identity when rates match, otherwise
/// a [`ResampleStage`].
pub(crate) enum ConvertStage {
    Identity,
    Resample(Box<ResampleStage>),
}

impl ConvertStage {
    pub(crate) fn new(in_rate: u32, out_rate: u32, channels: usize) -> Result<Self, String> {
        if in_rate == out_rate {
            Ok(Self::Identity)
        } else {
            Ok(Self::Resample(Box::new(ResampleStage::new(
                in_rate, out_rate, channels,
            )?)))
        }
    }

    pub(crate) fn feed(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), String> {
        match self {
            ConvertStage::Identity => {
                out.extend_from_slice(input);
                Ok(())
            }
            ConvertStage::Resample(r) => r.feed(input, out),
        }
    }

    pub(crate) fn flush(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        match self {
            ConvertStage::Identity => Ok(()),
            ConvertStage::Resample(r) => r.flush(out),
        }
    }
}
