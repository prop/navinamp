//! Output abstraction.
//!
//! Production uses [`crate::sink_cpal::CpalSink`] (default output device). Tests
//! use [`crate::sink_test::TestSink`], which is driven synchronously by
//! `pull()` calls instead of hardware interrupts, making the engine fully
//! testable without audio hardware.

use crate::error::AudioError;

/// The stream configuration the engine runs a sink at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count (interleaved).
    pub channels: u16,
}

/// The pull callback: `out` is interleaved f32, `out.len()` is a multiple of the
/// channel count. Implementations call it from the device interrupt (or a test
/// loop). It never blocks, locks, or allocates.
pub type AudioCallback = Box<dyn FnMut(&mut [f32]) + Send + 'static>;

/// Sink error reporter (invoked off-thread by the device backend).
pub type SinkErrorCallback = Box<dyn FnMut(String) + Send + 'static>;

/// A pull-mode audio output. The sink object itself is a factory owned by the
/// producer thread; [`AudioSink::open`] (re)builds the running stream and starts
/// it immediately. Opening a new stream stops the previous one.
pub trait AudioSink: Send + 'static {
    /// The configuration used when no better option exists (device default).
    fn default_config(&self) -> Result<SinkConfig, AudioError>;

    /// Open (or re-open) the output at `cfg` and drive `cb`. Fails with
    /// [`AudioError::UnsupportedConfig`] if the device cannot run at `cfg`.
    fn open(
        &mut self,
        cfg: SinkConfig,
        cb: AudioCallback,
        err_cb: SinkErrorCallback,
    ) -> Result<(), AudioError>;
}
