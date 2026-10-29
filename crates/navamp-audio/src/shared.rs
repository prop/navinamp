//! Shared engine state readable without locks from the UI side.
//!
//! The **timeline model**: the output callback increments `frames_played` for every
//! frame it consumes from the ring buffer (including frames it *discards* on a
//! seek/stop reset — those were pushed by the producer, so they must be counted to
//! keep both ends FIFO-aligned). The producer increments its private `pushed`
//! counter for every frame it writes. The invariant "the k-th popped frame is the
//! k-th pushed frame" lets the producer place track boundaries exactly, without
//! synchronization, on this shared counter.
//!
//! A track **session** records when (in `frames_played` units) a given track's
//! output began and where in the media it started (non-zero after a seek). Sessions
//! are appended by the producer and read by [`crate::EngineHandle`] — both
//! non-real-time threads — so a plain mutex is fine; the audio callback never
//! touches this structure.

use std::cell::UnsafeCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use crate::eq::EqParams;
use crate::spectrum::SpectrumTap;

/// Playback status encoded into a single atomic byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    /// No track loaded, or the queue ran out.
    Stopped,
    /// Audio is being consumed.
    Playing,
    /// The stream is running but emits silence; position is frozen.
    Paused,
}

impl PlaybackStatus {
    pub(crate) const fn to_u8(self) -> u8 {
        match self {
            PlaybackStatus::Stopped => 0,
            PlaybackStatus::Playing => 1,
            PlaybackStatus::Paused => 2,
        }
    }
    pub(crate) const fn from_u8(v: u8) -> Self {
        match v {
            1 => PlaybackStatus::Playing,
            2 => PlaybackStatus::Paused,
            _ => PlaybackStatus::Stopped,
        }
    }
}

/// One track's mapping onto the global played-frames timeline.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    pub track_id: u64,
    /// `frames_played` value at which this track's first output frame was consumed.
    pub start_frame: u64,
    /// Position within the media where output began (0.0, or the seek target).
    pub media_start_secs: f64,
    /// Track duration in media time, if known (from container metadata, replaced
    /// by the exact decoded count once the track finishes).
    pub duration_secs: Option<f64>,
    /// Output sample rate this session's frames are counted in (the sink rate).
    pub rate: u32,
}

/// Atomically-shared engine state.
pub(crate) struct Shared {
    pub status: AtomicU8,
    /// Target volume as f32 bits (lock-free f32).
    pub volume_bits: AtomicU32,
    /// Monotonic count of output frames consumed by the callback.
    pub frames_played: AtomicU64,
    /// Frames of silence emitted while Playing because the ring ran dry.
    pub underrun_frames: AtomicU64,
    /// Append-only session log (pruned occasionally by the producer).
    pub sessions: Mutex<Vec<Session>>,
    /// EQ parameter store (lock-free; see `eq.rs`). Cloned into the output
    /// callback's DSP chain at sink construction.
    pub eq: std::sync::Arc<EqParams>,
    /// Post-DSP analyser tap (see `spectrum.rs`), written by the callback.
    pub tap: std::sync::Arc<SpectrumTap>,
    /// Current output sample rate (analyser bin mapping; diagnostic).
    sink_rate: AtomicU32,
    /// ReplayGain switch markers, producer → callback (see [`RgMarkers`]).
    pub(crate) rg_markers: RgMarkers,
    /// The most recently published ReplayGain gain as f32 bits
    /// (diagnostics: the gain scheduled for the current/next track).
    replaygain_bits: AtomicU32,
}

impl Shared {
    pub(crate) fn new(initial_volume: f32) -> Self {
        Self {
            status: AtomicU8::new(PlaybackStatus::Stopped.to_u8()),
            volume_bits: AtomicU32::new(initial_volume.to_bits()),
            frames_played: AtomicU64::new(0),
            underrun_frames: AtomicU64::new(0),
            sessions: Mutex::new(Vec::new()),
            eq: std::sync::Arc::new(EqParams::new()),
            tap: std::sync::Arc::new(SpectrumTap::new()),
            sink_rate: AtomicU32::new(44_100),
            rg_markers: RgMarkers::new(),
            replaygain_bits: AtomicU32::new(1.0f32.to_bits()),
        }
    }

    pub(crate) fn status(&self) -> PlaybackStatus {
        PlaybackStatus::from_u8(self.status.load(Ordering::Relaxed))
    }
    pub(crate) fn set_status(&self, s: PlaybackStatus) {
        self.status.store(s.to_u8(), Ordering::Relaxed);
    }
    pub(crate) fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }
    pub(crate) fn set_volume(&self, v: f32) {
        self.volume_bits.store(v.to_bits(), Ordering::Relaxed);
    }
    pub(crate) fn played(&self) -> u64 {
        self.frames_played.load(Ordering::Relaxed)
    }

    /// Append a session (producer side).
    pub(crate) fn push_session(&self, session: Session) {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.push(session);
        // Prune: keep the tail; the newest session is always the active one.
        if sessions.len() > 64 {
            let keep = 8;
            let drop = sessions.len() - keep;
            sessions.drain(..drop);
        }
    }

    /// Update the duration of the most recent session for `track_id`.
    pub(crate) fn finish_session(&self, track_id: u64, duration_secs: f64) {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(last) = sessions.last_mut()
            && last.track_id == track_id
        {
            last.duration_secs = Some(duration_secs);
        }
    }

    /// The session containing the current playback position.
    fn active_session(&self) -> Option<Session> {
        let played = self.played();
        let sessions = self.sessions.lock().unwrap();
        sessions
            .iter()
            .rev()
            .find(|s| s.start_frame <= played)
            .cloned()
    }

    /// Current position in media time (seconds). Lock-free except for the session
    /// mutex, which is never held by the audio callback.
    pub(crate) fn position_secs(&self) -> f64 {
        let Some(s) = self.active_session() else {
            return 0.0;
        };
        let rel = self.played().saturating_sub(s.start_frame) as f64 / f64::from(s.rate);
        let pos = s.media_start_secs + rel;
        // Clamp to the known duration: resampler flush padding can momentarily
        // overshoot by a few ms.
        match s.duration_secs {
            Some(d) => pos.min(d),
            None => pos,
        }
    }

    /// Duration of the current track, if known.
    pub(crate) fn duration_secs(&self) -> Option<f64> {
        self.sessions
            .lock()
            .unwrap()
            .last()
            .and_then(|s| s.duration_secs)
    }

    /// Record the output sample rate (producer side, whenever a sink opens).
    pub(crate) fn set_sink_rate(&self, rate: u32) {
        self.sink_rate.store(rate, Ordering::Relaxed);
    }

    /// The current output sample rate (analyser bin mapping).
    pub(crate) fn sink_rate(&self) -> u32 {
        self.sink_rate.load(Ordering::Relaxed)
    }

    /// Schedule a ReplayGain switch: `gain` applies to every output frame at
    /// absolute index ≥ `frame` (producer side, published *before* the first
    /// such frame enters the ring, so the FIFO timeline orders it). Also
    /// updates the diagnostic copy read by [`Shared::replaygain_gain`].
    pub(crate) fn publish_replaygain(&self, frame: u64, gain: f32) {
        self.rg_markers.push(frame, gain);
        self.replaygain_bits
            .store(gain.to_bits(), Ordering::Release);
    }

    /// The most recently published ReplayGain gain (diagnostics).
    pub(crate) fn replaygain_gain(&self) -> f32 {
        f32::from_bits(self.replaygain_bits.load(Ordering::Acquire))
    }
}

/// How many ReplayGain markers can sit unread before the producer reuses
/// slots. Markers are published once per track start; the callback drains
/// at least one per block, so falling a full lap behind requires ~8 track
/// starts inside a single output block — impossible short of a dead device
/// (and `RgMarkers::take` degrades gracefully even then).
const RG_MARKER_CAPACITY: u64 = 8;

#[derive(Clone, Copy)]
struct RgSlot {
    frame: u64,
    gain: f32,
}

/// A tiny single-producer/single-consumer ring of *switch markers*:
/// "linear gain `gain` takes effect at absolute output frame `frame`".
///
/// This is what makes ReplayGain **sample-accurate at gapless boundaries**:
/// the producer publishes the marker (in `frames_played` coordinates — the
/// same FIFO timeline as [`Session`]s) before pushing the new track's first
/// frame, and the DSP chain adopts each marker at exactly its frame, so the
/// previous track's buffered tail (up to ~1 s) keeps its own gain.
///
/// Ordering: the writer fills the slot, then publishes the ticket
/// (`Release`); readers load the ticket (`Acquire`) and only then copy
/// slots whose writes happened-before that ticket. A slot can only be
/// reused after a full lap, which `take` detects and skips past.
pub(crate) struct RgMarkers {
    slots: [UnsafeCell<RgSlot>; RG_MARKER_CAPACITY as usize],
    /// Tickets ever published (producer-owned).
    write: AtomicU64,
}

// SAFETY: `slots` cells are written only by the producer (before the `write`
// publication) and read only by the consumer (only for tickets it has
// acquired, and only while the producer is less than a lap ahead — see
// `take`'s post-check).
unsafe impl Sync for RgMarkers {}

impl RgMarkers {
    pub(crate) fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| {
                UnsafeCell::new(RgSlot {
                    frame: 0,
                    gain: 1.0,
                })
            }),
            write: AtomicU64::new(0),
        }
    }

    /// Producer side: append one marker. Wait-free.
    pub(crate) fn push(&self, frame: u64, gain: f32) {
        let ticket = self.write.load(Ordering::Relaxed);
        // SAFETY: only the producer thread writes slots, and only the slot
        // for the unpublished ticket `write`.
        let slot = unsafe { &mut *self.slots[(ticket % RG_MARKER_CAPACITY) as usize].get() };
        slot.frame = frame;
        slot.gain = gain;
        self.write.store(ticket + 1, Ordering::Release);
    }

    /// Consumer side (RT): pop the next marker in publish order, if any.
    /// `read` is the caller's private cursor. Practically wait-free; the
    /// retry only triggers if the producer published a full lap of markers
    /// *during* the copy (a dead-device pathology).
    pub(crate) fn take(&self, read: &mut u64) -> Option<(u64, f32)> {
        loop {
            let published = self.write.load(Ordering::Acquire);
            if *read >= published {
                return None;
            }
            // If the producer lapped us (device stalled through many track
            // starts), skip to the oldest surviving marker: everything older
            // was superseded by hard resets that discarded its audio anyway.
            if published - *read > RG_MARKER_CAPACITY {
                *read = published - RG_MARKER_CAPACITY;
            }
            // SAFETY: we hold ticket `*read`, published (hence fully
            // written) and less than a lap behind `published`.
            let slot = unsafe { *self.slots[(*read % RG_MARKER_CAPACITY) as usize].get() };
            // The slot was reused while we copied it only if the producer
            // advanced a full lap meanwhile; retry with the fresh state.
            if self.write.load(Ordering::Acquire) - *read > RG_MARKER_CAPACITY {
                continue;
            }
            *read += 1;
            return Some((slot.frame, slot.gain));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_deliver_in_order() {
        let markers = RgMarkers::new();
        let mut read = 0;
        assert!(markers.take(&mut read).is_none());
        markers.push(100, 0.5);
        markers.push(200, 0.25);
        assert_eq!(markers.take(&mut read), Some((100, 0.5)));
        assert_eq!(markers.take(&mut read), Some((200, 0.25)));
        assert!(markers.take(&mut read).is_none());
    }

    #[test]
    fn markers_skip_to_survivors_when_lapped() {
        let markers = RgMarkers::new();
        for i in 0..(RG_MARKER_CAPACITY + 3) {
            markers.push(i * 10, i as f32);
        }
        let mut read = 0;
        // Falling 11 behind skips the 3 oldest; the survivors arrive in
        // order and the stream is not corrupted.
        assert_eq!(markers.take(&mut read), Some((30, 3.0)));
        assert_eq!(markers.take(&mut read), Some((40, 4.0)));
        for i in 5..=(RG_MARKER_CAPACITY + 2) {
            assert_eq!(markers.take(&mut read), Some((i * 10, i as f32)));
        }
        assert!(markers.take(&mut read).is_none());
    }
}
