//! cpal-backed sink: F32 streams on the default output device.
//!
//! The engine always requests interleaved f32 at stereo. If the device rejects
//! the requested rate, [`CpalSink::open`] fails and the engine keeps the already
//! open stream, resampling on the decode thread instead (see [`crate::resample`]).

use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use crate::error::AudioError;
use crate::sink::{AudioCallback, AudioSink, SinkConfig, SinkErrorCallback};

/// Sink over the system default output device (via cpal).
pub struct CpalSink {
    device: cpal::Device,
    stream: Option<Stream>,
    open_cfg: Option<SinkConfig>,
}

impl CpalSink {
    /// Capture the default output device.
    pub fn new() -> Result<Self, AudioError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice)?;
        Ok(Self {
            device,
            stream: None,
            open_cfg: None,
        })
    }

    /// Name of the output device (diagnostics).
    pub fn device_name(&self) -> String {
        self.device.to_string()
    }
}

impl AudioSink for CpalSink {
    fn default_config(&self) -> Result<SinkConfig, AudioError> {
        let cfg = self
            .device
            .default_output_config()
            .map_err(|e| AudioError::UnsupportedConfig(format!("default output config: {e}")))?;
        if cfg.sample_format() != SampleFormat::F32 {
            // Search for any supported F32 configuration; virtually all devices
            // expose one.
            let mut ranges = self
                .device
                .supported_output_configs()
                .map_err(|e| AudioError::UnsupportedConfig(format!("querying configs: {e}")))?;
            return ranges
                .find(|r| r.sample_format() == SampleFormat::F32)
                .map(|r| {
                    let c = r.with_max_sample_rate();
                    SinkConfig {
                        sample_rate: c.sample_rate(),
                        channels: c.channels(),
                    }
                })
                .ok_or_else(|| {
                    AudioError::UnsupportedConfig("device does not support f32 output".to_string())
                });
        }
        Ok(SinkConfig {
            sample_rate: cfg.sample_rate(),
            channels: cfg.channels(),
        })
    }

    fn open(
        &mut self,
        cfg: SinkConfig,
        mut cb: AudioCallback,
        mut err_cb: SinkErrorCallback,
    ) -> Result<(), AudioError> {
        // Locate a supported F32 range matching channels + rate.
        let ranges = self
            .device
            .supported_output_configs()
            .map_err(|e| AudioError::UnsupportedConfig(format!("querying configs: {e}")))?;
        let rate = cfg.sample_rate;
        let channels = cfg.channels;
        let range = ranges
            .into_iter()
            .filter(|r| r.sample_format() == SampleFormat::F32 && r.channels() == channels)
            .find(|r| rate >= r.min_sample_rate() && rate <= r.max_sample_rate())
            .ok_or_else(|| {
                AudioError::UnsupportedConfig(format!("{rate} Hz / {channels}ch f32"))
            })?;

        let stream_cfg: StreamConfig = range.with_sample_rate(rate).into();

        // Stop the previous stream before building the new one so at most one
        // callback is ever alive (old and new rings must never both consume).
        self.stream = None;

        let stream = self
            .device
            .build_output_stream(
                stream_cfg,
                move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| cb(data),
                move |err| err_cb(format!("output stream error: {err}")),
                Some(Duration::from_secs(5)),
            )
            .map_err(|e| AudioError::UnsupportedConfig(format!("open stream: {e}")))?;

        stream
            .play()
            .map_err(|e| AudioError::UnsupportedConfig(format!("start stream: {e}")))?;

        self.stream = Some(stream);
        self.open_cfg = Some(cfg);
        Ok(())
    }
}

impl std::fmt::Debug for CpalSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpalSink")
            .field("device", &self.device_name())
            .field("open_cfg", &self.open_cfg)
            .finish()
    }
}
