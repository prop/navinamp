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

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::callback::Callback;
use crate::decoder::PcmSource;
use crate::error::AudioError;
use crate::events::Events;
use crate::resample::{ConvertStage, convert_channels};
use crate::ring::{RingProducer, ring_buffer};
use crate::shared::{PlaybackStatus, Session, Shared, TrackMetadata};
use crate::sink::{AudioSink, SinkConfig};
use crate::{Command, Event, TrackSource, UpcomingTrack};

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
/// Upcoming tracks kept pre-opened ahead of the current one. The front of
/// this window is the gapless successor; the rest exist so a skip forward
/// does not have to wait for a network open. Bounded deliberately: every
/// ready entry holds a decoder (and, for remote sources, a stream).
const PREFETCH_AHEAD: usize = 4;
/// Parallel opener threads for lookahead sources (explicit `Play` targets get
/// their own dedicated opener so they never queue behind prefetches).
const PREFETCH_WORKERS: usize = 2;

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

/// Which opener a request belongs to. Results are matched against the state
/// that requested them: a window request may only fill its slot, while a Play
/// request may only complete the pending Play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenKind {
    Window,
    Play,
}

/// One source open handed to an opener thread.
///
/// Opening a remote source is not cheap: the transport performs the initial
/// ranged GET, ReplayGain scans the headers (usually another GET), and
/// symphonia probes the container. Doing that inline on the producer blocks
/// decoding for the track that is currently playing — audible as a short
/// burst followed by an underrun. Opens therefore happen on opener threads;
/// results are installed when they arrive, and stale results (a slot removed,
/// reordered or re-requested meanwhile) are dropped by id + generation.
#[derive(Clone)]
struct PrefetchRequest {
    id: u64,
    /// Bumped on every re-request of the same slot; a result is only accepted
    /// by the request that is still current for the slot.
    generation: u64,
    kind: OpenKind,
    source: TrackSource,
}

struct PrefetchResult {
    request: PrefetchRequest,
    result: Result<PcmSource, AudioError>,
}

/// State of one prefetch-window entry.
enum SlotState {
    /// Not handed to an opener yet (waiting for a worker, or beyond the
    /// active lookahead window).
    Queued,
    /// An opener is working on this generation.
    Opening { generation: u64 },
    /// Opened and ready for a gapless hand-off.
    Ready(PcmSource),
    /// The latest open failed. Reaching the front stops the queue, matching
    /// the previous single-next behaviour.
    Failed,
}

/// One entry in the ordered prefetch window.
struct WindowSlot {
    id: u64,
    key: String,
    source: TrackSource,
    state: SlotState,
}

/// A `Play` waiting for its source to open without blocking the producer.
struct PendingPlay {
    id: u64,
    source: TrackSource,
}

/// Opener channels: explicit Play opens get their own thread so a slow
/// lookahead open can never delay a user-requested switch; lookahead opens
/// share a small pool.
struct Prefetcher {
    play_tx: Sender<PrefetchRequest>,
    window_tx: Sender<PrefetchRequest>,
    result_rx: Receiver<PrefetchResult>,
}

pub(crate) struct Producer {
    cmd_rx: Receiver<Command>,
    shared: Arc<Shared>,
    events: Arc<Events>,
    sink: Box<dyn AudioSink>,
    sink_cfg: SinkConfig,
    ring: Option<RingProducer>,
    current: Option<ActiveTrack>,
    /// Ordered upcoming tracks. `window[0]` is the gapless successor.
    window: Vec<WindowSlot>,
    /// A Play whose source is still opening; the current track keeps playing
    /// until the opener hands it over.
    pending_play: Option<PendingPlay>,
    /// The window was created by `EnqueueNext` on an idle engine, which is
    /// the one case where a ready window entry may start playback by itself.
    /// `SetUpcoming` only declares what would follow a current track.
    autoplay: bool,
    prefetcher: Prefetcher,
    /// Monotonic request generation, shared by all lookahead slots.
    prefetch_generation: u64,
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
        let prefetcher = spawn_prefetcher()?;
        let mut producer = Self {
            cmd_rx,
            shared,
            events,
            sink,
            sink_cfg: default_cfg,
            ring: None,
            current: None,
            window: Vec::new(),
            pending_play: None,
            autoplay: false,
            prefetcher,
            prefetch_generation: 0,
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
        if self.current.is_some() || self.autoplay || self.pending_play.is_some() || self.draining {
            Duration::from_millis(5)
        } else {
            // An idle window (declared by `SetUpcoming`) may still be opening;
            // a 100 ms poll is plenty and avoids busy-looping while it waits.
            Duration::from_millis(100)
        }
    }

    /// Do one unit of work. Returns `true` if there is likely more work now.
    fn tick(&mut self) -> bool {
        let mut worked = self.poll_prefetch();

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
            // An empty window means the queue is over. A failed front entry
            // is terminal too (matching the previous single-next failure
            // behaviour); a queued/opening entry means "wait for the opener".
            if self.pending_play.is_none() && (self.window.is_empty() || self.window_front_failed())
            {
                self.draining = true;
                self.drain_started = Some(Instant::now());
            }
            worked = true;
        }

        // Swap: the current track is done and the window front is pre-opened
        // (gapless), or EnqueueNext asked an idle engine to start. A bare
        // SetUpcoming may pre-open sources but must never start playback by
        // itself. A pending explicit Play always wins: never auto-advance
        // past the track the user asked for while its open is still in flight.
        let current_done =
            self.current.as_ref().is_some_and(|t| t.announced) && self.pending_out.is_empty();
        let idle_start = self.current.is_none() && self.autoplay;
        if (current_done || idle_start)
            && self.pending_play.is_none()
            && let Some(q) = self.take_ready_front()
        {
            self.autoplay = false;
            self.current = None;
            self.draining = false;
            self.drain_started = None;
            self.start_track(q);
            self.prefill();
            if self.shared.status() == PlaybackStatus::Stopped {
                self.shared.set_status(PlaybackStatus::Playing);
            }
            self.pump_prefetch();
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

    /// Install completed opens, if they still belong to the state that
    /// requested them. A newer `SetUpcoming`/`Play`/`Stop` may have removed or
    /// re-requested a slot while the opener was busy with the network.
    fn poll_prefetch(&mut self) -> bool {
        let mut worked = false;
        while let Ok(result) = self.prefetcher.result_rx.try_recv() {
            worked = true;
            let PrefetchResult { request, result } = result;
            match request.kind {
                OpenKind::Play => {
                    // A Play request completes the pending Play whether the
                    // source came from the dedicated opener or a window slot
                    // that was promoted to Play.
                    if self
                        .pending_play
                        .as_ref()
                        .is_none_or(|p| p.id != request.id)
                    {
                        continue;
                    }
                    match result {
                        Ok(src) => {
                            let pending = self.pending_play.take().expect("checked above");
                            self.install_play(QueuedTrack {
                                id: pending.id,
                                source: pending.source,
                                src,
                            });
                        }
                        Err(e) => {
                            self.pending_play = None;
                            // The window belongs to the failed target's queue;
                            // do not let it advance the old track.
                            self.window.clear();
                            self.events.emit(Event::Error {
                                id: Some(request.id),
                                msg: e.msg(),
                            });
                            if self.current.as_ref().is_some_and(|t| t.announced) {
                                self.draining = true;
                                self.drain_started = Some(Instant::now());
                            }
                        }
                    }
                }
                OpenKind::Window => {
                    let Some(slot) = self.window.iter_mut().find(|s| s.id == request.id) else {
                        continue;
                    };
                    let SlotState::Opening { generation } = slot.state else {
                        continue;
                    };
                    if generation != request.generation {
                        // Superseded by a newer request for the same slot.
                        continue;
                    }
                    match result {
                        Ok(src) => slot.state = SlotState::Ready(src),
                        Err(e) => {
                            slot.state = SlotState::Failed;
                            let id = request.id;
                            let msg = e.msg();
                            self.events.emit(Event::Error { id: Some(id), msg });
                        }
                    }
                }
            }
        }

        if self
            .current
            .as_ref()
            .is_some_and(|t| t.announced && self.pending_out.is_empty())
            && self.pending_play.is_none()
            && self.window_front_failed()
            && !self.draining
        {
            self.draining = true;
            self.drain_started = Some(Instant::now());
        }
        // An idle engine with a failed window front has nothing to play and
        // no boundary at which to drain; drop the dead window so the producer
        // can go back to sleep.
        if self.current.is_none() && self.pending_play.is_none() && self.window_front_failed() {
            self.window.clear();
            self.shared.set_status(PlaybackStatus::Stopped);
        }

        self.pump_prefetch();
        worked
    }

    /// Hand queued window entries to the opener pool, up to the active
    /// lookahead depth and worker count.
    fn pump_prefetch(&mut self) {
        let mut opening = self
            .window
            .iter()
            .filter(|s| matches!(s.state, SlotState::Opening { .. }))
            .count();
        for idx in 0..self.window.len().min(PREFETCH_AHEAD) {
            if opening >= PREFETCH_WORKERS {
                break;
            }
            if !matches!(self.window[idx].state, SlotState::Queued) {
                continue;
            }
            self.prefetch_generation += 1;
            let generation = self.prefetch_generation;
            let request = PrefetchRequest {
                id: self.window[idx].id,
                generation,
                kind: OpenKind::Window,
                source: self.window[idx].source.clone(),
            };
            self.window[idx].state = SlotState::Opening { generation };
            opening += 1;
            if self.prefetcher.window_tx.send(request.clone()).is_err() {
                // Opener pool gone (should not happen while the producer
                // lives). Fall back to opening inline rather than wedging.
                match open_source(&request.source) {
                    Ok(src) => self.window[idx].state = SlotState::Ready(src),
                    Err(e) => {
                        self.window[idx].state = SlotState::Failed;
                        self.events.emit(Event::Error {
                            id: Some(request.id),
                            msg: e.msg(),
                        });
                    }
                }
            }
        }
    }

    /// Remove and return the window front if it is already open.
    fn take_ready_front(&mut self) -> Option<QueuedTrack> {
        if !matches!(
            self.window.first().map(|s| &s.state),
            Some(SlotState::Ready(_))
        ) {
            return None;
        }
        let slot = self.window.remove(0);
        let SlotState::Ready(src) = slot.state else {
            unreachable!("checked above");
        };
        Some(QueuedTrack {
            id: slot.id,
            source: slot.source,
            src,
        })
    }

    fn window_front_failed(&self) -> bool {
        self.window
            .first()
            .is_some_and(|s| matches!(s.state, SlotState::Failed))
    }

    // ----- command handling -----

    /// Returns `true` when the engine should shut down.
    fn handle_command(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Play(source) => self.cmd_play(source, None),
            Command::PlayKeyed { key, source } => self.cmd_play(source, Some(key)),
            Command::EnqueueNext(source) => self.cmd_enqueue(source),
            Command::SetUpcoming(tracks) => self.cmd_set_upcoming(tracks),
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

    /// Play `source`, reusing the prefetched window entry whose key (or, for
    /// the legacy unkeyed command, source) matches.
    fn cmd_play(&mut self, source: TrackSource, key: Option<String>) {
        self.autoplay = false;
        let matched = self.window.iter().position(|slot| match key.as_deref() {
            Some(k) => slot.key == k,
            None => slot.source == source,
        });

        if let Some(idx) = matched {
            let slot = self.window.remove(idx);
            // A new Play supersedes any target that was still opening.
            self.pending_play = None;
            match slot.state {
                SlotState::Ready(src) => {
                    // Instant switch: no open, no network. Keep the entries
                    // behind it — they are exactly the new queue's lookahead
                    // when this was a skip to a prefetched track.
                    self.install_play(QueuedTrack {
                        id: slot.id,
                        source: slot.source,
                        src,
                    });
                }
                SlotState::Queued | SlotState::Opening { .. } | SlotState::Failed => {
                    // Promote the entry to a Play. The dedicated Play opener
                    // starts it now instead of waiting behind lookahead opens;
                    // any in-flight window request for the slot is ignored.
                    let play_source = if slot.source == source {
                        slot.source
                    } else {
                        // Remote URLs are resolved per feed and may be
                        // refreshed; prefer the caller's current one.
                        source
                    };
                    self.begin_play(slot.id, play_source);
                }
            }
            return;
        }

        // Not prefetched: this is a queue-replacing Play, so drop the old
        // lookahead. Idle playback opens inline for an immediate first frame;
        // while something is playing the old track stays audible until the
        // dedicated opener hands over the new source.
        self.window.clear();
        self.pending_play = None;
        if self.current.is_none() {
            let id = self.alloc_id();
            match open_source(&source) {
                Ok(src) => self.install_play(QueuedTrack { id, source, src }),
                Err(e) => {
                    // Report and keep the engine stopped.
                    self.events.emit(Event::Error {
                        id: Some(id),
                        msg: e.msg(),
                    });
                    self.shared.set_status(PlaybackStatus::Stopped);
                }
            }
            return;
        }
        let id = self.alloc_id();
        self.begin_play(id, source);
    }

    /// Open `source` on the dedicated Play opener, keeping the current track
    /// audible until the hand-off.
    fn begin_play(&mut self, id: u64, source: TrackSource) {
        self.pending_play = Some(PendingPlay {
            id,
            source: source.clone(),
        });
        self.draining = false;
        self.drain_started = None;
        let request = PrefetchRequest {
            id,
            generation: 0,
            kind: OpenKind::Play,
            source: source.clone(),
        };
        if self.prefetcher.play_tx.send(request).is_err() {
            // Opener thread gone (should not happen while the producer
            // lives). Fall back to the old inline behavior.
            self.pending_play = None;
            match open_source(&source) {
                Ok(src) => self.install_play(QueuedTrack { id, source, src }),
                Err(e) => {
                    self.window.clear();
                    self.events.emit(Event::Error {
                        id: Some(id),
                        msg: e.msg(),
                    });
                    if self.current.as_ref().is_some_and(|t| t.announced) {
                        self.draining = true;
                        self.drain_started = Some(Instant::now());
                    }
                }
            }
        }
    }

    /// Replace the current track with an already-open source. The prefetch
    /// window is intentionally left alone: callers clear it for a
    /// queue-replacing Play, while a skip to a prefetched entry keeps the
    /// rest of the window as the new lookahead.
    fn install_play(&mut self, q: QueuedTrack) {
        if let Some(old) = self.current.take() {
            // The old track ends without ceremony (it was replaced, not finished).
            if !old.announced {
                self.events.emit(Event::TrackFinished { id: old.id });
            }
        }
        self.pending_play = None;
        self.autoplay = false;
        self.draining = false;
        self.drain_started = None;
        self.hard_reset();

        // Prefer running the device at the source rate (bit-exact, no resample).
        // Gapless transitions never re-open the device — only explicit Play does.
        let desired = SinkConfig {
            sample_rate: q.src.rate(),
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

        self.start_track(q);
        self.prefill();
        self.shared.set_status(PlaybackStatus::Playing);
        self.pump_prefetch();
    }

    fn cmd_enqueue(&mut self, source: TrackSource) {
        if self.current.is_none() && self.pending_play.is_none() {
            // Idle: EnqueueNext starts playback immediately, exactly like Play.
            self.window.clear();
            let id = self.alloc_id();
            match open_source(&source) {
                Ok(src) => {
                    self.window.push(WindowSlot {
                        id,
                        key: format!("enqueue-{id}"),
                        source,
                        state: SlotState::Ready(src),
                    });
                    // EnqueueNext-on-idle is the one command that starts
                    // playback without a current track.
                    self.autoplay = true;
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
            return;
        }
        // While playing, EnqueueNext keeps its historical "replace the next
        // slot" meaning: a one-entry window (opened off-thread).
        let key = format!("enqueue-{}", self.next_track_id + 1);
        self.cmd_set_upcoming(vec![UpcomingTrack { key, source }]);
    }

    /// Replace the prefetch window, carrying over entries whose keys survive.
    fn cmd_set_upcoming(&mut self, tracks: Vec<UpcomingTrack>) {
        let mut old: Vec<Option<WindowSlot>> = std::mem::take(&mut self.window)
            .into_iter()
            .map(Some)
            .collect();
        let mut window = Vec::with_capacity(tracks.len());
        for track in tracks {
            let found = old
                .iter()
                .position(|slot| slot.as_ref().is_some_and(|s| s.key == track.key));
            if let Some(pos) = found {
                let mut slot = old[pos].take().expect("found above");
                match slot.state {
                    SlotState::Ready(_) => {
                        // Already open: keep it even if the caller's URL was
                        // refreshed; the decoded stream is what matters.
                    }
                    SlotState::Queued => slot.source = track.source,
                    SlotState::Failed => {
                        // Retry only when a genuinely new source was supplied
                        // (e.g. a refreshed token), not for the same failure.
                        if slot.source != track.source {
                            slot.source = track.source;
                            slot.state = SlotState::Queued;
                        }
                    }
                    SlotState::Opening { .. } => {
                        if slot.source != track.source {
                            // Abandon the stale open; request the new source.
                            // The old result is ignored by generation.
                            slot.source = track.source;
                            slot.state = SlotState::Queued;
                        }
                    }
                }
                window.push(slot);
            } else {
                let id = self.alloc_id();
                window.push(WindowSlot {
                    id,
                    key: track.key,
                    source: track.source,
                    state: SlotState::Queued,
                });
            }
        }
        self.window = window;
        // A window that still has a viable entry rescues a queue that was
        // about to drain (late EnqueueNext replacement). Only while a track
        // is still resident: a window arriving after EndOfQueue must not
        // silently restart playback.
        if self.current.is_some()
            && self
                .window
                .iter()
                .any(|s| !matches!(s.state, SlotState::Failed))
        {
            self.draining = false;
            self.drain_started = None;
        }
        self.pump_prefetch();
    }

    fn cmd_stop(&mut self) {
        self.current = None;
        self.window.clear();
        self.pending_play = None;
        self.autoplay = false;
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
        self.window.clear();
        self.pending_play = None;
        self.autoplay = false;
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
        TrackSource::RemoteWithHeaders { url, headers } => {
            PcmSource::open_remote_with_headers(url, headers)
        }
    }
}

/// Spawn the opener threads and return their request/result ends.
///
/// One dedicated thread serves explicit `Play` targets so they never queue
/// behind a slow lookahead open; a small pool serves the prefetch window.
/// The threads live as long as the engine: when the producer (and with it the
/// request senders) is dropped, `recv()` returns and the workers exit. They
/// are deliberately *not* joined — an in-flight HTTP open must not delay app
/// shutdown, and a detached result simply fails to send.
fn spawn_prefetcher() -> std::io::Result<Prefetcher> {
    let (play_tx, play_rx) = crossbeam_channel::unbounded::<PrefetchRequest>();
    let (window_tx, window_rx) = crossbeam_channel::unbounded::<PrefetchRequest>();
    let (result_tx, result_rx) = crossbeam_channel::unbounded::<PrefetchResult>();

    spawn_opener(
        "navinamp-audio-play-opener",
        play_rx,
        result_tx.clone(),
        // Rapid Play clicks supersede each other: only the newest queued one
        // can still be wanted, so skip the stale opens.
        true,
    )?;
    for worker in 0..PREFETCH_WORKERS {
        spawn_opener(
            &format!("navinamp-audio-prefetch-{worker}"),
            window_rx.clone(),
            result_tx.clone(),
            // Every window request may fill a distinct slot.
            false,
        )?;
    }
    drop(result_tx);

    Ok(Prefetcher {
        play_tx,
        window_tx,
        result_rx,
    })
}

fn spawn_opener(
    name: &str,
    request_rx: Receiver<PrefetchRequest>,
    result_tx: Sender<PrefetchResult>,
    newest_only: bool,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            while let Ok(request) = request_rx.recv() {
                let mut request = request;
                if newest_only {
                    while let Ok(newer) = request_rx.try_recv() {
                        request = newer;
                    }
                }
                let result = open_source(&request.source);
                if result_tx.send(PrefetchResult { request, result }).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}
