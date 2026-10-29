//! Throwaway diagnostic: drive /tmp/navamp-test.m4a through the engine and
//! print every event + a sample-count summary. NOT for CI.

use std::time::{Duration, Instant};

use crossbeam_channel::TryRecvError;
use navinamp_audio::{Command, EngineHandle, Event, PlaybackStatus, TestSink, TrackSource};

#[test]
fn m4a_diagnostic() {
    let path = std::path::Path::new("/tmp/navamp-test.m4a");
    if !path.exists() {
        eprintln!("fixture missing — skipping");
        return;
    }

    let sink = TestSink::new(44_100);
    let engine = EngineHandle::start_with_sink(sink.clone()).expect("engine start");
    let events = engine.subscribe();

    engine
        .send(Command::Play(TrackSource::File(path.to_path_buf())))
        .expect("play");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut frames = 0usize;
    let mut started = false;
    let mut errors = Vec::new();

    while Instant::now() < deadline {
        match events.try_recv() {
            Ok(ev) => {
                eprintln!("event: {ev:?}");
                match ev {
                    Event::TrackStarted { .. } => started = true,
                    Event::Error { msg, .. } => errors.push(msg),
                    _ => {}
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => panic!("event channel closed"),
        }

        if engine.status() == PlaybackStatus::Playing {
            let out = sink.pull(128);
            frames += out.len() / 2;
        } else {
            sink.pull(128);
        }

        // Full track (~1.8 s) played to completion: the queue drains and the
        // engine flips back to Stopped.
        if started && matches!(engine.status(), PlaybackStatus::Stopped) && frames > 0 {
            break;
        }
    }

    eprintln!(
        "--- summary: started={started} frames={frames} status={:?}",
        engine.status()
    );
    eprintln!("--- errors: {errors:?}");
    eprintln!("--- underruns: {}", engine.underrun_frames());
    assert!(started, "TrackStarted never fired; errors={errors:?}");
    assert!(
        frames > 44_100,
        "expected a full second of audio, got {frames} frames"
    );
    // The "device cannot open at <rate>, resampling" notice is informational
    // (playback continues through the resampler); anything else is fatal.
    let fatal: Vec<_> = errors
        .iter()
        .filter(|e| !e.contains("resampling"))
        .collect();
    assert!(fatal.is_empty(), "engine reported errors: {fatal:?}");
}
