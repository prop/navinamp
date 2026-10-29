//! M6a integration tests: the EQ stage, ReplayGain application and the
//! spectrum analyser, all through the real engine on a `TestSink`.
//!
//! Fixtures are the shared PCM16 sine WAVs; expected engine output is the
//! quantized sine (i16/32768) — the exactness assertions hold because the
//! DSP chain is bit-transparent when flat, and applies ReplayGain as a
//! single f32 multiply when tagged.

mod common;

use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use navamp_audio::{Command, EngineHandle, Event, PlaybackStatus, TestSink, TrackSource};

use common::{
    RgMeta, as_expected_f32, replaygain_fixture, sine_i16, stereo_sine_fixture,
    stereo_sine_fixture_amp, temp_path, write_wav,
};

const RATE: u32 = 44_100;
const TIMEOUT: Duration = Duration::from_secs(10);

fn engine_on(sink: &TestSink) -> EngineHandle {
    EngineHandle::start_with_sink(sink.clone()).expect("engine start")
}

/// Pull the sink in small periods until `cond` holds.
fn drive(sink: &TestSink, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "drive: condition not met in time"
        );
        sink.pull(128);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Wait for an event matching `pred` while keeping the sink pulling.
fn wait_event(sink: &TestSink, rx: &Receiver<Event>, mut pred: impl FnMut(&Event) -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(e) = rx.try_recv() {
            if pred(&e) {
                return;
            }
            continue;
        }
        assert!(Instant::now() < deadline, "timed out waiting for event");
        sink.pull(128);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Wait for `TrackStarted` + `Playing` without consuming audio.
fn start_and_get_playing(engine: &EngineHandle, sink: &TestSink, rx: &Receiver<Event>) {
    wait_event(sink, rx, |e| matches!(e, Event::TrackStarted { .. }));
    drive(sink, || engine.status() == PlaybackStatus::Playing);
}

/// Pull exactly `frames` frames, appending to `out`.
fn pull_frames(sink: &TestSink, out: &mut Vec<f32>, frames: usize) {
    let mut remaining = frames;
    while remaining > 0 {
        let n = remaining.min(512);
        out.extend(sink.pull(n));
        remaining -= n;
    }
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / x.len() as f64).sqrt()
}

// ---------------------------------------------------------------------------
// EQ
// ---------------------------------------------------------------------------

#[test]
fn default_eq_is_bit_exact_passthrough() {
    // Default state: disabled + flat — the engine must reproduce the file
    // bit-for-bit (identity biquads would round).
    let (wav, expected) = stereo_sine_fixture("eq-default", RATE, 0.5, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    let state = engine.eq_state();
    assert!(!state.enabled);
    assert_eq!(state.preamp_db, 0.0);
    assert_eq!(state.bands_db, [0.0; 10]);

    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    let half = 11_025; // the 0.5 s fixture is 22_050 frames
    let mut out = Vec::new();
    pull_frames(&sink, &mut out, half);
    assert_eq!(out.len(), half * 2);
    assert_eq!(
        out,
        expected[..half * 2],
        "flat+disabled EQ must be bit-exact"
    );

    // Enabling a *flat* curve changes nothing either.
    engine.eq_set_enabled(true);
    let mut out2 = Vec::new();
    pull_frames(&sink, &mut out2, half);
    assert_eq!(out2.len(), half * 2);
    assert_eq!(out2, expected[half * 2..], "flat+enabled stays bit-exact");

    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

#[test]
fn eq_band_boosts_its_center_frequency() {
    // +12 dB on the 1 kHz band: a 1 kHz sine must come out ≈ 4× (+12 dB,
    // ±1 dB tolerance) after the filter settles.
    let (wav, expected) = stereo_sine_fixture_amp("eq-boost", RATE, 1.0, 1000.0, 0.25);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine.eq_set_band(4, 12.0).expect("band");
    engine.eq_set_preamp(0.0);
    engine.eq_set_enabled(true);
    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    let mut out = Vec::new();
    pull_frames(&sink, &mut out, RATE as usize);

    // Skip the first 0.25 s (biquad transient), measure steady state.
    let settle = (RATE as usize / 4) * 2;
    let out_l: Vec<f32> = out[settle..].iter().step_by(2).copied().collect();
    let exp_l: Vec<f32> = expected[settle..].iter().step_by(2).copied().collect();
    let ratio_db = 20.0 * (rms(&out_l) / rms(&exp_l)).log10();
    assert!(
        (ratio_db - 12.0).abs() <= 1.0,
        "1 kHz boost measured {ratio_db:.2} dB (expected ≈ +12)"
    );
    assert!(out.iter().all(|s| s.is_finite()));

    // The state we set is the state we read back.
    let state = engine.eq_state();
    assert!(state.enabled);
    assert_eq!(state.bands_db[4], 12.0);

    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

#[test]
fn eq_set_band_rejects_out_of_range_index() {
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    assert!(engine.eq_set_band(10, 3.0).is_err());
    assert!(engine.eq_set_band(u32::MAX, 3.0).is_err());
    assert!(engine.eq_set_band(9, 12.0).is_ok());
    assert_eq!(engine.eq_state().bands_db[9], 12.0);
}

#[test]
fn eq_param_changes_midstream_stay_finite() {
    let (wav, _exp) = stereo_sine_fixture("eq-hammer", RATE, 0.75, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();
    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    // Flip bands/enable/preamp between pulls; output must stay finite and
    // bounded (no NaN/inf, no runaway).
    let mut worst = 0.0f32;
    for step in 0..24 {
        engine.eq_set_enabled(step % 2 == 0);
        let _ = engine.eq_set_band(step % 10, (step % 25) as f32 - 12.0);
        engine.eq_set_preamp(((step * 7) % 25) as f32 - 12.0);
        let out = sink.pull(512);
        for &s in &out {
            assert!(s.is_finite(), "non-finite sample at step {step}");
            worst = worst.max(s.abs());
        }
    }
    assert!(worst < 12.0, "runaway amplitude: {worst}");

    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

#[test]
fn eq_disable_returns_to_exact_passthrough() {
    let (wav, expected) = stereo_sine_fixture("eq-toggle", RATE, 0.6, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    let _ = engine.eq_set_band(0, 9.0); // thump the 60 Hz band
    engine.eq_set_enabled(true);
    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    let track_frames = RATE as usize * 3 / 5; // the whole 0.6 s fixture
    let head = 10_000; // filtered head
    let mut out = Vec::new();
    pull_frames(&sink, &mut out, head);

    // While enabled, output is filtered: it must differ from the file...
    let filtered_prefix = out
        .as_chunks::<2>()
        .0
        .iter()
        .zip(expected.as_chunks::<2>().0)
        .take(2000)
        .any(|(a, e)| a != e);
    assert!(filtered_prefix, "enabled EQ must alter the signal");

    // Disable mid-track; the rest of the track comes through untouched.
    engine.eq_set_enabled(false);
    let mut tail = Vec::new();
    pull_frames(&sink, &mut tail, track_frames - head);
    out.extend_from_slice(&tail);
    assert_eq!(out.len(), track_frames * 2);

    // ...and the disable lands at a block boundary: from some frame on, the
    // remaining output is bit-exact passthrough again.
    let total_frames = out.len() / 2;
    let mut match_from = total_frames;
    while match_from > 0 && out[(match_from - 1) * 2..] == expected[(match_from - 1) * 2..] {
        match_from -= 1;
    }
    assert!(
        match_from <= total_frames - 2048,
        "output never returned to exact passthrough (match_from {match_from})"
    );

    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

// ---------------------------------------------------------------------------
// ReplayGain
// ---------------------------------------------------------------------------

#[test]
fn replaygain_album_gain_is_preferred_and_applied() {
    // Album -6.02 dB (≈ ×0.5) must win over the track tag (−12.04 dB).
    let gain = 10f32.powf(-6.02 / 20.0);
    let (wav, expected, applied) = replaygain_fixture(
        "rg-album",
        440.0,
        &RgMeta {
            album_gain_db: Some("-6.02 dB"),
            album_peak: Some("0.500"),
            track_gain_db: Some("-12.04 dB"),
            track_peak: Some("0.500"),
        },
        gain,
    );
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    assert!((engine.replaygain_gain() - applied).abs() < 1e-7);

    let mut out = Vec::new();
    pull_frames(&sink, &mut out, 22_050);
    // Peak 0.5 does not clip a ×0.5 gain; every sample is expected × gain,
    // bit-exactly (one f32 multiply on both sides).
    for (i, (a, e)) in out.iter().zip(&expected).enumerate() {
        assert_eq!(*a, e * applied, "sample {i}");
    }

    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

#[test]
fn replaygain_falls_back_to_track_gain() {
    // No album gain: track -6.02 dB applies (album peak alone wins nothing).
    let gain = 10f32.powf(-6.02 / 20.0);
    let (wav, expected, applied) = replaygain_fixture(
        "rg-track",
        440.0,
        &RgMeta {
            album_peak: Some("0.9"),
            track_gain_db: Some("-6.02 dB"),
            track_peak: Some("1.0"),
            ..RgMeta::default()
        },
        gain,
    );
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();
    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    assert!((engine.replaygain_gain() - applied).abs() < 1e-7);
    let mut out = Vec::new();
    pull_frames(&sink, &mut out, 4096);
    for (i, (a, e)) in out.iter().zip(&expected).enumerate() {
        assert_eq!(*a, e * applied, "sample {i}");
    }
    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
}

#[test]
fn replaygain_peak_protection_limits_the_boost() {
    // Track +6 dB (≈ ×2.0) with track peak 0.75 clamps to 1/0.75 ≈ 1.333.
    let gain = (1.0f32 / 0.75).min(10f32.powf(6.0 / 20.0));
    let (wav, expected, applied) = replaygain_fixture(
        "rg-clamp",
        440.0,
        &RgMeta {
            track_gain_db: Some("+6.00 dB"),
            track_peak: Some("0.75"),
            ..RgMeta::default()
        },
        gain,
    );
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();
    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    assert!((engine.replaygain_gain() - applied).abs() < 1e-7);
    assert!((applied - 1.0 / 0.75).abs() < 1e-6, "gain {applied}");
    let mut out = Vec::new();
    pull_frames(&sink, &mut out, 4096);
    for (i, (a, e)) in out.iter().zip(&expected).enumerate() {
        assert_eq!(*a, e * applied, "sample {i}");
    }
    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
}

#[test]
fn replaygain_gapless_boundary_is_sample_exact() {
    // A untagged track flows into a tagged one: the gain must switch at
    // exactly the first frame of track B — not early (while A's tail is
    // still buffered) and not late.
    let gain = 10f32.powf(-6.02 / 20.0);
    let (wav_a, exp_a) = stereo_sine_fixture("rg-gap-a", RATE, 0.5, 440.0);
    let (wav_b, exp_b, applied) = replaygain_fixture(
        "rg-gap-b",
        660.0,
        &RgMeta {
            album_gain_db: Some("-6.02 dB"),
            album_peak: Some("0.9"),
            ..RgMeta::default()
        },
        gain,
    );
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(wav_a)))
        .expect("play a");
    engine
        .send(Command::EnqueueNext(TrackSource::File(wav_b)))
        .expect("enqueue b");
    start_and_get_playing(&engine, &sink, &rx);

    let mut out = Vec::new();
    pull_frames(&sink, &mut out, 44_100); // A + B

    // A: untouched; B: scaled — sample-exact at the boundary frame.
    let mut expected = exp_a;
    expected.extend(exp_b.iter().map(|&s| s * applied));
    assert_eq!(out.len(), expected.len());
    assert_eq!(out, expected, "gapless gain switch must be sample-exact");

    assert_eq!(engine.underrun_frames(), 0);
    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

#[test]
fn untagged_fixture_reports_unity_gain() {
    let (wav, _exp) = stereo_sine_fixture("rg-none", RATE, 0.25, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();
    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);
    assert_eq!(engine.replaygain_gain(), 1.0);
    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
}

// ---------------------------------------------------------------------------
// Spectrum analyser
// ---------------------------------------------------------------------------

#[test]
fn spectrum_is_inactive_before_playback() {
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let s = engine.spectrum();
    assert!(!s.active);
    assert_eq!(s.spectrum, [0.0; 32]);
    assert_eq!(s.waveform, [0.0; 128]);
}

#[test]
fn spectrum_tracks_playback_status() {
    let pcm = sine_i16(RATE, RATE as usize / 4, 0.0, 0.0, 2); // digital silence
    let path = temp_path("vis-silence");
    write_wav(&path, RATE, 2, &pcm).unwrap();
    let _expected = as_expected_f32(&pcm);

    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();
    engine
        .send(Command::Play(TrackSource::File(path)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    // Active while playing: silence in → zero bars/waveform.
    pull_frames(&sink, &mut Vec::new(), 4096);
    let s = engine.spectrum();
    assert!(s.active);
    assert_eq!(s.spectrum, [0.0; 32], "silence must read as zero bars");
    assert!(s.waveform.iter().all(|&v| v == 0.0));

    // ...and inactive once the queue drains.
    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
    let s = engine.spectrum();
    assert!(!s.active);
}

#[test]
fn spectrum_full_scale_sine_lands_in_expected_bin() {
    let (wav, _exp) = stereo_sine_fixture_amp("vis-1k", RATE, 0.6, 1000.0, 1.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();
    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    pull_frames(&sink, &mut Vec::new(), 8192); // fill the 2048 tap well past startup
    let s = engine.spectrum();
    assert!(s.active);
    let loudest = s
        .spectrum
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    // Geometric 20 Hz..22.05 kHz grid: 1 kHz falls in bar 17 (±1 tolerance).
    assert!(
        (16..=18).contains(&loudest),
        "1 kHz peaked in bar {loudest}: {:?}",
        s.spectrum
    );
    assert!(
        s.spectrum[loudest] > 0.6,
        "full-scale sine should read high: {:?}",
        s.spectrum
    );
    assert!(
        s.waveform.iter().any(|&v| v.abs() > 0.5),
        "waveform should show the sine: {:?}",
        &s.waveform[..16]
    );

    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}
