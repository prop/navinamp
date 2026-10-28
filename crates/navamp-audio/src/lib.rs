//! NavAmp audio engine (M1 implementation pending).
//!
//! Target design: decode thread (symphonia) -> DSP chain -> lock-free ring
//! buffer -> cpal audio callback, with a gapless play queue.
