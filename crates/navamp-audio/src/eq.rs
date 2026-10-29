//! Winamp-style 10-band equaliser: the lock-free parameter store, the public
//! state/preset types, and the preset table.
//!
//! # Lock-free parameter path (RT contract)
//!
//! [`EqParams`] is the single source of truth for every EQ setting. Writers
//! (the Tauri IPC thread, via [`crate::EngineHandle`]) store each value as
//! raw `f32` bits in its own atomic (`Release`) and then bump a generation
//! counter (`fetch_add`, `Release`). The real-time side — [`crate::dsp`]'s
//! `DspChain` — loads only the generation counter each block (`Acquire`) and
//! re-reads the values, recomputing all biquad coefficients, **only when it
//! changed**. Seeing generation *N* via `Acquire` guarantees every store
//! sequenced before increment *N* is visible, so a snapshot is never
//! internally stale. The only anomaly possible is a one-block mix of old and
//! new values while two independent setters race (e.g. one band updated, the
//! next not yet); values are always finite and in range, and the next block
//! re-synchronises — no locks, no allocation, no blocking anywhere.
//!
//! # Layout
//!
//! 10 peaking biquads (RBJ cookbook, Q ≈ 1.0) at 60, 170, 310, 600, 1k, 3k,
//! 6k, 12k, 14k, 16k — the authentic classic-Winamp band centres — plus a
//! preamp applied *before* the bands. Flat (all bands 0 dB, preamp 0 dB,
//! enabled or not) bypasses the entire EQ stage bit-exactly; see
//! [`crate::dsp`].

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use serde::Serialize;

/// Number of EQ bands.
pub const EQ_BAND_COUNT: usize = 10;

/// Band centre frequencies in Hz — the authentic classic-Winamp EQ layout,
/// matching the labels on the skin's sliders (60, 170, 310, 600, 1K, 3K,
/// 6K, 12K, 14K, 16K).
/// Authentic classic-Winamp EQ band centres (as labelled on the skin's
/// sliders): 60, 170, 310, 600, 1k, 3k, 6k, 12k, 14k, 16k.
pub const EQ_BAND_FREQUENCIES_HZ: [f32; EQ_BAND_COUNT] = [
    60.0, 170.0, 310.0, 600.0, 1000.0, 3000.0, 6000.0, 12000.0, 14000.0, 16000.0,
];

/// Slider range in dB (`±12`, like Winamp).
pub const EQ_RANGE_DB: f32 = 12.0;

/// Q of every peaking band (~1.0 — a middle-of-the-road curve width; a
/// peaking filter's response at its centre frequency is exactly the band
/// gain regardless of Q, so this only shapes the shoulders).
pub(crate) const EQ_Q: f32 = 1.0;

/// Sanitize + clamp a dB value coming from the IPC surface. Non-finite
/// values (NaN, ±inf — legal f32, absurd EQ) collapse to flat 0 dB so the
/// RT path can never see a coefficient it did not expect.
pub(crate) fn sanitize_db(db: f32) -> f32 {
    if db.is_finite() {
        db.clamp(-EQ_RANGE_DB, EQ_RANGE_DB)
    } else {
        0.0
    }
}

/// The equaliser's full parameter set, as atomics. Shared between the IPC
/// side (setters) and the audio callback (generation-counted snapshots); see
/// the module docs for the ordering protocol.
pub(crate) struct EqParams {
    enabled: AtomicBool,
    preamp_db: AtomicU32,
    bands_db: [AtomicU32; EQ_BAND_COUNT],
    generation: AtomicU64,
}

impl EqParams {
    pub(crate) fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            preamp_db: AtomicU32::new(0.0f32.to_bits()),
            bands_db: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
            generation: AtomicU64::new(0),
        }
    }

    /// Bump the generation after a value store (writer side, in order).
    fn publish(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        self.publish();
    }

    /// Preamp in dB, clamped to ±[`EQ_RANGE_DB`].
    pub(crate) fn set_preamp_db(&self, db: f32) {
        self.preamp_db
            .store(sanitize_db(db).to_bits(), Ordering::Release);
        self.publish();
    }

    /// One band's gain in dB (index < [`EQ_BAND_COUNT`]), clamped to
    /// ±[`EQ_RANGE_DB`].
    pub(crate) fn set_band_db(&self, index: usize, db: f32) {
        if index >= EQ_BAND_COUNT {
            return;
        }
        self.bands_db[index].store(sanitize_db(db).to_bits(), Ordering::Release);
        self.publish();
    }

    /// Current generation (reader side: load `Acquire` *before* the values).
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Read every parameter (reader side; call after [`Self::generation`]).
    pub(crate) fn snapshot(&self) -> EqSnapshot {
        EqSnapshot {
            enabled: self.enabled.load(Ordering::Relaxed),
            preamp_db: f32::from_bits(self.preamp_db.load(Ordering::Relaxed)),
            bands_db: std::array::from_fn(|i| {
                f32::from_bits(self.bands_db[i].load(Ordering::Relaxed))
            }),
        }
    }
}

/// A consistent-ish read of every EQ parameter (see module docs).
pub(crate) struct EqSnapshot {
    pub enabled: bool,
    pub preamp_db: f32,
    pub bands_db: [f32; EQ_BAND_COUNT],
}

impl EqSnapshot {
    /// True when the EQ stage must actually run: enabled and not flat. Flat
    /// means preamp 0 dB and every band 0 dB — running samples through
    /// unity biquads would only accumulate float error, so the chain skips
    /// the whole stage instead.
    pub(crate) fn active(&self) -> bool {
        self.enabled && (self.preamp_db != 0.0 || self.bands_db.iter().any(|&db| db != 0.0))
    }
}

/// The EQ state reported to the UI (serialized `camelCase`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqState {
    /// Whether the EQ stage is active.
    pub enabled: bool,
    /// Preamp in dB (−12..=12), applied before the bands.
    pub preamp_db: f32,
    /// Band gains in dB (−12..=12), index = [`EQ_BAND_FREQUENCIES_HZ`] order.
    pub bands_db: [f32; EQ_BAND_COUNT],
}

/// One named preset curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqPreset {
    /// Display name.
    pub name: &'static str,
    /// Band gains in dB (clamped to ±12 — a few classic curves reach ±16 in
    /// Winamp's own files; our engine range is ±12 like the sliders).
    pub bands_db: [f32; EQ_BAND_COUNT],
}

/// The classic Winamp preset curves (values from the well-known preset
/// tables), clamped to ±12 dB. Order is alphabetical, like the Winamp menu.
static EQ_PRESETS: [EqPreset; 17] = [
    EqPreset {
        name: "Classical",
        bands_db: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -7.2, -7.2, -7.2, -9.6],
    },
    EqPreset {
        name: "Club",
        bands_db: [0.0, 0.0, 4.8, 3.6, 3.6, 3.6, 2.4, 0.0, 0.0, 0.0],
    },
    EqPreset {
        name: "Dance",
        bands_db: [9.6, 7.2, 2.4, 0.0, 0.0, -5.4, -7.2, -7.2, 0.0, 0.0],
    },
    EqPreset {
        name: "Full Bass",
        bands_db: [-8.0, 9.6, 9.6, 5.6, 1.6, -4.0, -8.0, -10.4, -11.2, -11.2],
    },
    EqPreset {
        name: "Full Bass & Treble",
        bands_db: [4.8, 3.6, 0.0, -7.2, -4.8, 1.2, 8.0, 11.2, 12.0, 12.0],
    },
    EqPreset {
        name: "Full Treble",
        bands_db: [-9.6, -9.6, -9.6, -4.2, 2.4, 11.2, 12.0, 12.0, 12.0, 12.0],
    },
    EqPreset {
        name: "Jazz",
        bands_db: [4.8, -3.6, -2.4, 1.6, 4.8, 4.8, 0.0, 1.6, 3.6, 4.8],
    },
    EqPreset {
        name: "Large Hall",
        bands_db: [10.4, 10.4, 5.6, 5.6, 0.0, -4.8, -4.8, -4.8, 0.0, 0.0],
    },
    EqPreset {
        name: "Live",
        bands_db: [-4.8, 0.0, 4.0, 5.6, 5.6, 5.6, 4.0, 2.4, 2.4, 2.4],
    },
    EqPreset {
        name: "Party",
        bands_db: [7.2, 7.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 7.2, 7.2],
    },
    EqPreset {
        name: "Pop",
        bands_db: [-1.2, 4.8, 7.2, 8.0, 5.6, 0.0, -2.4, -4.8, -4.8, -1.2],
    },
    EqPreset {
        name: "Reggae",
        bands_db: [0.0, 0.0, 0.0, -5.4, 0.0, 6.6, 6.6, 0.0, 0.0, 0.0],
    },
    EqPreset {
        name: "Rock",
        bands_db: [8.0, 4.8, -5.6, -8.0, -3.6, 4.2, 8.8, 11.2, 11.2, 11.2],
    },
    EqPreset {
        name: "Ska",
        bands_db: [-2.4, -4.8, -4.0, 0.0, 4.0, 5.6, 8.4, 9.6, 11.2, 9.6],
    },
    EqPreset {
        name: "Soft",
        bands_db: [4.8, 1.2, 0.0, -2.4, 0.0, 4.0, 8.0, 9.6, 11.2, 12.0],
    },
    EqPreset {
        name: "Soft Rock",
        bands_db: [4.0, 4.0, 2.4, 0.0, -4.0, -5.6, -3.6, 0.0, 2.4, 8.8],
    },
    EqPreset {
        name: "Techno",
        bands_db: [8.0, 5.6, 0.0, -5.6, -4.8, 0.0, 8.0, 9.6, 9.6, 8.8],
    },
];

/// The built-in preset curves (classic Winamp set).
pub fn eq_presets() -> &'static [EqPreset] {
    &EQ_PRESETS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_params_are_flat_and_disabled() {
        let params = EqParams::new();
        let snap = params.snapshot();
        assert!(!snap.enabled);
        assert_eq!(snap.preamp_db, 0.0);
        assert_eq!(snap.bands_db, [0.0; EQ_BAND_COUNT]);
        assert!(!snap.active(), "flat+disabled must be inactive");
    }

    #[test]
    fn generation_counts_every_set() {
        let params = EqParams::new();
        let g0 = params.generation();
        params.set_band_db(0, 6.0);
        params.set_band_db(9, -6.0);
        params.set_preamp_db(3.0);
        params.set_enabled(true);
        assert_eq!(params.generation(), g0 + 4);
        let snap = params.snapshot();
        assert!(snap.active());
        assert_eq!(snap.bands_db[0], 6.0);
        assert_eq!(snap.bands_db[9], -6.0);
        assert_eq!(snap.preamp_db, 3.0);
    }

    #[test]
    fn setters_clamp_and_sanitize() {
        let params = EqParams::new();
        params.set_preamp_db(99.0);
        assert_eq!(params.snapshot().preamp_db, EQ_RANGE_DB);
        params.set_preamp_db(-99.0);
        assert_eq!(params.snapshot().preamp_db, -EQ_RANGE_DB);
        params.set_band_db(3, f32::NAN);
        assert_eq!(params.snapshot().bands_db[3], 0.0);
        params.set_band_db(3, f32::INFINITY);
        assert_eq!(params.snapshot().bands_db[3], 0.0);
        params.set_band_db(3, f32::NEG_INFINITY);
        assert_eq!(params.snapshot().bands_db[3], 0.0);
        // Out-of-range index is a silent no-op (the IPC layer rejects it).
        params.set_band_db(EQ_BAND_COUNT, 5.0);
        assert_eq!(params.snapshot().bands_db, [0.0; EQ_BAND_COUNT]);
    }

    #[test]
    fn flat_but_enabled_is_still_inactive() {
        let params = EqParams::new();
        params.set_enabled(true);
        params.set_band_db(2, 4.0);
        params.set_band_db(2, 0.0); // back to flat
        assert!(!params.snapshot().active());
        params.set_preamp_db(0.5); // preamp alone counts
        assert!(params.snapshot().active());
    }

    #[test]
    fn presets_are_named_clamped_and_nonempty() {
        let presets = eq_presets();
        assert!(presets.len() >= 8, "contract asks for ~8+ presets");
        for p in presets {
            assert!(!p.name.is_empty());
            assert_eq!(p.bands_db.len(), EQ_BAND_COUNT);
            for &db in &p.bands_db {
                assert!((-EQ_RANGE_DB..=EQ_RANGE_DB).contains(&db), "{p:?}");
            }
        }
        let names: Vec<_> = presets.iter().map(|p| p.name).collect();
        for wanted in ["Rock", "Pop", "Classical", "Jazz"] {
            assert!(names.contains(&wanted), "missing preset {wanted}");
        }
    }
}
