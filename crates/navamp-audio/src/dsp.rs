//! DSP chain — **replaygain → preamp → 10× biquad EQ** — and the volume
//! de-zipper.
//!
//! # Position in the engine
//!
//! The plan (§2) chains the processors as *replay gain → preamp → 10×
//! biquad EQ → volume*. That chain runs in the output callback **after** the
//! ring buffer and **before** the volume ramp, i.e. exactly where
//! [`DspChain::process`] is called in [`crate::callback::Callback::pull`].
//! The analyser tap (`spectrum.rs`) taps the signal right after this chain,
//! pre-volume.
//!
//! # Real-time contract
//!
//! Everything here runs on the audio interrupt:
//! * no allocation, no locks, no blocking, no unbounded loops;
//! * biquad state lives in fixed buffers allocated at construction (one
//!   state per channel per band);
//! * EQ parameters arrive lock-free via [`crate::eq::EqParams`]: the chain
//!   snapshots its generation counter each block and re-reads values (and
//!   recomputes all coefficients — ~10 biquads × a few dozen flops, fine
//!   inline) **only when the generation changed**;
//! * ReplayGain arrives as frame-indexed switch markers ([`RgMarkers`]) on
//!   the engine's played-frames timeline, so gain changes land at the exact
//!   sample even across gapless track boundaries.
//!
//! # Bit-transparency
//!
//! When the EQ is inactive (disabled, or enabled and flat) **and** the
//! current ReplayGain is exactly 1.0, `process` touches nothing — output is
//! bit-identical to the decoder's f32. Running audio through identity
//! biquads would only accumulate float error, so flat means bypass.

use std::sync::Arc;

use crate::eq::{EQ_BAND_COUNT, EQ_BAND_FREQUENCIES_HZ, EQ_Q, EqParams};
use crate::shared::RgMarkers;

/// One peaking-EQ biquad section (RBJ cookbook) in transposed direct form
/// II, with a private state pair per channel.
struct EqBand {
    /// Normalised coefficients (a0 == 1).
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    /// Band gain in dB (to skip recomputing identical coefficients).
    db: f32,
    /// One filter state per output channel.
    states: Box<[BiquadState]>,
}

#[derive(Clone, Copy, Default)]
struct BiquadState {
    z1: f32,
    z2: f32,
}

impl EqBand {
    fn new(channels: usize) -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            db: 0.0,
            states: vec![BiquadState::default(); channels].into_boxed_slice(),
        }
    }

    /// Recompute peaking-EQ coefficients for `db` at this band's centre
    /// frequency. Filter states are deliberately kept — coefficient tweaks
    /// (slider moves) then glide without a state reset click.
    fn set_peaking(&mut self, db: f32, f0: f32, sample_rate: u32, q: f32) {
        self.db = db;
        if db == 0.0 {
            // Flat band: neutral coefficients (the whole stage is bypassed
            // anyway when every band is flat — this is just belt and braces).
            self.b0 = 1.0;
            self.b1 = 0.0;
            self.b2 = 0.0;
            self.a1 = 0.0;
            self.a2 = 0.0;
            return;
        }
        // RBJ audio-EQ-cookbook peaking filter.
        let a = 10f32.powf(db / 40.0);
        let w0 = std::f32::consts::TAU * f0 / sample_rate as f32;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = b1;
        let a2 = 1.0 - alpha / a;
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// One sample through the section (transposed direct form II).
    #[inline(always)]
    fn process(&mut self, x: f32, channel: usize) -> f32 {
        let state = &mut self.states[channel];
        let y = self.b0 * x + state.z1;
        state.z1 = self.b1 * x - self.a1 * y + state.z2;
        state.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Real-time DSP chain: ReplayGain scalar → preamp → 10-band EQ. All state
/// is fixed-allocation; see the module docs.
pub(crate) struct DspChain {
    channels: usize,
    sample_rate: u32,
    eq: Arc<EqParams>,
    /// Generation at the last snapshot; starts at `u64::MAX` so the very
    /// first block always takes a snapshot.
    eq_generation: u64,
    eq_active: bool,
    preamp_gain: f32,
    bands: Box<[EqBand]>,
    /// Current ReplayGain scalar (starts at unity: no track, no gain).
    rg_gain: f32,
    /// Next unconsumed switch marker, if one was taken.
    rg_pending: Option<(u64, f32)>,
    /// Private marker-ring cursor (consumer-owned).
    rg_read: u64,
}

impl DspChain {
    pub(crate) fn new(channels: usize, sample_rate: u32, eq: Arc<EqParams>) -> Self {
        let channels = channels.max(1);
        let bands: [EqBand; EQ_BAND_COUNT] = std::array::from_fn(|b| {
            let mut band = EqBand::new(channels);
            band.set_peaking(0.0, EQ_BAND_FREQUENCIES_HZ[b], sample_rate, EQ_Q);
            band
        });
        Self {
            channels,
            sample_rate,
            bands: Box::from(bands),
            eq,
            eq_generation: u64::MAX,
            eq_active: false,
            preamp_gain: 1.0,
            rg_gain: 1.0,
            rg_pending: None,
            rg_read: 0,
        }
    }

    /// Re-read the EQ parameters (only called when the generation moved)
    /// and recompute what the chain needs: bypass decision, preamp scalar
    /// and all ten coefficient sets.
    fn refresh_eq(&mut self) {
        let snapshot = self.eq.snapshot();
        self.eq_active = snapshot.active();
        self.preamp_gain = db_to_gain(snapshot.preamp_db);
        let rate = self.sample_rate;
        for (index, band) in self.bands.iter_mut().enumerate() {
            let db = snapshot.bands_db[index];
            if db != band.db {
                band.set_peaking(db, EQ_BAND_FREQUENCIES_HZ[index], rate, EQ_Q);
            }
        }
    }
}

impl DspChain {
    /// Process a block of interleaved samples in place. RT-safe.
    ///
    /// `first_frame` is the absolute (engine-timeline) index of the block's
    /// first frame — the same coordinate ReplayGain markers use. `markers`
    /// is the shared marker ring; switches due inside this block are
    /// adopted at their exact frame.
    #[inline]
    pub(crate) fn process(&mut self, buf: &mut [f32], first_frame: u64, markers: &RgMarkers) {
        // EQ snapshot: one Acquire load per block; full re-read + coefficient
        // recompute only when the generation counter moved.
        let generation = self.eq.generation();
        if generation != self.eq_generation {
            self.refresh_eq();
            self.eq_generation = generation;
        }

        let block_frames = buf.len() / self.channels;
        let last_frame = first_frame + block_frames as u64;

        // Hold the next unconsumed marker, then adopt every marker that is
        // due by the start of the block (they share one gain from here on).
        if self.rg_pending.is_none() {
            self.rg_pending = markers.take(&mut self.rg_read);
        }
        while let Some((frame, gain)) = self.rg_pending
            && frame <= first_frame
        {
            self.rg_gain = gain;
            self.rg_pending = markers.take(&mut self.rg_read);
        }

        // Bit-transparent fast path: no EQ, unity gain, and no switch due
        // anywhere inside this block.
        if !self.eq_active
            && self.rg_gain == 1.0
            && self.rg_pending.is_none_or(|(frame, _)| frame > last_frame)
        {
            return;
        }

        let rg = self.rg_gain;
        let preamp = self.preamp_gain;
        let scale = rg * preamp;
        for (i, frame) in buf.chunks_exact_mut(self.channels).enumerate() {
            let absolute = first_frame + i as u64;
            // A switch due mid-block takes effect at exactly its frame.
            while let Some((switch_frame, gain)) = self.rg_pending
                && switch_frame <= absolute
            {
                self.rg_gain = gain;
                self.rg_pending = markers.take(&mut self.rg_read);
            }
            let scale = if self.rg_gain == rg {
                scale
            } else {
                self.rg_gain * preamp
            };

            if !self.eq_active {
                if scale != 1.0 {
                    for s in frame.iter_mut() {
                        *s *= scale;
                    }
                }
            } else {
                for s in frame.iter_mut() {
                    *s *= scale;
                }
                for band in self.bands.iter_mut() {
                    for (channel, s) in frame.iter_mut().enumerate() {
                        *s = band.process(*s, channel);
                    }
                }
            }
        }
    }
}

/// dB → linear amplitude.
fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Duration of a full-scale volume sweep, in seconds. 30 ms is fast enough to feel
/// instantaneous and slow enough to be inaudible (classic de-zipper constant).
const RAMP_SECS: f32 = 0.03;

/// Per-sample linear volume ramp. Prevents clicks and zipper noise when the target
/// volume changes. Owned by the output callback; the target arrives as an atomic
/// (bit-cast f32), so no locking is needed.
pub(crate) struct VolumeRamp {
    current: f32,
    /// Maximum per-sample change of the gain.
    step: f32,
}

impl VolumeRamp {
    pub(crate) fn new(sample_rate: u32, initial: f32) -> Self {
        Self {
            current: initial,
            step: 1.0 / (sample_rate as f32 * RAMP_SECS),
        }
    }

    /// Apply the current gain to `buf`, gliding toward `target`.
    #[inline]
    pub(crate) fn apply(&mut self, buf: &mut [f32], target: f32) {
        for s in buf.iter_mut() {
            if self.current < target {
                self.current = (self.current + self.step).min(target);
            } else if self.current > target {
                self.current = (self.current - self.step).max(target);
            }
            *s *= self.current;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;

    const RATE: u32 = 44_100;

    fn chain() -> (DspChain, Arc<EqParams>, RgMarkers) {
        let eq = Arc::new(EqParams::new());
        let markers = RgMarkers::new();
        (DspChain::new(2, RATE, eq.clone()), eq, markers)
    }

    /// Deterministic test signal (not silence, includes a negative zero).
    fn signal(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let v = ((n as f64 / 97.0) * PI).sin() as f32;
                [v, if n % 13 == 0 { -0.0 } else { v }]
            })
            .collect()
    }

    #[test]
    fn flat_and_disabled_is_bit_exact_passthrough() {
        let (mut dsp, _eq, markers) = chain();
        let mut buf = signal(4096);
        let before = buf.clone();
        dsp.process(&mut buf, 0, &markers);
        dsp.process(&mut buf, 4096, &markers);
        assert_eq!(buf, before, "flat+disabled must not touch a single bit");
    }

    #[test]
    fn flat_but_enabled_is_still_bit_exact() {
        let (mut dsp, eq, markers) = chain();
        eq.set_enabled(true); // enabled, but every band + preamp at 0 dB
        let mut buf = signal(2048);
        let before = buf.clone();
        dsp.process(&mut buf, 0, &markers);
        assert_eq!(buf, before, "flat curve must bypass, enabled or not");
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn plus_12db_band_gains_center_frequency() {
        let (mut dsp, eq, markers) = chain();
        eq.set_enabled(true);
        eq.set_band_db(4, 12.0); // 1 kHz band
        eq.set_band_db(4, 12.0); // idempotent set, exercises generation skip

        // 1 kHz sine, well below clipping even boosted (×3.98).
        let frames = RATE as usize / 2;
        let input: Vec<f32> = (0..frames)
            .map(|n| (2.0 * PI * 1000.0 * n as f64 / f64::from(RATE)).sin() as f32 * 0.1)
            .collect();
        let mut stereo: Vec<f32> = input.iter().flat_map(|&s| [s, s]).collect();
        let block = 512 * 2;
        for (n, chunk) in stereo.chunks_mut(block).enumerate() {
            dsp.process(chunk, (n * block / 2) as u64, &markers);
        }

        // Steady state: skip the first quarter second (filter transient).
        let settle = (RATE as usize / 4) * 2;
        let out_l: Vec<f32> = stereo[settle..].iter().step_by(2).copied().collect();
        let in_l: Vec<f32> = input[settle / 2..].to_vec();
        let ratio_db = 20.0 * (rms(&out_l) / rms(&in_l)).log10();
        assert!(
            (ratio_db - 12.0).abs() <= 1.0,
            "1 kHz gain {ratio_db:.2} dB (expected ≈ +12)"
        );
    }

    #[test]
    fn preamp_attenuates_when_bands_flat() {
        let (mut dsp, eq, markers) = chain();
        eq.set_enabled(true);
        eq.set_preamp_db(-6.0);
        let input = signal(2048);
        let mut buf = input.clone();
        dsp.process(&mut buf, 0, &markers);
        let expected: Vec<f32> = input.iter().map(|&s| s * db_to_gain(-6.0)).collect();
        let max_err = buf
            .iter()
            .zip(&expected)
            .map(|(a, e)| (a - e).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 1e-6, "preamp error {max_err}");
    }

    #[test]
    fn param_changes_midstream_never_panic_or_blow_up() {
        let (mut dsp, eq, markers) = chain();
        let mut frame_no = 0u64;
        for step in 0..64 {
            // Hammer every parameter shape between blocks.
            if step % 2 == 0 {
                eq.set_enabled(step % 4 == 0);
            }
            eq.set_band_db((step as usize) % EQ_BAND_COUNT, step as f32 * 0.4 - 12.0);
            eq.set_preamp_db((step % 17) as f32 - 8.0);
            let mut buf = signal(256);
            dsp.process(&mut buf, frame_no, &markers);
            assert!(
                buf.iter().all(|s| s.is_finite()),
                "non-finite sample at step {step}"
            );
            frame_no += 256;
        }
    }

    #[test]
    fn enabled_toggle_switches_between_filtered_and_passthrough() {
        let (mut dsp, eq, markers) = chain();
        eq.set_band_db(4, 9.0);

        // Disabled: bit-exact.
        let input = signal(1024);
        let mut buf = input.clone();
        dsp.process(&mut buf, 0, &markers);
        assert_eq!(buf, input);

        // Enabled: filtered (differs, stays finite, no explosion).
        eq.set_enabled(true);
        let mut buf2 = input.clone();
        dsp.process(&mut buf2, 1024, &markers);
        assert!(buf2.iter().all(|s| s.is_finite()));
        assert_ne!(buf2, input, "enabled + boosted band must filter");

        // Disabled again: passthrough resumes exactly.
        eq.set_enabled(false);
        let mut buf3 = input.clone();
        dsp.process(&mut buf3, 2048, &markers);
        assert_eq!(buf3, input);
    }

    #[test]
    fn replaygain_marker_switches_at_exact_frame() {
        let (mut dsp, _eq, markers) = chain();
        markers.push(1500, 0.5); // switch mid-block (block = frames 0..2048)

        let input = signal(2048);
        let mut buf = input.clone();
        dsp.process(&mut buf, 0, &markers);
        for frame in 0..2048 {
            let (l, r) = (buf[frame * 2], buf[frame * 2 + 1]);
            let (el, er) = (input[frame * 2], input[frame * 2 + 1]);
            if (frame as u64) < 1500 {
                assert_eq!((l, r), (el, er), "frame {frame} must be untouched");
            } else {
                assert_eq!(l, el * 0.5, "frame {frame} must be scaled");
                assert_eq!(r, er * 0.5, "frame {frame} must be scaled");
            }
        }
    }

    #[test]
    fn unity_replaygain_is_skipped_entirely() {
        let (mut dsp, _eq, markers) = chain();
        markers.push(0, 1.0); // a unity marker is still exact passthrough
        let mut buf = signal(1024);
        let before = buf.clone();
        dsp.process(&mut buf, 0, &markers);
        assert_eq!(buf, before);
    }
}
