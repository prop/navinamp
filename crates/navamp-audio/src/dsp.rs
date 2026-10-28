//! DSP chain placeholder and volume de-zipper.
//!
//! # DSP insertion point (M6)
//!
//! The plan (§2) chains the processors as: *replay gain → preamp → 10× biquad EQ →
//! volume*. That entire chain runs in the output callback **after** the ring buffer
//! and **before** the volume ramp, i.e. exactly where [`DspChain::process`] is
//! called in [`crate::callback::Callback::pull`].
//!
//! Constraints for anything inserted here (real-time context):
//! - no allocation, no locks, no blocking, no unbounded loops;
//! - biquad state lives in fixed-capacity buffers allocated at construction;
//! - parameter changes arrive via a lock-free snapshot (e.g. an `ArcSwap`-style
//!   generation counter), published from the IPC side.
//!
//! For M1 the chain applies only the preamp, fixed at 1.0 (a no-op). ReplayGain
//! will slot in front of the preamp once tag scanning (lofty) lands.

/// Real-time DSP chain. Currently a documented placeholder (preamp = 1.0).
pub(crate) struct DspChain {
    preamp: f32,
    // M6: `bands: Vec<Biquad>` (allocated once at setup), ReplayGain gain scalar.
}

impl DspChain {
    pub(crate) fn new() -> Self {
        Self { preamp: 1.0 }
    }

    /// Process a block of interleaved samples in place. RT-safe.
    #[inline]
    pub(crate) fn process(&mut self, buf: &mut [f32]) {
        if self.preamp != 1.0 {
            for s in buf.iter_mut() {
                *s *= self.preamp;
            }
        }
        // M6: replay gain -> preamp -> 10-band biquad chain goes here.
    }
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
