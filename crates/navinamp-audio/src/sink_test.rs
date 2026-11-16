//! Hardware-free sink for tests and offline rendering.
//!
//! A [`TestSink`] is `Clone`: hand one copy to
//! [`crate::EngineHandle::start_with_sink`] and keep another to *drive* playback
//! by calling [`TestSink::pull`], exactly like a device interrupt would. The sink
//! refuses configurations other than the one it was built with, which is how the
//! resample path is exercised deterministically (e.g. a 48 kHz file into a
//! 44.1 kHz sink).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::error::AudioError;
use crate::sink::{AudioCallback, AudioSink, SinkConfig, SinkErrorCallback};

struct TestInner {
    cfg: SinkConfig,
    state: Mutex<TestState>,
    /// Mirrors the last [`AudioSink::set_output_paused`] call, so tests can
    /// assert the engine's suspend/resume lifecycle (idle device suspended,
    /// playing device live).
    paused: AtomicBool,
}

struct TestState {
    cb: Option<AudioCallback>,
    open_count: u64,
}

/// A pull-driven, cloneable sink for tests.
#[derive(Clone)]
pub struct TestSink {
    inner: std::sync::Arc<TestInner>,
}

impl TestSink {
    /// Create a stereo test sink at `sample_rate`.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            inner: std::sync::Arc::new(TestInner {
                cfg: SinkConfig {
                    sample_rate,
                    channels: 2,
                },
                state: Mutex::new(TestState {
                    cb: None,
                    open_count: 0,
                }),
                paused: AtomicBool::new(false),
            }),
        }
    }

    /// Whether the engine last asked this sink to suspend its output. A real
    /// device drops its OS power assertion while suspended; tests assert this
    /// flag instead of measuring power.
    pub fn suspended(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }

    /// The fixed configuration this sink accepts.
    pub fn config(&self) -> SinkConfig {
        self.inner.cfg
    }

    /// How many times the engine (re)opened this sink.
    pub fn open_count(&self) -> u64 {
        self.inner.state.lock().unwrap().open_count
    }

    /// Pull `frames` frames through the engine callback (like a device period).
    ///
    /// The pull is **paced to real time** (a `frames/rate` duration), because a
    /// real device consumes audio at 1×; unpaced pulling would drain the ring
    /// faster than any producer can decode and show phantom underruns.
    /// Returns the interleaved samples "played".
    pub fn pull(&self, frames: usize) -> Vec<f32> {
        let start = std::time::Instant::now();
        let mut state = self.inner.state.lock().unwrap();
        let mut out = vec![0.0; frames * self.inner.cfg.channels as usize];
        if let Some(cb) = state.cb.as_mut() {
            cb(&mut out);
        }
        drop(state);
        let target = Duration::from_secs_f64(frames as f64 / f64::from(self.inner.cfg.sample_rate));
        let elapsed = start.elapsed();
        if target > elapsed {
            std::thread::sleep(target - elapsed);
        }
        out
    }
}

impl AudioSink for TestSink {
    fn default_config(&self) -> Result<SinkConfig, AudioError> {
        Ok(self.inner.cfg)
    }

    fn open(
        &mut self,
        cfg: SinkConfig,
        cb: AudioCallback,
        _err_cb: SinkErrorCallback,
    ) -> Result<(), AudioError> {
        if cfg != self.inner.cfg {
            return Err(AudioError::UnsupportedConfig(format!(
                "test sink is fixed at {} Hz / {}ch",
                self.inner.cfg.sample_rate, self.inner.cfg.channels
            )));
        }
        let mut state = self.inner.state.lock().unwrap();
        state.cb = Some(cb);
        state.open_count += 1;
        Ok(())
    }

    fn set_output_paused(&mut self, paused: bool) {
        self.inner.paused.store(paused, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for TestSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestSink")
            .field("cfg", &self.inner.cfg)
            .finish()
    }
}
