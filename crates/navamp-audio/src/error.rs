//! Error type for the audio engine.

/// Errors produced while opening devices, decoding tracks, or talking to the engine.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// An I/O error (file open/read failure, etc.).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// No usable audio output device is present.
    #[error("no audio output device available")]
    NoOutputDevice,

    /// The output device rejected the requested stream configuration.
    #[error("unsupported stream config: {0}")]
    UnsupportedConfig(String),

    /// The media container/codec could not be probed or decoded.
    #[error("unsupported or broken media: {0}")]
    UnsupportedFormat(String),

    /// The resampler rejected a configuration or failed mid-stream.
    #[error("resampler error: {0}")]
    Resample(String),

    /// The engine thread is no longer running (after [`Command::Shutdown`] or a drop).
    #[error("audio engine is not running")]
    EngineStopped,
}

impl AudioError {
    pub(crate) fn msg(self) -> String {
        self.to_string()
    }
}
