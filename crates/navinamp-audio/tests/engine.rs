//! Engine integration tests, all hardware-free via `TestSink`.
//!
//! Fixtures are PCM16 sine WAVs generated in-test; the expected engine output is
//! the quantized sine itself (i16/32768), so gapless/pause/seek paths are checked
//! *sample-exactly*.

mod common;

use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};
use navinamp_audio::{Command, EngineHandle, Event, PlaybackStatus, TestSink, TrackSource};

use common::{as_expected_f32, sine_i16, stereo_sine_fixture, temp_path, write_wav};

const RATE: u32 = 44100;
const TIMEOUT: Duration = Duration::from_secs(10);

fn engine_on(sink: &TestSink) -> EngineHandle {
    EngineHandle::start_with_sink(sink.clone()).expect("engine start")
}

/// Pull the sink in small periods until `cond` holds. Returns everything pulled.
fn drive(sink: &TestSink, mut cond: impl FnMut() -> bool) -> Vec<f32> {
    let mut out = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "drive: condition not met in time"
        );
        out.extend(sink.pull(128));
        std::thread::sleep(Duration::from_millis(1));
    }
    out
}

/// Wait for the next event while keeping the sink pulling.
fn next_event(sink: &TestSink, rx: &Receiver<Event>) -> Event {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match rx.try_recv() {
            Ok(e) => return e,
            Err(TryRecvError::Disconnected) => panic!("event channel closed"),
            Err(TryRecvError::Empty) => {}
        }
        assert!(Instant::now() < deadline, "timed out waiting for event");
        sink.pull(128);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Wait for an event matching `pred`, skipping (and recording) others.
fn wait_event(
    sink: &TestSink,
    rx: &Receiver<Event>,
    mut pred: impl FnMut(&Event) -> bool,
) -> Event {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(e) = rx.try_recv() {
            if pred(&e) {
                return e;
            }
            continue;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for matching event"
        );
        sink.pull(128);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Wait for `TrackStarted` and then for status `Playing`, **without consuming any
/// audio**: pulls happen only while the engine is not Playing, so the first
/// recorded pull afterwards is exactly frame 0 of the track.
fn start_and_get_playing(engine: &EngineHandle, sink: &TestSink, rx: &Receiver<Event>) -> Event {
    let started = wait_event(sink, rx, |e| matches!(e, Event::TrackStarted { .. }));
    while engine.status() != PlaybackStatus::Playing {
        assert!(
            Instant::now() < Instant::now() + TIMEOUT,
            "engine never started playing"
        );
        sink.pull(64);
        std::thread::sleep(Duration::from_millis(1));
    }
    started
}

/// Pull exactly `frames` frames, appending to `out` (interleaved stereo).
fn pull_frames(sink: &TestSink, out: &mut Vec<f32>, frames: usize) {
    let mut remaining = frames;
    while remaining > 0 {
        let n = remaining.min(512);
        out.extend(sink.pull(n));
        remaining -= n;
    }
}

fn assert_close(actual: &[f32], expected: &[f32], tol: f64, ctx: &str) {
    assert_eq!(actual.len(), expected.len(), "{ctx}: length mismatch");
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (f64::from(*a) - f64::from(*e)).abs() <= tol,
            "{ctx}: sample {i}: {a} vs {e}"
        );
    }
}

// ---------------------------------------------------------------------------
// (a) play: TrackStarted, duration, position advance
// ---------------------------------------------------------------------------

#[test]
fn play_reports_start_duration_and_advancing_position() {
    let (wav, _expected) = stereo_sine_fixture("play-basic", RATE, 0.5, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(wav.clone())))
        .expect("send play");

    let started = start_and_get_playing(&engine, &sink, &rx);
    match started {
        Event::TrackStarted { id, duration_secs } => {
            assert_eq!(id, 1);
            let d = duration_secs.expect("wav duration should be known");
            assert!((d - 0.5).abs() < 0.01, "duration {d}");
        }
        other => panic!("expected TrackStarted, got {other:?}"),
    }
    let d = engine.duration_secs().expect("duration known");
    assert!((d - 0.5).abs() < 0.01);

    let p0 = engine.position_secs();
    assert!(p0 < 0.02, "position should start near 0, was {p0}");

    drive(&sink, || engine.position_secs() > 0.1);
    let p1 = engine.position_secs();
    assert!(
        p1 > 0.09 && p1 < 0.3,
        "position should have advanced, was {p1}"
    );

    // Drain to the end of the queue.
    let e = wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    let _ = e;
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
    let p_end = engine.position_secs();
    assert!((p_end - 0.5).abs() < 0.02, "final position {p_end} ≈ 0.5");
    assert_eq!(engine.status(), PlaybackStatus::Stopped);
}

// ---------------------------------------------------------------------------
// (b) gapless: output == A ++ B exactly
// ---------------------------------------------------------------------------

#[test]
fn gapless_concatenation_is_sample_exact() {
    let (wav_a, exp_a) = stereo_sine_fixture("gapless-a", RATE, 0.5, 440.0);
    let (wav_b, exp_b) = stereo_sine_fixture("gapless-b", RATE, 0.5, 660.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    // Enqueue B immediately so the producer certainly has it before A's decode
    // runs dry (this is also how the UI will drive the queue in practice).
    engine
        .send(Command::Play(TrackSource::File(wav_a)))
        .expect("play a");
    engine
        .send(Command::EnqueueNext(TrackSource::File(wav_b)))
        .expect("enqueue b");

    let started = start_and_get_playing(&engine, &sink, &rx);
    assert!(matches!(started, Event::TrackStarted { id: 1, .. }));

    // Pull exactly A + B worth of frames: this window must be pure audio.
    let mut out = Vec::new();
    pull_frames(&sink, &mut out, 22_050 + 22_050);

    let mut expected = exp_a;
    expected.extend_from_slice(&exp_b);
    assert_close(&out, &expected, 1e-9, "gapless concatenation");

    // Event order at the transition: Finished(A) then Started(B), then EndOfQueue
    // once the consumer has drained everything.
    let finished = next_event(&sink, &rx);
    assert_eq!(finished, Event::TrackFinished { id: 1 });
    let started_b = next_event(&sink, &rx);
    assert!(
        matches!(started_b, Event::TrackStarted { id: 2, .. }),
        "{started_b:?}"
    );
    let end = wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    let _ = end;
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);

    assert_eq!(
        engine.underrun_frames(),
        0,
        "gapless playback must not underrun"
    );

    // After the queue ends the sink must be silent.
    let tail = sink.pull(256);
    assert!(tail.iter().all(|&s| s == 0.0), "silence after EndOfQueue");
}

// ---------------------------------------------------------------------------
// (c) pause / resume: frozen position, silence, exact continuation
// ---------------------------------------------------------------------------

#[test]
fn pause_freezes_position_and_resumes_exactly() {
    let (wav, expected) = stereo_sine_fixture("pause-resume", RATE, 0.5, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    // Consume a recorded 0.1 s of audio.
    let mut head = Vec::new();
    pull_frames(&sink, &mut head, 4410);
    assert_close(&head, &expected[..4410 * 2], 1e-9, "pre-pause audio");

    engine.send(Command::Pause).expect("pause");
    // PULL until the status flips; those pulls may consume a little more audio
    // (they are Playing at the time), which is fine — the splice point comes
    // from the engine position, not from a frame count.
    drive(&sink, || engine.status() == PlaybackStatus::Paused);
    let p_paused = engine.position_secs();
    let n = (p_paused * f64::from(RATE)).round() as usize;
    assert!((4410..4410 + 2000).contains(&n), "splice frame {n}");

    let silence_note: Vec<f32> = Vec::new();
    let _ = silence_note;
    let mut mid = Vec::new();
    for _ in 0..16 {
        mid.extend(sink.pull(256));
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        mid.iter().all(|s| *s == 0.0),
        "paused output must be silence"
    );
    let p_still = engine.position_secs();
    assert!(
        (p_still - p_paused).abs() < 0.002,
        "position frozen while paused: {p_paused} -> {p_still}"
    );

    engine.send(Command::Resume).expect("resume");
    // Pull only silence until the status flips (silence pulls consume nothing).
    while engine.status() != PlaybackStatus::Playing {
        sink.pull(64);
        std::thread::sleep(Duration::from_millis(1));
    }

    // The rest of the track continues exactly where it was suspended.
    let mut tail = Vec::new();
    pull_frames(&sink, &mut tail, 22_050 - n);
    assert_close(&tail, &expected[n * 2..], 1e-9, "post-resume audio");

    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
    assert!((engine.position_secs() - 0.5).abs() < 0.02);
}

// ---------------------------------------------------------------------------
// (d) seek: position jumps, playback continues from the target
// ---------------------------------------------------------------------------

#[test]
fn seek_while_paused_jumps_and_continues_exactly() {
    let (wav, expected) = stereo_sine_fixture("seek", RATE, 0.5, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    let mut head = Vec::new();
    pull_frames(&sink, &mut head, 4410); // ~0.1 s
    engine.send(Command::Pause).expect("pause");
    drive(&sink, || engine.status() == PlaybackStatus::Paused);

    // Seek while paused: position should jump to 0.3 (the seek target).
    engine.send(Command::Seek(0.3)).expect("seek");
    drive(&sink, || engine.position_secs() >= 0.295);
    let p = engine.position_secs();
    assert!((p - 0.3).abs() < 0.005, "position after seek {p}");

    engine.send(Command::Resume).expect("resume");
    while engine.status() != PlaybackStatus::Playing {
        sink.pull(64);
        std::thread::sleep(Duration::from_millis(1));
    }

    // Output resumes exactly at frame 0.3 * 44100 = 13230.
    let mut tail = Vec::new();
    pull_frames(&sink, &mut tail, 3000);
    assert_close(
        &tail,
        &expected[13230 * 2..(13230 + 3000) * 2],
        2e-4,
        "post-seek audio",
    );

    let p2 = engine.position_secs();
    assert!(
        (p2 - (0.3 + 3000.0 / f64::from(RATE))).abs() < 0.01,
        "position after post-seek pulls {p2}"
    );
    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

// ---------------------------------------------------------------------------
// (e) volume ramp: no click at SetVolume
// ---------------------------------------------------------------------------

#[test]
fn volume_change_ramps_without_clicks() {
    let (wav, _exp) = stereo_sine_fixture("volume", RATE, 0.5, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    let mut before = Vec::new();
    pull_frames(&sink, &mut before, 2205); // 50 ms at volume 1.0
    engine.send(Command::SetVolume(0.25)).expect("volume");
    let mut after = Vec::new();
    pull_frames(&sink, &mut after, 4410); // 100 ms across the change

    // Largest step between consecutive samples anywhere in the window. The sine
    // itself moves ~0.031/sample at 440 Hz / 0.5 amp; an un-ramped volume jump
    // would add up to ~0.375.
    let mut max_step = 0.0f32;
    for w in [before.windows(2), after.windows(2)] {
        for s in w {
            let d = (s[1] - s[0]).abs();
            if d > max_step {
                max_step = d;
            }
        }
    }
    assert!(
        max_step < 0.05,
        "discontinuity too large: {max_step} (click/zipper noise)"
    );

    // Amplitude actually changed: RMS of the tail of `after` ≈ 25% of `before`.
    let rms = |x: &[f32]| {
        (x.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / x.len() as f64).sqrt()
    };
    let pre_rms = rms(&before);
    let post_tail = &after[after.len() - 882 * 2..];
    let post_rms = rms(post_tail);
    assert!(pre_rms > 0.3, "pre-change rms {pre_rms}");
    assert!(
        (post_rms / pre_rms - 0.25).abs() < 0.03,
        "post-change rms ratio {}",
        post_rms / pre_rms
    );

    // And the change is a ramp: the first samples after the command are still
    // near the old amplitude, not an instant jump.
    let early_peak = after[..662 * 2].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(
        early_peak > 0.4,
        "early peak {early_peak} should still be ~0.5"
    );

    let _ = rx;
}

// ---------------------------------------------------------------------------
// (f) resample: 48 kHz track into 44.1 kHz sink
// ---------------------------------------------------------------------------

#[test]
fn resamples_48k_track_into_44k1_sink() {
    let (wav, _exp) = stereo_sine_fixture("resample-48k", 48_000, 1.0, 440.0);
    let sink = TestSink::new(RATE); // fixed at 44.1 kHz -> engine must resample
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");

    // The failed 48 kHz open attempt surfaces as a non-fatal Error event.
    let started = wait_event(&sink, &rx, |e| matches!(e, Event::TrackStarted { .. }));
    assert!(matches!(
        started,
        Event::TrackStarted { id: 1, duration_secs: Some(d) } if (d - 1.0).abs() < 0.01
    ));

    while engine.status() != PlaybackStatus::Playing {
        sink.pull(64);
        std::thread::sleep(Duration::from_millis(1));
    }

    // Pull the whole queue.
    let mut out = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    loop {
        assert!(Instant::now() < deadline, "queue did not drain");
        out.extend(sink.pull(256));
        match rx.try_recv() {
            Ok(Event::EndOfQueue) => break,
            Ok(_) => {}
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => panic!("channel closed"),
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);

    // Active-signal span (left channel): the resampled 1 s must be 44100 frames
    // within ±1%.
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    let thresh = 0.05f32;
    let first = l
        .iter()
        .position(|s| s.abs() >= thresh)
        .expect("signal start");
    let last = l
        .iter()
        .rposition(|s| s.abs() >= thresh)
        .expect("signal end");
    let span = last - first + 1;
    assert!(
        (span as f64 - 44_100.0).abs() / 44_100.0 < 0.01,
        "resampled span {span} frames (expected ~44100)"
    );

    // Frequency sanity: sign changes in the steady middle ≈ 2 * 440 per second.
    let mid = &l[11_025..33_075]; // 0.5 s window
    let crossings = mid
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    let expected = 2 * 440 / 2; // half-second window
    assert!(
        (crossings as i32 - expected).abs() < expected / 20,
        "zero-crossing count {crossings} (expected ~{expected})"
    );

    // Duration / position sanity.
    let d = engine.duration_secs().expect("duration");
    assert!((d - 1.0).abs() < 0.01);
    let p = engine.position_secs();
    assert!((p - 1.0).abs() < 0.02, "final position {p}");
}

// ---------------------------------------------------------------------------
// (d2) seek while playing (live path): position jumps, playback continues
// ---------------------------------------------------------------------------

#[test]
fn seek_while_playing_jumps_and_continues() {
    let (wav, expected) = stereo_sine_fixture("seek-live", RATE, 0.5, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    let mut head = Vec::new();
    pull_frames(&sink, &mut head, 2205); // 50 ms in
    engine.send(Command::Seek(0.3)).expect("seek");

    // Position must *jump* to ~0.3 — long before natural playback could get
    // there (it was at ~0.05 s and pulls are real-time paced, so reaching 0.29
    // by consuming audio in this window is impossible: it can only be the seek).
    drive(&sink, || engine.position_secs() >= 0.29);
    let p = engine.position_secs();
    assert!((0.29..=0.35).contains(&p), "position after live seek {p}");

    // Playback continues: pull a window and require that a decent stretch of it
    // matches the expected waveform at/after the seek target (13230). The exact
    // boundary alignment is covered by the paused-seek test; here a transient of
    // up to one period of stale/silent audio at the head is tolerated.
    let mut out = Vec::new();
    pull_frames(&sink, &mut out, 2000);
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    let exp_l: Vec<f32> = expected.iter().step_by(2).copied().collect();

    let window = 300usize;
    let mut matched = false;
    'outer: for start in 0..(1500.min(l.len().saturating_sub(window))) {
        for offset in 13230..(13230 + 2000).min(exp_l.len().saturating_sub(window)) {
            let err: f64 = (0..window)
                .map(|f| {
                    let d = f64::from(l[start + f] - exp_l[offset + f]);
                    d * d
                })
                .sum::<f64>()
                / window as f64;
            if err < 1e-8 {
                matched = true;
                break 'outer;
            }
        }
    }
    assert!(matched, "no post-seek waveform found near the seek target");

    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

// ---------------------------------------------------------------------------
// (g) stop clears queue; EnqueueNext while idle starts playback
// ---------------------------------------------------------------------------

#[test]
fn stop_clears_queue_and_enqueue_while_idle_plays() {
    let (wav_a, _a) = stereo_sine_fixture("stop-a", RATE, 0.5, 440.0);
    let (wav_b, _b) = stereo_sine_fixture("stop-b", RATE, 0.5, 660.0);
    let (wav_c, exp_c) = stereo_sine_fixture("stop-c", RATE, 0.25, 880.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(wav_a)))
        .expect("play a");
    start_and_get_playing(&engine, &sink, &rx);
    pull_frames(&sink, &mut Vec::new(), 1000);

    // Queue B, then stop. Note: with the tiny fixture the producer may have
    // already swapped to B (TrackFinished(A) + TrackStarted(B) fire producer-side
    // at the scheduling moment, long before they are audible) — that is legal.
    // What Stop must guarantee: no further events, and silence from now on.
    engine
        .send(Command::EnqueueNext(TrackSource::File(wav_b)))
        .expect("enqueue b");
    std::thread::sleep(Duration::from_millis(20)); // let the producer catch up
    engine.send(Command::Stop).expect("stop");
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);

    // Drain anything that legitimately happened before the Stop took effect.
    while rx.try_recv().is_ok() {}
    // No new events, and the output is silent from here on.
    let mut extra = Vec::new();
    for _ in 0..8 {
        extra.extend(sink.pull(256));
        if rx.try_recv().is_ok() {
            panic!("unexpected event after Stop");
        }
    }
    assert!(extra.iter().all(|s| *s == 0.0));

    // EnqueueNext while idle behaves like Play.
    engine
        .send(Command::EnqueueNext(TrackSource::File(wav_c)))
        .expect("enqueue c");
    let started = start_and_get_playing(&engine, &sink, &rx);
    assert!(matches!(started, Event::TrackStarted { .. }));
    let mut out = Vec::new();
    pull_frames(&sink, &mut out, 2000);
    assert_close(&out, &exp_c[..2000 * 2], 1e-9, "enqueue-while-idle audio");
    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

// ---------------------------------------------------------------------------
// (h) errors: bad files report Error and never wedge the queue
// ---------------------------------------------------------------------------

#[test]
fn missing_file_reports_error_and_engine_survives() {
    let missing = temp_path("does-not-exist");
    let (wav_a, _a) = stereo_sine_fixture("err-after", RATE, 0.25, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    // Playing a missing file from idle: error, stays stopped.
    engine
        .send(Command::Play(TrackSource::File(missing.clone())))
        .expect("send");
    let e = wait_event(&sink, &rx, |e| matches!(e, Event::Error { .. }));
    match e {
        Event::Error { id, msg } => {
            assert_eq!(id, Some(1));
            assert!(!msg.is_empty());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(engine.status(), PlaybackStatus::Stopped);

    // A bad enqueue while playing: error now, current track finishes normally.
    engine
        .send(Command::Play(TrackSource::File(wav_a)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);
    engine
        .send(Command::EnqueueNext(TrackSource::File(missing)))
        .expect("enqueue bad");
    let e2 = wait_event(&sink, &rx, |e| matches!(e, Event::Error { .. }));
    // ids: 1 = first failed Play, 2 = wav_a, 3 = failed EnqueueNext.
    assert!(matches!(e2, Event::Error { id: Some(3), .. }), "{e2:?}");
    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

// ---------------------------------------------------------------------------
// (i) mono -> stereo duplication
// ---------------------------------------------------------------------------

#[test]
fn mono_source_is_duplicated_to_stereo() {
    let pcm = sine_i16(RATE, 11_025, 440.0, 0.5, 1);
    let expected = as_expected_f32(&pcm);
    let path = temp_path("mono");
    write_wav(&path, RATE, 1, &pcm).expect("write mono wav");

    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();
    engine
        .send(Command::Play(TrackSource::File(path)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    let mut out = Vec::new();
    pull_frames(&sink, &mut out, 2000);
    for (frame, pair) in out.as_chunks::<2>().0.iter().enumerate() {
        assert!(
            (f64::from(pair[0]) - f64::from(expected[frame])).abs() < 1e-9
                && (f64::from(pair[1]) - f64::from(expected[frame])).abs() < 1e-9,
            "frame {frame}: L={:?} R={:?} expected {}",
            pair[0],
            pair[1],
            expected[frame]
        );
    }
    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
}

// ---------------------------------------------------------------------------
// (j) track metadata: source rate / channels / average bitrate, cleared on stop
// ---------------------------------------------------------------------------

#[test]
fn metadata_reports_source_rate_channels_and_bitrate() {
    let (wav, _expected) = stereo_sine_fixture("metadata-stereo", RATE, 0.5, 440.0);
    let sink = TestSink::new(RATE);
    let engine = engine_on(&sink);
    let rx = engine.subscribe();

    assert_eq!(engine.metadata(), None, "idle engine has no metadata");

    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    start_and_get_playing(&engine, &sink, &rx);

    let meta = engine.metadata().expect("metadata while playing");
    assert_eq!(meta.sample_rate_hz, RATE);
    assert_eq!(meta.channels, 2);
    let kbps = meta.bitrate_kbps.expect("wav bitrate");
    // 16-bit stereo 44.1 kHz PCM = 1411 kbps (plus a 44-byte header).
    assert!((1400..=1420).contains(&kbps), "kbps {kbps}");

    engine.send(Command::Stop).expect("stop");
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
    assert_eq!(engine.metadata(), None, "metadata cleared on stop");

    // A mono 48 kHz file on a 44.1 kHz sink: metadata follows the SOURCE, not
    // the resampled output.
    let pcm = sine_i16(48_000, 24_000, 440.0, 0.5, 1);
    let path = temp_path("metadata-mono-48k");
    write_wav(&path, 48_000, 1, &pcm).expect("write mono 48k wav");
    engine
        .send(Command::Play(TrackSource::File(path)))
        .expect("play mono");
    start_and_get_playing(&engine, &sink, &rx);

    let meta = engine.metadata().expect("mono metadata");
    assert_eq!(meta.sample_rate_hz, 48_000);
    assert_eq!(meta.channels, 1);

    // Let the queue end; metadata must go with it.
    wait_event(&sink, &rx, |e| matches!(e, Event::EndOfQueue));
    drive(&sink, || engine.status() == PlaybackStatus::Stopped);
    assert_eq!(engine.metadata(), None, "metadata cleared at end of queue");
}
