//! The real-time consumer: pulls frames from the ring buffer into the device.
//!
//! Hard rules enforced here — this code runs on the audio interrupt:
//! * no allocation,
//! * no locks (the session log is never touched),
//! * no blocking (ring operations are wait-free),
//! * no panics (no unwraps, no unchecked indexing).
//!
//! Per pull: sync ring generation (fast-forward discards after seek/stop), then
//! pop audio while [`PlaybackStatus::Playing`], emit silence otherwise. The DSP
//! chain (replaygain → preamp → EQ, see [`crate::dsp`]) runs *before* the
//! volume ramp, matching the plan's chain order: replaygain → preamp → EQ →
//! volume. The analyser tap ([`crate::spectrum`]) copies the post-DSP,
//! pre-volume block for `audio_spectrum`.

use crate::dsp::{DspChain, VolumeRamp};
use crate::ring::RingConsumer;
use crate::shared::{PlaybackStatus, Shared};
use crate::sink::SinkConfig;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) struct Callback {
    cons: RingConsumer,
    shared: Arc<Shared>,
    channels: usize,
    ramp: VolumeRamp,
    dsp: DspChain,
}

impl Callback {
    pub(crate) fn new(cons: RingConsumer, shared: Arc<Shared>, cfg: &SinkConfig) -> Self {
        let eq = std::sync::Arc::clone(&shared.eq);
        Self {
            cons,
            ramp: VolumeRamp::new(cfg.sample_rate, shared.volume()),
            dsp: DspChain::new(cfg.channels as usize, cfg.sample_rate, eq),
            shared,
            channels: cfg.channels as usize,
        }
    }

    /// Fill `out` (interleaved f32, frame-aligned) with the next audio block.
    pub(crate) fn pull(&mut self, out: &mut [f32]) {
        // Always observe resets, even while paused, so producer reset handshakes
        // (seek/stop) complete promptly. Discarded frames still count as played.
        let discarded = self.cons.sync_generation();

        if self.shared.status() != PlaybackStatus::Playing {
            out.fill(0.0);
            if discarded > 0 {
                self.account(discarded);
            }
            return;
        }

        let got = self.cons.pop(out);
        if discarded > 0 {
            self.account(discarded);
        }
        if got > 0 {
            self.account(got);
        }
        if got < out.len() {
            // Ring ran dry mid-block: emit silence for the remainder and count it
            // as an underrun (diagnostics + tests). The fill is mandatory: the
            // device buffer contents are undefined, and `pop` only wrote
            // `out[..got]` — without this, an underrun would replay the previous
            // period (or garbage) instead of silence.
            let missing = (out.len() - got) / self.channels;
            self.shared
                .underrun_frames
                .fetch_add(missing as u64, Ordering::Relaxed);
            out[got..].fill(0.0);
        }

        let signal = &mut out[..got];
        // DSP (replaygain → preamp → EQ) then the analyser tap, then volume:
        // the tap must hear the post-DSP, pre-volume signal.
        let played = self.shared.played();
        let block_frames = got / self.channels;
        let first_frame = played - block_frames as u64;
        self.dsp
            .process(signal, first_frame, &self.shared.rg_markers);
        self.shared.tap.write(signal, self.channels);
        let target = self.shared.volume();
        self.ramp.apply(signal, target);
    }

    fn account(&self, samples: usize) {
        let frames = (samples / self.channels) as u64;
        if frames > 0 {
            self.shared
                .frames_played
                .fetch_add(frames, Ordering::Relaxed);
        }
    }
}
