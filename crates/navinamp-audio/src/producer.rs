//! The producer (decode) thread.
//!
//! Owns the symphonia readers, the DSP/resampling front-end and the ring-buffer
//! producer half. Runs a simple reactive loop: drain the command queue, then do
//! one unit of work (decode a packet / flush a track tail / swap tracks / check
//! queue drain). See the crate-level docs for the full threading model.
//!
//! Timeline bookkeeping (see [`crate::shared`]): `pushed` counts every output
//! frame written to any ring; the callback's `frames_played` counts every frame
//! consumed (including reset discards). Track sessions mark, in `frames_played`
//! coordinates, where each track (or seek) begins, so
//! `position = media_start + (played - start) / rate` is exact without any locks.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};

use crate::callback::Callback;
use crate::decoder::PcmSource;
use crate::error::AudioError;
use crate::events::Events;
use crate::resample::{ConvertStage, convert_channels};
use crate::ring::{RingProducer, ring_buffer};
use crate::shared::{PlaybackStatus, Session, Shared, TrackMetadata};
use crate::sink::{AudioSink, SinkConfig};
use crate::{Command, Event, TrackSource};

/// Ring buffer depth in seconds of audio.
const RING_SECS: f64 = 2.0;
/// Minimum buffered audio before playback is un-muted (status → Playing).
const PREFILL_SECS: f64 = 0.125;
/// Keep the ring at least this full while decoding.
const HIGH_WATER_SECS: f64 = 1.0;
/// How long a reset (seek/stop) waits for the consumer to observe it.
const RESET_WAIT: Duration = Duration::from_millis(250);
/// Safety valve for waiting on the consumer to drain the queue.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

struct ActiveTrack {
    id: u64,
    src: PcmSource,
    stage: ConvertStage,
    /// Demuxer reported EOF (or an error was treated as EOF); tail flushed.
    eof: bool,
    /// `TrackFinished` has been emitted for this track (done producing).
    announced: bool,
}

struct QueuedTrack {
    id: u64,
    #[allow(dead_code)] // used for diagnostics in future milestones
    source: TrackSource,
    src: PcmSource,
}

pub(crate) struct Producer {
    cmd_rx: Receiver<Command>,
    shared: Arc<Shared>,
    events: Arc<Events>,
    sink: Box<dyn AudioSink>,
    sink_cfg: SinkConfig,
    ring: Option<RingProducer>,
    current: Option<ActiveTrack>,
    next: Option<QueuedTrack>,
    /// Output frames ever pushed into the ring (engine timeline).
    pushed: u64,
    next_track_id: u64,
    draining: bool,
    drain_started: Option<Instant>,
    shutdown: bool,
    /// Decoded frames not yet accepted by the ring (ring was full).
    pending_out: Vec<f32>,
    decode_scratch: Vec<f32>,
    convert_scratch: Vec<f32>,
}

impl Producer {
    /// Open the sink at its default config and spawn the producer thread.
    pub(crate) fn spawn(
        sink: Box<dyn AudioSink>,
        cmd_rx: Receiver<Command>,
        shared: Arc<Shared>,
        events: Arc<Events>,
    ) -> Result<JoinHandle<()>, AudioError> {
        let default_cfg = sink.default_config()?;
        let mut producer = Self {
            cmd_rx,
            shared,
            events,
            sink,
            sink_cfg: default_cfg,
            ring: None,
            current: None,
            next: None,
            pushed: 0,
            next_track_id: 0,
            draining: false,
            drain_started: None,
            shutdown: false,
            pending_out: Vec::new(),
            decode_scratch: Vec::new(),
            convert_scratch: Vec::new(),
        };
        producer.open_sink_at(default_cfg)?;

        let handle = std::thread::Builder::new()
            .name("navinamp-audio-producer".to_string())
            .spawn(move || producer.run())?;
        Ok(handle)
    }

    fn run(mut self) {
        while !self.shutdown {
            while let Ok(cmd) = self.cmd_rx.try_recv() {
                if self.handle_command(cmd) {
                    self.shutdown = true;
                }
            }
            if self.shutdown {
                break;
            }
            if self.tick() {
                std::thread::sleep(Duration::from_micros(200));
            } else {
                match self.cmd_rx.recv_timeout(self.idle_timeout()) {
                    Ok(cmd) => {
                        if self.handle_command(cmd) {
                            self.shutdown = true;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        }
        // Dropping `self` drops the sink (stopping the stream), the rings and
        // all decoders.
    }

    fn idle_timeout(&self) -> Duration {
        if self.current.is_some() || self.next.is_some() || self.draining {
            Duration::from_millis(5)
        } else {
            Duration::from_millis(100)
        }
    }

    /// Do one unit of work. Returns `true` if there is likely more work now.
    fn tick(&mut self) -> bool {
        let mut worked = false;

        self.flush_pending();

        // Current track finished producing and its tail is fully buffered:
        // announce the (producer-side) end of the track.
        if let Some(t) = self.current.as_mut()
            && t.eof
            && !t.announced
            && self.pending_out.is_empty()
        {
            t.announced = true;
            let exact = t.src.decoded_frames() as f64 / f64::from(t.src.rate());
            self.shared.finish_session(t.id, exact);
            self.events.emit(Event::TrackFinished { id: t.id });
            if self.next.is_none() {
                self.draining = true;
                self.drain_started = Some(Instant::now());
            }
            worked = true;
        }

        // Swap: the current track is done and a next track is pre-opened
        // (gapless), or nothing is current at all (EnqueueNext while idle).
        let current_done =
            self.current.as_ref().is_some_and(|t| t.announced) && self.pending_out.is_empty();
        if (self.current.is_none() || current_done)
            && let Some(q) = self.next.take()
        {
            self.current = None;
            self.draining = false;
            self.drain_started = None;
            self.start_track(q);
            self.prefill();
            if self.shared.status() == PlaybackStatus::Stopped {
                self.shared.set_status(PlaybackStatus::Playing);
            }
            worked = true;
        }

        // Queue drained: everything the producer pushed was consumed.
        if self.draining {
            let played = self.shared.played();
            let timed_out = self
                .drain_started
                .is_some_and(|t| t.elapsed() > DRAIN_TIMEOUT);
            if played >= self.pushed || timed_out {
                self.finish_queue();
            }
            worked = true;
        }

        // Decode ahead while the ring is below the high-water mark. The current
        // track stays resident (even once finished) until it is swapped out or
        // the queue finishes, so that seeks during the drain still work.
        if let Some(t) = self.current.as_ref()
            && !t.eof
            && self.shared.status() == PlaybackStatus::Playing
            && self.ring_len_frames() < self.high_water_frames()
        {
            self.decode_block();
            worked = true;
        }

        worked
    }

    // ----- command handling -----

    /// Returns `true` when the engine should shut down.
    fn handle_command(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Play(source) => self.cmd_play(source),
            Command::EnqueueNext(source) => self.cmd_enqueue(source),
            Command::Pause => {
                if self.shared.status() == PlaybackStatus::Playing {
                    self.shared.set_status(PlaybackStatus::Paused);
                }
            }
            Command::Resume => {
                if self.shared.status() == PlaybackStatus::Paused {
                    self.shared.set_status(PlaybackStatus::Playing);
                }
            }
            Command::Stop => self.cmd_stop(),
            Command::Seek(secs) => self.cmd_seek(secs),
            Command::SetVolume(v) => self.shared.set_volume(v.clamp(0.0, 1.0)),
            Command::Shutdown => return true,
        }
        false
    }

    fn cmd_play(&mut self, source: TrackSource) {
        let id = self.alloc_id();
        let src = match open_source(&source) {
            Ok(s) => s,
            Err(e) => {
                // Report and keep whatever is playing; if idle, ensure Stopped.
                self.events.emit(Event::Error {
                    id: Some(id),
                    msg: e.msg(),
                });
                if self.current.is_none() {
                    self.shared.set_status(PlaybackStatus::Stopped);
                }
                return;
            }
        };

        if let Some(old) = self.current.take() {
            // The old track ends without ceremony (it was replaced, not finished).
            if !old.announced {
                self.events.emit(Event::TrackFinished { id: old.id });
            }
        }
        self.next = None;
        self.draining = false;
        self.drain_started = None;
        self.hard_reset();

        // Prefer running the device at the source rate (bit-exact, no resample).
        // Gapless transitions never re-open the device — only explicit Play does.
        let desired = SinkConfig {
            sample_rate: src.rate(),
            channels: 2,
        };
        if desired != self.sink_cfg && self.open_sink_at(desired).is_err() {
            // Device can't: keep the current stream and resample on this thread.
            self.events.emit(Event::Error {
                id: None,
                msg: format!(
                    "device cannot open at {} Hz, resampling to {} Hz",
                    desired.sample_rate, self.sink_cfg.sample_rate
                ),
            });
        }

        self.start_track(QueuedTrack { id, source, src });
        self.prefill();
        self.shared.set_status(PlaybackStatus::Playing);
    }

    fn cmd_enqueue(&mut self, source: TrackSource) {
        let id = self.alloc_id();
        match open_source(&source) {
            Ok(src) => {
                self.next = Some(QueuedTrack { id, source, src });
                // A late enqueue during drain rescues the queue gaplessly.
                self.draining = false;
                self.drain_started = None;
            }
            Err(e) => {
                self.events.emit(Event::Error {
                    id: Some(id),
                    msg: e.msg(),
                });
            }
        }
    }

    fn cmd_stop(&mut self) {
        self.current = None;
        self.next = None;
        self.draining = false;
        self.drain_started = None;
        self.shared.set_metadata(None);
        self.hard_reset();
        self.shared.set_status(PlaybackStatus::Stopped);
    }

    fn cmd_seek(&mut self, secs: f64) {
        if self.current.is_none() {
            // Nothing seekable (fully idle).
            return;
        }
        // Stage the first post-seek block before discarding the buffered audio,
        // so the consumer never sees an empty ring.
        let staged = self.stage_after_seek(secs);
        let Some((actual, audio, at_eof)) = staged else {
            return;
        };
        let (id, duration_hint) = {
            let t = self.current.as_ref().expect("checked above");
            (t.id, t.src.duration_hint_secs)
        };

        self.hard_reset();
        self.pending_out = audio;
        if at_eof && let Some(t) = self.current.as_mut() {
            t.eof = true;
            let mut tail = Vec::new();
            if let Err(e) = t.stage.flush(&mut tail) {
                self.events.emit(Event::Error {
                    id: Some(id),
                    msg: format!("seek flush: {e}"),
                });
            }
            self.pending_out.extend(tail);
        }

        // A seek takes the track "off the finished list": it is current and
        // producing again, so cancel any pending queue-drain.
        self.draining = false;
        self.drain_started = None;

        self.shared.push_session(Session {
            track_id: id,
            start_frame: self.pushed,
            media_start_secs: actual,
            duration_secs: duration_hint,
            rate: self.sink_cfg.sample_rate,
        });
        self.flush_pending();
        // Refill to the prefill target before returning, exactly like a
        // track start: a single staged block leaves the ring with ~zero
        // slack, so the first post-seek (or post-Resume) pull could hit an
        // empty ring whenever the decode thread is briefly starved.
        self.prefill();
    }

    /// Seek the reader and decode/convert the first block into a staging buffer.
    /// Returns `None` only if the seek itself failed (audio continues as before).
    /// A fresh conversion stage is installed: the old one carries stale resampler
    /// phase (and possibly flushed-tail state) from before the seek.
    fn stage_after_seek(&mut self, secs: f64) -> Option<(f64, Vec<f32>, bool)> {
        let t = self.current.as_mut()?;
        let id = t.id;
        let actual = match t.src.seek(secs) {
            Ok(a) => a,
            Err(msg) => {
                self.events.emit(Event::Error { id: Some(id), msg });
                return None;
            }
        };
        // Resurrect the track (it may already have been drained/finished).
        t.eof = false;
        t.announced = false;
        match ConvertStage::new(
            t.src.rate(),
            self.sink_cfg.sample_rate,
            self.sink_cfg.channels as usize,
        ) {
            Ok(stage) => t.stage = stage,
            Err(e) => {
                self.events.emit(Event::Error {
                    id: Some(id),
                    msg: format!("post-seek resampler: {e}"),
                });
                return None;
            }
        }
        self.decode_scratch.clear();
        let mut staged = Vec::new();
        let outcome = t.src.next_block(&mut self.decode_scratch);
        match outcome {
            Ok(true) => {
                let converted = convert_channels(
                    &self.decode_scratch,
                    t.src.channels(),
                    self.sink_cfg.channels as usize,
                    &mut self.convert_scratch,
                );
                if let Err(e) = t.stage.feed(converted, &mut staged) {
                    // Position moved but we could not convert: report and keep
                    // the timeline consistent with an empty staging buffer.
                    self.events.emit(Event::Error {
                        id: Some(id),
                        msg: format!("post-seek convert: {e}"),
                    });
                }
                Some((actual, staged, false))
            }
            Ok(false) => Some((actual, staged, true)),
            Err(msg) => {
                self.events.emit(Event::Error {
                    id: Some(id),
                    msg: format!("post-seek decode: {msg}"),
                });
                Some((actual, staged, false))
            }
        }
    }

    // ----- decode pipeline -----

    /// Decode one packet, convert channels/rate, and append to the ring.
    fn decode_block(&mut self) {
        let Some(t) = self.current.as_mut() else {
            return;
        };
        let id = t.id;
        // `next_block` appends: start each block fresh or history gets re-fed.
        self.decode_scratch.clear();
        let mut out = std::mem::take(&mut self.pending_out);

        match t.src.next_block(&mut self.decode_scratch) {
            Ok(true) => {
                let converted = convert_channels(
                    &self.decode_scratch,
                    t.src.channels(),
                    self.sink_cfg.channels as usize,
                    &mut self.convert_scratch,
                );
                if let Err(e) = t.stage.feed(converted, &mut out) {
                    self.events.emit(Event::Error {
                        id: Some(id),
                        msg: format!("resample: {e}"),
                    });
                    t.eof = true;
                    if let Err(e) = t.stage.flush(&mut out) {
                        self.events.emit(Event::Error {
                            id: Some(id),
                            msg: format!("flush: {e}"),
                        });
                    }
                }
            }
            Ok(false) => {
                t.eof = true;
                if let Err(e) = t.stage.flush(&mut out) {
                    self.events.emit(Event::Error {
                        id: Some(id),
                        msg: format!("flush: {e}"),
                    });
                }
            }
            Err(msg) => {
                // Mid-track decode error: report, then behave like EOF so the
                // queue advances (or stops gracefully) instead of wedging.
                self.events.emit(Event::Error { id: Some(id), msg });
                t.eof = true;
                if let Err(e) = t.stage.flush(&mut out) {
                    self.events.emit(Event::Error {
                        id: Some(id),
                        msg: format!("flush: {e}"),
                    });
                }
            }
        }

        self.pending_out = out;
        self.flush_pending();
    }

    /// Push pending frames into the ring until it refuses more.
    fn flush_pending(&mut self) {
        let Some(ring) = self.ring.as_mut() else {
            return;
        };
        let ch = self.sink_cfg.channels as usize;
        while !self.pending_out.is_empty() {
            let n = ring.push(&self.pending_out);
            if n == 0 {
                break;
            }
            self.pushed += (n / ch) as u64;
            self.pending_out.drain(..n);
        }
    }

    /// Decode until the ring holds at least the prefill target (or EOF/full).
    fn prefill(&mut self) {
        let target = self.prefill_frames();
        let mut guard = 0;
        while self.ring_len_frames() < target
            && self.current.as_ref().is_some_and(|t| !t.eof)
            && self.pending_out.len() < 4 * target as usize * self.sink_cfg.channels as usize
            && guard < 4096
        {
            self.decode_block();
            guard += 1;
        }
    }

    // ----- track lifecycle -----

    fn start_track(&mut self, q: QueuedTrack) {
        let QueuedTrack { id, source, src } = q;
        let stage = match ConvertStage::new(
            src.rate(),
            self.sink_cfg.sample_rate,
            self.sink_cfg.channels as usize,
        ) {
            Ok(s) => s,
            Err(e) => {
                self.events.emit(Event::Error {
                    id: Some(id),
                    msg: format!("resampler: {e}"),
                });
                return;
            }
        };
        // Schedule this track's ReplayGain at the exact frame its audio will
        // begin on the played-frames timeline (== frames pushed so far),
        // *before* any of it enters the ring — the DSP chain adopts it at
        // that frame, so a gapless predecessor's buffered tail keeps its own
        // gain. Published even when 1.0 (cheap; keeps the timeline exact).
        self.shared
            .publish_replaygain(self.pushed, src.replaygain_gain);
        let duration = src.duration_hint_secs;
        self.shared.push_session(Session {
            track_id: id,
            start_frame: self.pushed,
            media_start_secs: 0.0,
            duration_secs: duration,
            rate: self.sink_cfg.sample_rate,
        });
        // Publish the track's audio properties for the classic kbps/kHz and
        // mono/stereo readouts (source rate, not the possibly-resampled sink).
        self.shared.set_metadata(Some(TrackMetadata {
            sample_rate_hz: src.rate(),
            channels: src.channels().min(u16::MAX as usize) as u16,
            bitrate_kbps: src.bitrate_kbps,
        }));
        self.events.emit(Event::TrackStarted {
            id,
            duration_secs: duration,
        });
        // Keep `source` around for future diagnostics (stream URLs in M5).
        let _ = source;
        self.current = Some(ActiveTrack {
            id,
            src,
            stage,
            eof: false,
            announced: false,
        });
    }

    fn finish_queue(&mut self) {
        self.draining = false;
        self.drain_started = None;
        self.current = None;
        self.shared.set_metadata(None);
        self.shared.set_status(PlaybackStatus::Stopped);
        self.events.emit(Event::EndOfQueue);
    }

    // ----- sink / ring management -----

    /// (Re)open the output stream at `cfg`, building a matching ring + callback.
    fn open_sink_at(&mut self, cfg: SinkConfig) -> Result<(), AudioError> {
        let frames = (f64::from(cfg.sample_rate) * RING_SECS).ceil() as u64;
        let (prod, cons) = ring_buffer(frames, cfg.channels);
        let mut cb = Callback::new(cons, self.shared.clone(), &cfg);
        let events = self.events.clone();
        let err_cb = Box::new(move |msg: String| {
            events.emit(Event::Error { id: None, msg });
        }) as Box<dyn FnMut(String) + Send>;
        self.sink
            .open(cfg, Box::new(move |out: &mut [f32]| cb.pull(out)), err_cb)?;

        // The analyser maps FFT bins with the output rate; keep it current.
        self.shared.set_sink_rate(cfg.sample_rate);

        // The old stream and its callback are gone now. Any old-ring frames that
        // were still buffered will never play; account them as consumed so the
        // played/pushed counters stay FIFO-aligned (≤ RING_SECS one-time shift,
        // only when the device rate changes mid-session).
        let played = self.shared.played();
        let deficit = self.pushed.saturating_sub(played);
        if deficit > 0 {
            self.shared
                .frames_played
                .fetch_add(deficit, Ordering::Relaxed);
        }

        self.ring = Some(prod);
        self.sink_cfg = cfg;
        Ok(())
    }

    /// Discard all buffered audio: bump the ring generation, wait (bounded) for
    /// the consumer to acknowledge the discard, then forget pending output.
    fn hard_reset(&mut self) {
        self.pending_out.clear();
        if let Some(ring) = self.ring.as_mut() {
            let target = ring.request_reset();
            ring.wait_reset_acked(target, RESET_WAIT);
            ring.refresh();
        }
    }

    // ----- helpers -----

    fn ring_len_frames(&self) -> u64 {
        self.ring
            .as_ref()
            .map_or(0, |r| r.len() / u64::from(self.sink_cfg.channels))
    }

    fn prefill_frames(&self) -> u64 {
        let rate = f64::from(self.sink_cfg.sample_rate);
        ((rate * PREFILL_SECS).ceil() as u64).max(1024)
    }

    fn high_water_frames(&self) -> u64 {
        (f64::from(self.sink_cfg.sample_rate) * HIGH_WATER_SECS) as u64
    }

    fn alloc_id(&mut self) -> u64 {
        self.next_track_id += 1;
        self.next_track_id
    }
}

fn open_source(source: &TrackSource) -> Result<PcmSource, AudioError> {
    match source {
        TrackSource::File(path) => PcmSource::open(path),
        TrackSource::Remote { url } => PcmSource::open_remote(url),
    }
}
