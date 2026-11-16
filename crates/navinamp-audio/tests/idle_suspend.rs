//! The engine's output-suspend lifecycle (idle-power contract).
//!
//! A real output device drops its OS audio power assertion while suspended,
//! so NaVinaMP must not hold a suspended device unless playback is actually
//! live — and must resume it the instant playback needs the callback (ring
//! reset handshakes complete inside the callback). [`TestSink::suspended`]
//! mirrors what a real device would do, so these assertions run everywhere
//! CI runs.

use std::time::{Duration, Instant};

use navinamp_audio::{Command, EngineHandle, PlaybackStatus, TestSink, TrackSource};

mod common;

use common::{sine_i16, temp_path, write_wav};

/// Poll `cond` until it holds or the deadline passes.
fn wait_for(cond: impl Fn() -> bool, desc: &str) {
    wait_for_at_most(Duration::from_secs(2), cond, desc);
}

fn wait_for_at_most(limit: Duration, cond: impl Fn() -> bool, desc: &str) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for: {desc}");
}

fn tone(name: &str, secs: f32) -> std::path::PathBuf {
    let path = temp_path(name);
    let frames = (44_100.0 * secs) as usize;
    let samples = sine_i16(44_100, frames, 440.0, 0.5, 2);
    write_wav(&path, 44_100, 2, &samples).expect("write wav");
    path
}

#[test]
fn idle_engine_suspends_the_device() {
    let sink = TestSink::new(44_100);
    let engine = EngineHandle::start_with_sink(sink.clone()).expect("start");

    // Nothing was ever played: the device must be suspended (the producer's
    // first sync_output runs before it blocks on the command channel).
    wait_for(|| sink.suspended(), "idle engine suspends the device");
    drop(engine);
}

#[test]
fn device_tracks_the_playback_lifecycle() {
    let wav = tone("suspend-lifecycle", 2.0);
    let sink = TestSink::new(44_100);
    let engine = EngineHandle::start_with_sink(sink.clone()).expect("start");

    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    wait_for(|| !sink.suspended(), "playing engine resumes the device");
    wait_for(
        || engine.status() == PlaybackStatus::Playing,
        "engine reports playing",
    );

    engine.send(Command::Pause).expect("pause");
    wait_for(|| sink.suspended(), "paused engine suspends the device");

    engine.send(Command::Resume).expect("resume");
    wait_for(|| sink.suspended(), "resumed engine resumes the device");

    engine.send(Command::Stop).expect("stop");
    wait_for(|| sink.suspended(), "stopped engine suspends the device");
    wait_for(
        || engine.status() == PlaybackStatus::Stopped,
        "engine reports stopped",
    );
}

#[test]
fn seek_while_paused_completes_and_re_suspends() {
    let wav = tone("suspend-seek", 5.0);
    let sink = TestSink::new(44_100);
    let engine = EngineHandle::start_with_sink(sink.clone()).expect("start");

    engine
        .send(Command::Play(TrackSource::File(wav)))
        .expect("play");
    wait_for(
        || engine.status() == PlaybackStatus::Playing,
        "engine reports playing",
    );

    engine.send(Command::Pause).expect("pause");
    wait_for(|| sink.suspended(), "paused engine suspends the device");

    // A seek while the device is suspended stages the post-seek audio and
    // runs the ring-reset handshake. Media time does not move while paused
    // (nothing is consumed), so the position still reads the pre-seek value
    // until playback resumes and crosses into the seeked range.
    engine.send(Command::Seek(2.5)).expect("seek");
    assert!(sink.suspended(), "still paused: device stays suspended");
    assert_eq!(engine.status(), PlaybackStatus::Paused);
    assert!(
        engine.position_secs() < 2.5,
        "nothing consumed while paused"
    );

    // Resuming plays from the seeked position: the device comes back live
    // and consumption crosses the seek point.
    engine.send(Command::Resume).expect("resume");
    wait_for(|| sink.suspended(), "resumed engine resumes the device");

    // Drive playback through pulls (the test sink is pull-driven): position
    // only advances with consumption, and it crosses the seek point once
    // played frames reach the seek session's start.
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && engine.position_secs() < 2.5 {
        sink.pull(2048);
    }
    assert!(
        engine.position_secs() >= 2.5,
        "position = {}",
        engine.position_secs()
    );
    assert!(!sink.suspended(), "playing engine keeps the device live");
}
