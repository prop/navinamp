//! Symphonia-based PCM source: probe by content, decode to interleaved f32,
//! sample-accurate seek.
//!
//! One [`PcmSource`] owns a `FormatReader` + `Decoder` pair for a single file.
//! It lives on the producer thread only. The next track in the queue is opened
//! eagerly (`EnqueueNext`) so the gapless swap never waits on file I/O.

use std::fs::File;
use std::io::{self, Seek};
use std::path::Path;

use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};
use symphonia::default::{get_codecs, get_probe};

use crate::error::AudioError;
use crate::http_stream::HttpStream;
use crate::replaygain;

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
    /// ReplayGain scalar to apply to this track (album gain preferred, track
    /// gain fallback, peak-protected; 1.0 when untagged). Read once at open
    /// and published by the producer as a frame-accurate switch marker.
    pub replaygain_gain: f32,
    /// Exact frames decoded so far (media time).
    decoded_frames: u64,
    /// Average bitrate of the source (kbps), when size and duration are known.
    pub bitrate_kbps: Option<u32>,
    /// f32 interleaving buffer, grown to the largest packet seen.
    sample_buf: SampleBuffer<f32>,
    sample_buf_frames: u64,
    /// Samples decoded while priming the decoder for containers that don't
    /// declare a channel layout (M4A/AAC) — served before anything else.
    primed: Vec<f32>,
    /// After a seek: drop decoded frames until this track timestamp.
    skip_until_ts: Option<u64>,
}

impl PcmSource {
    /// Open and probe a media file by content (no extension hints). The
    /// file's ReplayGain tags are read in the same pass (lofty, best-effort).
    pub(crate) fn open(path: &Path) -> Result<Self, AudioError> {
        let mut file = File::open(path).map_err(AudioError::Io)?;
        let source_len = file.metadata().ok().map(|m| m.len());
        let replaygain_gain = replaygain::scan(&mut file);
        file.rewind().map_err(AudioError::Io)?;
        Self::probe(Box::new(file), replaygain_gain, source_len)
    }

    /// Open and probe a remote HTTP(S) media stream by content. The
    /// [`HttpStream`] performs the initial ranged GET here (on the producer
    /// thread); duration/seek work exactly as for files once the transport
    /// is seekable (range-capable server, or spilled to a temp file).
    /// ReplayGain tags are read before the hand-off to symphonia (one extra
    /// ranged GET on range-capable servers).
    pub(crate) fn open_remote(url: &str) -> Result<Self, AudioError> {
        let mut stream = HttpStream::open(url).map_err(AudioError::Io)?;
        let source_len = Some(stream.total_len());
        let replaygain_gain = replaygain::scan(&mut stream);
        stream.rewind().map_err(AudioError::Io)?;
        Self::probe(Box::new(stream), replaygain_gain, source_len)
    }

    /// Probe `source` (a `Read + Seek` media source) and build the decoder.
    fn probe(
        source: Box<dyn MediaSource>,
        replaygain_gain: f32,
        source_len: Option<u64>,
    ) -> Result<Self, AudioError> {
        let mss = MediaSourceStream::new(source, Default::default());

        // Enable gapless so the reader provides encoder delay/padding trim info.
        let fmt_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let probed = get_probe()
            .format(&Hint::new(), mss, &fmt_opts, &MetadataOptions::default())
            .map_err(|e| AudioError::UnsupportedFormat(format!("probe failed: {e}")))?;

        let mut reader = probed.format;
        let track = reader
            .tracks()
            .iter()
            .find(|t| {
                t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some()
            })
            .ok_or_else(|| AudioError::UnsupportedFormat("no decodable audio track".to_string()))?;

        let track_id = track.id;
        let rate = track.codec_params.sample_rate.expect("checked above");
        let time_base = track
            .codec_params
            .time_base
            .unwrap_or(TimeBase::new(1, rate));
        let duration_hint_secs = track
            .codec_params
            .n_frames
            .map(|n| n as f64 / f64::from(rate));
        // Average bitrate: source bytes over media duration. This is the
        // classic "kbps" readout (VBR files show their average). Unknown
        // duration or size (rare) leaves it blank rather than guessing.
        let bitrate_kbps = source_len
            .zip(duration_hint_secs)
            .and_then(|(bytes, secs)| average_kbps(bytes, secs));

        let mut decoder = get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| AudioError::UnsupportedFormat(format!("decoder init failed: {e}")))?;

        // Some containers (M4A/AAC, ALAC) don't declare a channel layout in
        // the track header — the real signal spec is only knowable after
        // decoding the first frame. Prime the decoder in that case instead
        // of failing: the primed samples are real media from position 0 and
        // are served before anything else (see `next_block`).
        let mut primed_interleaved: Vec<f32> = Vec::new();
        let spec = match track.codec_params.channels {
            Some(ch) => SignalSpec::new(rate, ch),
            None => {
                let mut learned: Option<SignalSpec> = None;
                let mut attempts = 0u32;
                while learned.is_none() && attempts < 100 {
                    attempts += 1;
                    let packet = match reader.next_packet() {
                        Ok(p) => p,
                        Err(_) => break,
                    };
                    if packet.track_id() != track_id {
                        continue;
                    }
                    let decoded = match decoder.decode(&packet) {
                        Ok(d) => d,
                        Err(SymError::DecodeError(_)) => continue,
                        Err(_) => break,
                    };
                    let frames = decoded.frames();
                    if frames == 0 {
                        continue;
                    }
                    learned = Some(*decoded.spec());
                    let mut sb = SampleBuffer::<f32>::new(frames as u64, *decoded.spec());
                    sb.copy_interleaved_ref(decoded);
                    primed_interleaved.extend_from_slice(sb.samples());
                }
                match learned {
                    Some(spec) => spec,
                    None => {
                        return Err(AudioError::UnsupportedFormat(
                            "unknown channel layout (decoder produced no audio to learn it from)"
                                .into(),
                        ));
                    }
                }
            }
        };
        let channels_count = spec.channels.count().max(1);

        let initial_frames = 8192;
        Ok(Self {
            reader,
            decoder,
            track_id,
            spec,
            time_base,
            duration_hint_secs,
            replaygain_gain,
            decoded_frames: (primed_interleaved.len() / channels_count) as u64,
            bitrate_kbps,
            sample_buf: SampleBuffer::new(initial_frames, spec),
            sample_buf_frames: initial_frames,
            primed: primed_interleaved,
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
        // Primed samples (channel-layout discovery decode) are real media
        // from position 0 — serve them before pulling more packets.
        if !self.primed.is_empty() {
            let primed = std::mem::take(&mut self.primed);
            out.extend_from_slice(&primed);
            return Ok(true);
        }
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
        self.primed.clear();
        self.skip_until_ts = Some(seeked.required_ts);

        let t = self.time_base.calc_time(seeked.required_ts);
        Ok(t.seconds as f64 + t.frac)
    }
}

/// Average bitrate in kbps from a source's byte length and media duration.
/// Returns `None` for degenerate inputs instead of displaying garbage.
fn average_kbps(bytes: u64, duration_secs: f64) -> Option<u32> {
    if !duration_secs.is_finite() || duration_secs <= 0.0 || bytes == 0 {
        return None;
    }
    let kbps = (bytes as f64 * 8.0 / duration_secs / 1000.0).round();
    (kbps.is_finite() && (1.0..=100_000.0).contains(&kbps)).then_some(kbps as u32)
}

#[cfg(test)]
mod tests {
    use super::average_kbps;

    #[test]
    fn average_kbps_computes_classic_readout() {
        // 0.5 s of 16-bit stereo 44.1 kHz PCM ≈ 1.41 Mbps (plus a tiny header).
        let bytes = 44_100u64 * 2 * 2 / 2;
        assert_eq!(average_kbps(bytes, 0.5), Some(1411));
        // Typical 3-minute 192 kbps MP3.
        assert_eq!(average_kbps(192_000 * 180 / 8, 180.0), Some(192));
    }

    #[test]
    fn average_kbps_rejects_degenerate_inputs() {
        assert_eq!(average_kbps(0, 10.0), None);
        assert_eq!(average_kbps(1000, 0.0), None);
        assert_eq!(average_kbps(1000, -1.0), None);
        assert_eq!(average_kbps(1000, f64::NAN), None);
        assert_eq!(average_kbps(1, 1e9), None); // rounds to 0 kbps
    }
}
