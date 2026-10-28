//! Symphonia-based PCM source: probe by content, decode to interleaved f32,
//! sample-accurate seek.
//!
//! One [`PcmSource`] owns a `FormatReader` + `Decoder` pair for a single file.
//! It lives on the producer thread only. The next track in the queue is opened
//! eagerly (`EnqueueNext`) so the gapless swap never waits on file I/O.

use std::fs::File;
use std::io;
use std::path::Path;

use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};
use symphonia::default::{get_codecs, get_probe};

use crate::error::AudioError;

/// A decoded audio source. Yields interleaved f32 blocks at the source sample
/// rate / channel count.
pub(crate) struct PcmSource {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    spec: SignalSpec,
    time_base: TimeBase,
    /// Duration from container metadata (n_frames / rate), if the container knows.
    pub duration_hint_secs: Option<f64>,
    /// Exact frames decoded so far (media time).
    decoded_frames: u64,
    /// f32 interleaving buffer, grown to the largest packet seen.
    sample_buf: SampleBuffer<f32>,
    sample_buf_frames: u64,
    /// After a seek: drop decoded frames until this track timestamp.
    skip_until_ts: Option<u64>,
}

impl PcmSource {
    /// Open and probe a media file by content (no extension hints).
    pub(crate) fn open(path: &Path) -> Result<Self, AudioError> {
        let file = File::open(path).map_err(AudioError::Io)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        // Enable gapless so the reader provides encoder delay/padding trim info.
        let fmt_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let probed = get_probe()
            .format(&Hint::new(), mss, &fmt_opts, &MetadataOptions::default())
            .map_err(|e| AudioError::UnsupportedFormat(format!("probe failed: {e}")))?;

        let reader = probed.format;
        let track = reader
            .tracks()
            .iter()
            .find(|t| {
                t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some()
            })
            .ok_or_else(|| AudioError::UnsupportedFormat("no decodable audio track".to_string()))?;

        let track_id = track.id;
        let rate = track.codec_params.sample_rate.expect("checked above");
        let channels = track
            .codec_params
            .channels
            .ok_or_else(|| AudioError::UnsupportedFormat("unknown channel layout".into()))?;
        let spec = SignalSpec::new(rate, channels);
        let time_base = track
            .codec_params
            .time_base
            .unwrap_or(TimeBase::new(1, rate));
        let duration_hint_secs = track
            .codec_params
            .n_frames
            .map(|n| n as f64 / f64::from(rate));

        let decoder = get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| AudioError::UnsupportedFormat(format!("decoder init failed: {e}")))?;

        let initial_frames = 8192;
        Ok(Self {
            reader,
            decoder,
            track_id,
            spec,
            time_base,
            duration_hint_secs,
            decoded_frames: 0,
            sample_buf: SampleBuffer::new(initial_frames, spec),
            sample_buf_frames: initial_frames,
            skip_until_ts: None,
        })
    }

    pub(crate) fn rate(&self) -> u32 {
        self.spec.rate
    }

    pub(crate) fn channels(&self) -> usize {
        self.spec.channels.count()
    }

    pub(crate) fn decoded_frames(&self) -> u64 {
        self.decoded_frames
    }

    /// Decode the next packet into interleaved f32 and append to `out`.
    ///
    /// Returns `Ok(true)` if samples were appended, `Ok(false)` on clean EOF.
    /// A recoverable bitstream error skips the packet; anything else is returned
    /// as a message suitable for [`crate::Event::Error`].
    pub(crate) fn next_block(&mut self, out: &mut Vec<f32>) -> Result<bool, String> {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(ref e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(false);
                }
                // For file sources a reset is equivalent to end-of-stream.
                Err(SymError::ResetRequired) => return Ok(false),
                Err(e) => return Err(format!("demuxer error: {e}")),
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            // Post-seek alignment: skip packets entirely before the seek target.
            let mut trim_to_ts = None;
            if let Some(skip_ts) = self.skip_until_ts {
                if packet.ts() + packet.dur() <= skip_ts {
                    continue;
                }
                if packet.ts() < skip_ts {
                    trim_to_ts = Some(skip_ts);
                } else {
                    self.skip_until_ts = None;
                }
            }

            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                // Corrupt packet: skip it and keep going (best effort).
                Err(SymError::DecodeError(_)) => continue,
                Err(e) => return Err(format!("decode error: {e}")),
            };

            let frames = decoded.frames();
            if frames == 0 {
                continue;
            }

            // Grow the conversion buffer if this packet is larger than any before.
            if frames as u64 > self.sample_buf_frames {
                self.sample_buf = SampleBuffer::new(frames as u64, self.spec);
                self.sample_buf_frames = frames as u64;
            }
            self.sample_buf.copy_interleaved_ref(decoded);
            let samples = self.sample_buf.samples();

            // Drop the frames before the seek target inside the first packet.
            let mut keep_from = 0;
            if let Some(skip_ts) = trim_to_ts {
                self.skip_until_ts = None;
                let ticks = skip_ts - packet.ts();
                let secs = ticks as f64 * f64::from(self.time_base.numer)
                    / f64::from(self.time_base.denom);
                let drop_frames = (secs * f64::from(self.spec.rate)).round() as usize;
                keep_from = drop_frames * self.spec.channels.count();
            }

            if keep_from >= samples.len() {
                continue;
            }
            out.extend_from_slice(&samples[keep_from..]);
            self.decoded_frames +=
                ((samples.len() - keep_from) / self.spec.channels.count()) as u64;
            return Ok(true);
        }
    }

    /// Seek to `secs` (media time). Returns the actual position reached.
    /// Best effort: tries sample-accurate first, falls back to coarse, and never
    /// panics. On failure the playback position is unchanged.
    pub(crate) fn seek(&mut self, secs: f64) -> Result<f64, String> {
        let max = self.duration_hint_secs.unwrap_or(f64::INFINITY);
        let target = secs.clamp(0.0, max);
        let make_seek_to = || SeekTo::Time {
            time: Time::from(target),
            track_id: Some(self.track_id),
        };

        let seeked = match self.reader.seek(SeekMode::Accurate, make_seek_to()) {
            Ok(s) => s,
            Err(e1) => match self.reader.seek(SeekMode::Coarse, make_seek_to()) {
                Ok(s) => s,
                Err(e2) => {
                    return Err(format!("seek failed (accurate: {e1}; coarse: {e2})"));
                }
            },
        };

        // The decoder must be rebuilt/reset after a seek.
        self.decoder.reset();
        self.skip_until_ts = Some(seeked.required_ts);

        let t = self.time_base.calc_time(seeked.required_ts);
        Ok(t.seconds as f64 + t.frac)
    }
}
