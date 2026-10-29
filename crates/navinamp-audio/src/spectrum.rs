//! Spectrum analyser: an RT-side lock-free tap plus the (non-RT) FFT that
//! turns it into Winamp-style bars and a waveform.
//!
//! # Tap design
//!
//! [`SpectrumTap`] is a fixed 2048-frame mono ring written by the output
//! callback **after the DSP chain and before the volume ramp** — the tap
//! hears exactly what the listener hears, pre-volume. The callback
//! mono-mixes each interleaved frame and stores it at a single write cursor
//! (`Release`): no allocation, no locks, one atomic store per frame. The
//! reader ([`crate::EngineHandle::spectrum`], IPC thread) snapshots the
//! latest 2048 samples; because the writer may lap the reader mid-copy, a
//! snapshot can splice a few ms of older samples at the wrap point — a
//! benign, invisible-to-the-eye race for a 60 fps visualiser.
//!
//! # Scaling choices
//!
//! * **Waveform**: the last 128 mono samples, clamped to −1..=1 (EQ boost
//!   can exceed full scale; the display must not).
//! * **Spectrum**: a 2048-point FFT over a Hann window; 32 bars on a
//!   geometric (log) grid from 20 Hz to Nyquist; each bar is the peak FFT
//!   magnitude whose centre frequency falls in the bar's band. Magnitudes
//!   are normalised so a full-scale sinusoid lands at 0 dBFS (Hann coherent
//!   gain ½ ⇒ divide by N/4), then mapped linearly in dB from a −60 dB
//!   floor (`0.0`) to 0 dBFS (`1.0`), clamped. DC and sub-20 Hz bins are
//!   skipped so infrasonic rumble cannot light the lowest bar.

use std::cell::UnsafeCell;
use std::f32::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use rustfft::{Fft, FftPlanner, num_complex::Complex};
use serde::Serialize;

/// Tap/FFT length in mono samples (≈46 ms at 44.1 kHz).
pub(crate) const WINDOW: usize = 2048;
/// Waveform (oscilloscope) length in samples.
pub(crate) const WAVEFORM: usize = 128;
/// Number of spectrum bars.
pub const SPECTRUM_BINS: usize = 32;
/// Lowest analysed frequency (geometric grid start), Hz.
const F_MIN_HZ: f64 = 20.0;
/// Display floor: this many dB below full scale maps to bar value 0.
const FLOOR_DB: f32 = -60.0;

/// The analyser snapshot returned to the UI (serialized `camelCase`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spectrum {
    /// True only while the engine is `Playing`.
    pub active: bool,
    /// 32 log-spaced bins, 0..=1 (−60 dB floor → 0 dBFS).
    pub spectrum: [f32; SPECTRUM_BINS],
    /// Last 128 mono samples, −1..=1.
    pub waveform: [f32; WAVEFORM],
}

// serde only derives arrays up to 32 elements; serialize the 128-sample
// waveform as a plain JSON array (identical wire format to the contract).
impl Serialize for Spectrum {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Spectrum", 3)?;
        state.serialize_field("active", &self.active)?;
        state.serialize_field("spectrum", &self.spectrum[..])?;
        state.serialize_field("waveform", &self.waveform[..])?;
        state.end()
    }
}

impl Default for Spectrum {
    /// Inactive silence — what an idle (never-started) engine reports.
    fn default() -> Self {
        Self {
            active: false,
            spectrum: [0.0; SPECTRUM_BINS],
            waveform: [0.0; WAVEFORM],
        }
    }
}

/// Latest-2048-mono-samples tap. Single writer (the audio callback), any
/// number of readers; see the module docs for the benign-lap race.
pub(crate) struct SpectrumTap {
    buf: UnsafeCell<Box<[f32; WINDOW]>>,
    /// Total frames ever written; `buf[pos & MASK]` is the newest.
    pos: AtomicU64,
}

// SAFETY: `buf` is written only by the callback (ahead of `pos`, published
// with `Release`); readers copy behind `pos` (loaded with `Acquire`) and
// tolerate a writer lap mid-copy (documented above).
unsafe impl Sync for SpectrumTap {}

impl SpectrumTap {
    pub(crate) fn new() -> Self {
        Self {
            buf: UnsafeCell::new(
                vec![0.0; WINDOW]
                    .into_boxed_slice()
                    .try_into()
                    .ok()
                    .unwrap(),
            ),
            pos: AtomicU64::new(0),
        }
    }

    /// RT: mono-mix `buf` (interleaved, `channels` wide) into the tap.
    pub(crate) fn write(&self, buf: &[f32], channels: usize) {
        let ch = channels.max(1);
        let buf_ptr = unsafe { &mut *self.buf.get() };
        let mut pos = self.pos.load(Ordering::Relaxed);
        for frame in buf.chunks_exact(ch) {
            let mut mono = 0.0f32;
            for &s in frame {
                mono += s;
            }
            mono /= ch as f32;
            buf_ptr[(pos & (WINDOW as u64 - 1)) as usize] = mono;
            pos += 1;
        }
        self.pos.store(pos, Ordering::Release);
    }

    /// Copy out the latest [`WINDOW`] mono samples (unwritten history is
    /// silence).
    pub(crate) fn window(&self) -> [f32; WINDOW] {
        let pos = self.pos.load(Ordering::Acquire);
        let src = unsafe { &*self.buf.get() };
        let mut out = [0.0f32; WINDOW];
        let start = pos.wrapping_sub(WINDOW as u64);
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = src[(start.wrapping_add(i as u64) & (WINDOW as u64 - 1)) as usize];
        }
        out
    }
}

/// The shared forward FFT plan (planned once; `process` is then
/// allocation-free apart from the caller's buffers).
fn fft() -> &'static Arc<dyn Fft<f32>> {
    static PLAN: OnceLock<Arc<dyn Fft<f32>>> = OnceLock::new();
    PLAN.get_or_init(|| FftPlanner::<f32>::new().plan_fft_forward(WINDOW))
}

/// Analyse one tap window: 32 spectrum bars + 128 waveform samples. Pure
/// function (unit-testable); runs on the IPC thread, never in the callback.
pub(crate) fn analyse(window: &[f32; WINDOW], rate: u32) -> SpectrumValues {
    // Waveform: the newest samples, display-clamped.
    let mut waveform = [0.0f32; WAVEFORM];
    for (i, slot) in waveform.iter_mut().enumerate() {
        *slot = window[WINDOW - WAVEFORM + i].clamp(-1.0, 1.0);
    }

    // Hann-windowed FFT.
    let mut cx: Vec<Complex<f32>> = Vec::with_capacity(WINDOW);
    for (n, &s) in window.iter().enumerate() {
        let w = 0.5 - 0.5 * (2.0 * PI * n as f32 / WINDOW as f32).cos();
        cx.push(Complex::new(s * w, 0.0));
    }
    fft().process(&mut cx);

    // Geometric band grid: bar k covers [F_MIN * r^k, F_MIN * r^(k+1)].
    let rate = f64::from(rate.max(1));
    let nyquist = rate * 0.5;
    let log_step = (nyquist / F_MIN_HZ).ln() / SPECTRUM_BINS as f64;
    // Full-scale sinusoid ≈ N/4 magnitude after the Hann window.
    let norm = WINDOW as f32 / 4.0;

    let mut spectrum = [0.0f32; SPECTRUM_BINS];
    for (j, c) in cx.iter().enumerate().take(WINDOW / 2).skip(1) {
        let freq = j as f64 * rate / WINDOW as f64;
        if freq < F_MIN_HZ {
            continue;
        }
        let bar = (((freq / F_MIN_HZ).ln() / log_step) as usize).min(SPECTRUM_BINS - 1);
        let mag = c.norm();
        if !mag.is_finite() || mag <= f32::MIN_POSITIVE {
            continue; // silence (and NaNs) contribute nothing
        }
        let db = 20.0 * (mag / norm).log10();
        let value = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
        if value > spectrum[bar] {
            spectrum[bar] = value;
        }
    }
    SpectrumValues { spectrum, waveform }
}

/// Bar/waveform pair produced by [`analyse`] (the `active` flag is a
/// playback-status question, added by the caller).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SpectrumValues {
    pub spectrum: [f32; SPECTRUM_BINS],
    pub waveform: [f32; WAVEFORM],
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_window(freq: f64, amp: f64, rate: f64) -> [f32; WINDOW] {
        let mut w = [0.0f32; WINDOW];
        for (n, slot) in w.iter_mut().enumerate() {
            *slot = (amp * (2.0 * std::f64::consts::PI * freq * n as f64 / rate).sin()) as f32;
        }
        w
    }

    #[test]
    fn silence_yields_all_zero_bins_and_waveform() {
        let v = analyse(&[0.0; WINDOW], 44_100);
        assert_eq!(v.spectrum, [0.0; SPECTRUM_BINS]);
        assert_eq!(v.waveform, [0.0; WAVEFORM]);
    }

    #[test]
    fn full_scale_sine_lands_in_expected_bin() {
        let rate = 44_100.0;
        let v = analyse(&sine_window(1000.0, 1.0, rate), 44_100);
        // Expected bar: floor(ln(1000/20)/ln(22050/20)*32) = 17.
        let loudest = v
            .spectrum
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert!(
            (16..=18).contains(&loudest),
            "1 kHz peaked in bar {loudest}: {:?}",
            v.spectrum
        );
        assert!(
            v.spectrum[loudest] > 0.7,
            "full-scale sine should read near 1.0: {:?}",
            v.spectrum
        );
        // Energy stays local: distant bars are quiet.
        assert!(
            v.spectrum[5] < 0.2 && v.spectrum[28] < 0.2,
            "{:?}",
            v.spectrum
        );
    }

    #[test]
    fn waveform_tracks_latest_samples() {
        let mut w = [0.25f32; WINDOW];
        w[WINDOW - 1] = -0.75;
        let v = analyse(&w, 44_100);
        assert_eq!(v.waveform[WAVEFORM - 1], -0.75);
        assert!(v.waveform[..WAVEFORM - 1].iter().all(|&s| s == 0.25));
    }

    #[test]
    fn waveform_is_clamped_to_display_range() {
        let v = analyse(&sine_window(1000.0, 4.0, 44_100.0), 44_100);
        assert!(v.waveform.iter().all(|s| (-1.0..=1.0).contains(s)));
    }

    #[test]
    fn tap_roundtrips_and_zero_fills_history() {
        let tap = SpectrumTap::new();
        // Fresh tap: all silence.
        assert_eq!(tap.window(), [0.0; WINDOW]);
        // Write exactly one stereo block of 1024 frames = 1024 mono samples.
        let block: Vec<f32> = (0..1024).flat_map(|i| [i as f32, i as f32]).collect();
        tap.write(&block, 2);
        let w = tap.window();
        // Newest 1024 are ours, oldest 1024 are unwritten silence.
        assert!(w[..WINDOW - 1024].iter().all(|&s| s == 0.0));
        for i in 0..1024 {
            assert_eq!(w[WINDOW - 1024 + i], i as f32);
        }
        // A second block wraps the ring.
        tap.write(&block, 2);
        let w = tap.window();
        assert!(w[..1024].iter().enumerate().all(|(i, &s)| s == i as f32));
    }
}
