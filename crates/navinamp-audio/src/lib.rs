//! # navinamp-audio
//!
//! Gapless-capable playback engine for NaVinaMP: **symphonia decode → DSP →
//! lock-free ring buffer → cpal output**.
//!
//! ## Thread model
//!
//! ```text
//! UI / IPC thread                 producer thread ("navinamp-audio-producer")     audio callback (device RT thread)
//! ────────────────                ─────────────────────────────────────────    ────────────────────────────────
//! EngineHandle                    Producer                                     Callback
//!  ├─ Command  ──crossbeam────▶   ├─ PcmSource (symphonia FormatReader+Decoder)  ├─ RingConsumer   (lock-free)
//!  ├─ atomics  ◀─……Relaxed……▶    ├─ channel convert → rubato resample           ├─ DspChain       (RG→preamp→EQ)
//!  └─ Receiver(Event) ◀───────    ├─ RingProducer (lock-free SPSC)               ├─ SpectrumTap    (post-DSP)
//!                                 └─ Events broadcast ────────────────────────▶  └─ atomics: status/volume/played
//!
//! prefetch pool ("navinamp-audio-prefetch-N")
//!  └─ opens/probes lookahead sources (HTTP GETs, ReplayGain, container probe)
//!     and hands ready PcmSources to the producer
//!
//! play opener ("navinamp-audio-play-opener")
//!  └─ opens explicit Play targets ahead of any lookahead work
//! ```
//!
//! * **One** decoder/producer thread owns the active symphonia reader and the
//!   prefetch window. A dedicated **play opener** opens explicit `Play`
//!   targets, while a small **prefetch pool** opens the ordered lookahead
//!   window, so network probe latency can never stall the track that is
//!   currently playing.
//! * The **cpal callback** only touches the ring buffer, atomics and fixed
//!   per-block DSP state — it never allocates, locks, or blocks.
//! * Position tracking is a samples-played counter (`AtomicU64`); status is an
//!   `AtomicU8`; volume is an f32 bit-cast into an `AtomicU32`. All `Relaxed`:
//!   nothing else piggybacks on them.
//!
//! ## Sample-rate policy
//!
//! On `Play`, the engine tries to (re)open the device at the **source** sample
//! rate (stereo) so most playback is bit-exact with no resampling. If the device
//! refuses that rate (or a test sink is pinned), the current stream is kept and
//! the track is **resampled on the decode thread** with an FFT-based synchronous
//! resampler (rubato, fixed chunks). Gapless track transitions never re-open the
//! device — a next track at a different rate is resampled to the running stream
//! rate instead.
//!
//! ## Gapless + lookahead
//!
//! `SetUpcoming` declares the ordered tracks after the current one. The front
//! (gapless successor) opens first and the rest follow in a bounded window
//! (currently four entries, two opener workers), so both automatic advances
//! and forward skips avoid a network wait. Only two opens run at a time, so a
//! deep entry is instant once its open has completed; on a slow server the
//! far entries land progressively. `PlayKeyed` reuses a ready window entry
//! instantly. `EnqueueNext` remains the single-entry form. When the next
//! source is ready its decoded audio simply continues in the same ring buffer
//! after the current track's last frame — zero inserted
//! silence. `TrackFinished`/`TrackStarted` fire from the producer at the moment
//! the swap is *scheduled* (i.e. when the last frame of a track enters the
//! ring), which leads the audible transition by at most the ring-buffer latency
//! (2 s worst case, typically ≪100 ms since the callback is draining it
//! continuously). `EndOfQueue` fires after the last audible frame has been
//! consumed.
//!
//! ## DSP insertion points
//!
//! The plan's chain — *replay gain → preamp → 10× biquad EQ → volume* — runs in
//! the callback between the ring buffer and the volume ramp; see `src/dsp.rs`.
//! EQ parameters travel lock-free (atomics + generation counter, `src/eq.rs`);
//! ReplayGain is scanned with lofty at track open (`src/replaygain.rs`) and
//! applied as a frame-accurate per-track scalar; a post-DSP analyser tap
//! (`src/spectrum.rs`) feeds [`EngineHandle::spectrum`].
//!
//! ## Supported formats
//!
//! MP3, FLAC, Ogg Vorbis, AAC/ALAC (M4A), WAV (probed by content) via symphonia
//! 0.5. **Opus note**: symphonia 0.5/0.6 has *no* released Opus decoder (the
//! `ogg` container is enabled, but `.opus` files will surface an
//! [`Event::Error`]). Revisit in M6 (third-party adapter or ffmpeg sidecar).
//!
//! ## Remote streaming
//!
//! [`TrackSource::Remote`] plays an `http(s)://` URL through the same decode
//! path as a file: the producer thread wraps a blocking `Read + Seek` HTTP
//! transport (`src/http_stream.rs`, reqwest + rustls) in symphonia's
//! `MediaSourceStream`. Seeks issue `Range: bytes=N-` requests; servers that
//! ignore ranges (Navidrome transcodes) are fully downloaded to a temp-file
//! spill on the first seek and served locally thereafter (the spill is
//! deleted on drop). Gapless local↔remote transitions and lookahead
//! pre-opening work identically to files; the next tracks' initial GETs,
//! ReplayGain scans and container probes run on opener threads. Resolved
//! media links can carry request headers (a matching User-Agent / Referer)
//! through [`TrackSource::RemoteWithHeaders`]. The transport never touches the
//! audio callback; URL credentials are scrubbed from surfaced errors.
//!
//! ## Example
//!
//! ```no_run
//! use navinamp_audio::{Command, EngineHandle, Event, PlaybackStatus, TrackSource};
//! # fn main() -> Result<(), navinamp_audio::AudioError> {
//! let engine = EngineHandle::start()?;
//! let events = engine.subscribe();
//! engine.send(Command::Play(TrackSource::File("a.flac".into())))?;
//! engine.send(Command::EnqueueNext(TrackSource::File("b.flac".into())))?; // gapless
//! engine.send(Command::SetVolume(0.8))?;
//! loop {
//!     match events.recv() {
//!         Ok(Event::TrackStarted { id, duration_secs }) => {
//!             println!("track {id} started ({duration_secs:?} s)");
//!         }
//!         Ok(Event::EndOfQueue) => break,
//!         Ok(_) => {}
//!         Err(_) => break,
//!     }
//! }
//! assert_eq!(engine.status(), PlaybackStatus::Stopped);
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

mod callback;
mod decoder;
mod dsp;
mod eq;
mod error;
mod events;
mod http_stream;
mod producer;
mod replaygain;
mod resample;
mod ring;
mod shared;
mod sink;
mod sink_cpal;
mod sink_test;
mod spectrum;
mod wav_repair;

pub use dsp::auto_headroom_db;
pub use eq::{EQ_BAND_COUNT, EQ_BAND_FREQUENCIES_HZ, EQ_RANGE_DB, EqPreset, EqState, eq_presets};
pub use error::AudioError;
pub use shared::{PlaybackStatus, TrackMetadata};
pub use sink::{AudioSink, SinkConfig};
pub use sink_cpal::CpalSink;
pub use sink_test::TestSink;
pub use spectrum::{SPECTRUM_BINS, Spectrum};

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use events::Events;
use producer::Producer;
use shared::Shared;

/// What to play.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TrackSource {
    /// A local file, probed by content.
    File(PathBuf),
    /// A remote HTTP(S) stream, probed by content. Served over the crate's
    /// [`HttpStream`](crate#remote-streaming) transport (ranged GETs; a
    /// range-ignoring server — e.g. a transcode — is fully downloaded to a
    /// temp file on the first seek). Gapless/seek behave like local files
    /// once seekable.
    Remote {
        /// Absolute `http://`/`https://` URL. May carry query credentials
        /// (Subsonic tokens); the engine never logs it.
        url: String,
    },
    /// A remote stream whose TLS certificate must not be verified.
    ///
    /// Self-hosted servers commonly use a self-signed certificate. The
    /// caller opts in per server (the Navidrome profile's insecure-TLS
    /// setting); this variant keeps the exception scoped to that stream.
    RemoteInsecure {
        /// Absolute `http(s)://` URL. May carry credentials; never logged.
        url: String,
    },
    /// A remote stream that needs extra request headers on every GET.
    ///
    /// Used by resolved media links (e.g. yt-dlp's `http_headers`, which carry
    /// a matching User-Agent/Referer). Behaving exactly like
    /// [`TrackSource::Remote`] otherwise.
    RemoteWithHeaders {
        /// Absolute `http://`/`https://` URL. May carry credentials; the
        /// engine never logs it.
        url: String,
        /// Extra request headers (`(name, value)`).
        headers: Vec<(String, String)>,
    },
}

impl TrackSource {
    /// Whether this source decodes over the network. Exhaustive on purpose:
    /// a new variant must be classified here or underrun attribution and the
    /// remote decode-ahead silently miss it.
    pub fn is_remote(&self) -> bool {
        matches!(
            self,
            TrackSource::Remote { .. }
                | TrackSource::RemoteInsecure { .. }
                | TrackSource::RemoteWithHeaders { .. }
        )
    }
}

/// One entry in the engine's prefetch window (see [`Command::SetUpcoming`]).
///
/// The key is a stable caller-side identity (the UI's queue-item id). Window
/// updates are reconciled by key: an entry that keeps its key keeps its
/// already-opened source, even when the queue around it was edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpcomingTrack {
    /// Stable caller-chosen identity for this entry.
    pub key: String,
    /// The source to open and play when this entry reaches the front.
    pub source: TrackSource,
}

/// Commands sent from the UI side to the engine.
#[derive(Debug, Clone)]
pub enum Command {
    /// Replace the queue and play this source now (also the "Play" button).
    ///
    /// A window entry whose source matches is reused if it is already open;
    /// otherwise the source is opened off the decode thread.
    Play(TrackSource),
    /// Play this source now, preferring the pre-opened window entry `key`.
    ///
    /// This is the keyed form of [`Command::Play`] for callers that also use
    /// [`Command::SetUpcoming`]: when the entry is ready the switch is instant
    /// and performs no I/O. When `key` is not in the window it behaves exactly
    /// like [`Command::Play`].
    PlayKeyed {
        /// Key of the prefetched [`UpcomingTrack`] to reuse, if present.
        key: String,
        /// Source to open when the key is not already prefetched.
        source: TrackSource,
    },
    /// Pre-open the next source for a gapless hand-off (replaces the whole
    /// prefetch window with this single entry; starts playback immediately
    /// when the engine is idle).
    EnqueueNext(TrackSource),
    /// Replace the prefetch window with these tracks, in play order.
    ///
    /// The front entry is the gapless successor. The engine keeps the first
    /// few entries opened ahead (bounded); entries beyond the active window
    /// stay queued and are opened as the front advances. Entries whose keys
    /// match the previous window are carried over, including their already
    /// opened sources, so a queue edit or a normal advance does not re-open
    /// them. An empty list cancels all lookahead without touching the track
    /// that is currently playing. On a fully idle engine this only pre-opens
    /// sources; it never starts playback by itself.
    SetUpcoming(Vec<UpcomingTrack>),
    /// Suspend output; the device stream keeps running and emits silence while
    /// the position freezes.
    Pause,
    /// Continue after [`Command::Pause`].
    Resume,
    /// Clear queue + buffers. Emits no events.
    Stop,
    /// Seek within the current track (seconds, media time). Best-effort per
    /// container; errors surface as [`Event::Error`].
    Seek(f64),
    /// Set the target volume (0..=1); the callback ramps to it (no clicks).
    SetVolume(f32),
    /// Stop the engine thread and drop the device stream.
    Shutdown,
}

/// Engine events, in order, delivered to every subscriber.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The producer scheduled this track's output (leads the audible start by
    /// at most the ring-buffer latency).
    TrackStarted {
        /// Engine-assigned track id.
        id: u64,
        /// Container-reported duration when known; replaced by the exact decoded
        /// duration at track end.
        duration_secs: Option<f64>,
    },
    /// The last frame of this track entered the ring buffer (gapless swap
    /// scheduled, or the track was replaced by `Play`).
    TrackFinished {
        /// Engine-assigned track id.
        id: u64,
    },
    /// The last track's final frame was consumed by the output.
    EndOfQueue,
    /// Something failed. `id` refers to a track when known. Playback skips to
    /// the next queued track or stops gracefully; it never wedges or panics.
    Error {
        /// Track id if the error is track-specific.
        id: Option<u64>,
        /// Human-readable description.
        msg: String,
    },
}

/// Handle to a running engine. Cloning the handle is not supported; keep one
/// and share the [`Command`] sender from [`EngineHandle::command_sender`] if
/// needed.
pub struct EngineHandle {
    cmd: Sender<Command>,
    shared: Arc<Shared>,
    events: Arc<Events>,
    join: Option<JoinHandle<()>>,
}

impl EngineHandle {
    /// Start the engine on the default output device.
    pub fn start() -> Result<Self, AudioError> {
        let sink = CpalSink::new()?;
        Self::start_with_sink(sink)
    }

    /// Start the engine on a custom sink (e.g. [`TestSink`] in tests).
    pub fn start_with_sink(sink: impl AudioSink) -> Result<Self, AudioError> {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let shared = Arc::new(Shared::new(1.0));
        let events = Arc::new(Events::new());
        let join = Producer::spawn(Box::new(sink), cmd_rx, shared.clone(), events.clone())?;
        Ok(Self {
            cmd: cmd_tx,
            shared,
            events,
            join: Some(join),
        })
    }

    /// Send a command. Fails only if the engine thread has exited.
    pub fn send(&self, cmd: Command) -> Result<(), AudioError> {
        self.cmd.send(cmd).map_err(|_| AudioError::EngineStopped)
    }

    /// A cloneable sender for threads that need to issue commands.
    pub fn command_sender(&self) -> Sender<Command> {
        self.cmd.clone()
    }

    /// Current playback status (atomic read).
    pub fn status(&self) -> PlaybackStatus {
        self.shared.status()
    }

    /// Position in the current track, seconds (media time). Lock-free from the
    /// audio path's point of view; reads a small session log owned by the
    /// producer side.
    pub fn position_secs(&self) -> f64 {
        self.shared.position_secs()
    }

    /// Duration of the current track, if known.
    pub fn duration_secs(&self) -> Option<f64> {
        self.shared.duration_secs()
    }

    /// Audio properties of the current track (source sample rate, channel
    /// count, average bitrate); `None` while stopped.
    pub fn metadata(&self) -> Option<TrackMetadata> {
        self.shared.metadata()
    }

    /// Target volume (0..=1).
    pub fn volume(&self) -> f32 {
        self.shared.volume()
    }

    /// Frames of silence emitted while `Playing` because the ring ran dry
    /// (diagnostics; should stay 0 in normal operation).
    pub fn underrun_frames(&self) -> u64 {
        self.shared.underrun_frames.load(Ordering::Relaxed)
    }

    /// Of [`Self::underrun_frames`], the frames lost while a **remote**
    /// source was active (network stalls vs local decode/device hiccups).
    pub fn underrun_remote_frames(&self) -> u64 {
        self.shared.underrun_remote_frames()
    }

    // ----- equaliser (see `eq.rs`; all reads/writes are lock-free) -----

    /// The current EQ state (atomic snapshot).
    pub fn eq_state(&self) -> EqState {
        let snapshot = self.shared.eq.snapshot();
        EqState {
            enabled: snapshot.enabled,
            preamp_db: snapshot.preamp_db,
            bands_db: snapshot.bands_db,
        }
    }

    /// Enable or disable the EQ stage (takes effect at the next block; a
    /// flat curve stays bit-transparent either way).
    pub fn eq_set_enabled(&self, enabled: bool) {
        self.shared.eq.set_enabled(enabled);
    }

    /// Set the preamp in dB (server-side clamp: ±[`EQ_RANGE_DB`];
    /// non-finite values become 0 dB).
    pub fn eq_set_preamp(&self, db: f32) {
        self.shared.eq.set_preamp_db(db);
    }

    /// Set one band's gain in dB. `index` must be `0..EQ_BAND_COUNT`
    /// (else [`AudioError::InvalidParam`]); the value is clamped like the
    /// preamp.
    pub fn eq_set_band(&self, index: u32, db: f32) -> Result<(), AudioError> {
        if index as usize >= EQ_BAND_COUNT {
            return Err(AudioError::InvalidParam(format!(
                "EQ band index {index} out of range (0..{EQ_BAND_COUNT})"
            )));
        }
        self.shared.eq.set_band_db(index as usize, db);
        Ok(())
    }

    /// The ReplayGain scalar scheduled for the current (or next) track
    /// (diagnostics; 1.0 when nothing is tagged).
    pub fn replaygain_gain(&self) -> f32 {
        self.shared.replaygain_gain()
    }

    /// One analyser snapshot: 32 log-spaced spectrum bars and the last 128
    /// waveform samples, computed on demand from the post-DSP tap. `active`
    /// is false unless the engine is currently [`PlaybackStatus::Playing`].
    /// Intended for a ~60 Hz UI poll; never touches the audio callback.
    pub fn spectrum(&self) -> Spectrum {
        let active = self.shared.status() == PlaybackStatus::Playing;
        let values = spectrum::analyse(&self.shared.tap.window(), self.shared.sink_rate());
        Spectrum {
            active,
            spectrum: values.spectrum,
            waveform: values.waveform,
        }
    }

    /// Subscribe to engine events. Every subscriber sees every event; use
    /// non-blocking `try_recv` or `recv_timeout` on the returned receiver.
    pub fn subscribe(&self) -> Receiver<Event> {
        self.events.subscribe()
    }

    /// Stop the engine and wait for the producer thread to exit.
    pub fn shutdown(mut self) {
        self.stop_join();
    }

    fn stop_join(&mut self) {
        let _ = self.cmd.send(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.stop_join();
    }
}
