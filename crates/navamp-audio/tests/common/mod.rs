//! Test fixtures: write PCM16 WAV files in-test (std only) and derive the exact
//! f32 samples the engine should reproduce (i16/32768 — symphonia's conversion).

use std::f64::consts::PI;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

/// A unique-enough temp file path for a test fixture.
pub fn temp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "navamp-audio-test-{}-{name}.wav",
        std::process::id()
    ));
    p
}

/// Minimal PCM16 WAV writer (RIFF, no extras).
pub fn write_wav(
    path: &std::path::Path,
    sample_rate: u32,
    channels: u16,
    interleaved: &[i16],
) -> std::io::Result<()> {
    assert!(
        interleaved.len().is_multiple_of(channels as usize),
        "samples must be frame-aligned"
    );
    let block_align = channels * 2;
    let data_len = interleaved.len() as u32 * 2;
    let byte_rate = sample_rate * u32::from(block_align);

    let mut f = BufWriter::new(File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in interleaved {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}

/// Interleaved PCM16 sine (same signal on every channel).
pub fn sine_i16(sample_rate: u32, frames: usize, freq: f64, amp: f64, channels: u16) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * channels as usize);
    for n in 0..frames {
        let t = n as f64 / f64::from(sample_rate);
        let v = (2.0 * PI * freq * t).sin() * amp;
        let s = (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        for _ in 0..channels {
            out.push(s);
        }
    }
    out
}

/// The exact f32 samples decoding the WAV should yield (i16 → f32 is a division
/// by 32768).
pub fn as_expected_f32(interleaved: &[i16]) -> Vec<f32> {
    interleaved
        .iter()
        .map(|&s| f32::from(s) / 32768.0)
        .collect()
}

/// Write a stereo sine fixture and return the expected decoded samples.
/// Returns (path, expected interleaved f32).
pub fn stereo_sine_fixture(
    name: &str,
    sample_rate: u32,
    secs: f64,
    freq: f64,
) -> (PathBuf, Vec<f32>) {
    let frames = (f64::from(sample_rate) * secs).round() as usize;
    let pcm = sine_i16(sample_rate, frames, freq, 0.5, 2);
    let path = temp_path(name);
    write_wav(&path, sample_rate, 2, &pcm).expect("write fixture wav");
    (path, as_expected_f32(&pcm))
}
