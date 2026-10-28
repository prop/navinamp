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

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

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
}

impl Shared {
    pub(crate) fn new(initial_volume: f32) -> Self {
        Self {
            status: AtomicU8::new(PlaybackStatus::Stopped.to_u8()),
            volume_bits: AtomicU32::new(initial_volume.to_bits()),
            frames_played: AtomicU64::new(0),
            underrun_frames: AtomicU64::new(0),
            sessions: Mutex::new(Vec::new()),
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
}
