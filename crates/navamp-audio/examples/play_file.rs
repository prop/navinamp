//! Play an audio file through the NavAmp engine and print events.
//!
//! Usage: `cargo run -p navamp-audio --example play_file -- <file>`
//!
//! Ctrl-C exits cleanly (Shutdown command + engine join).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::RecvTimeoutError;
use navamp_audio::{Command, EngineHandle, TrackSource};

fn main() {
    let path: PathBuf = match std::env::args().nth(1) {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: play_file <audio-file>");
            std::process::exit(2);
        }
    };

    let engine = match EngineHandle::start() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("failed to open audio output: {e}");
            std::process::exit(1);
        }
    };

    let events = engine.subscribe();
    let done = Arc::new(AtomicBool::new(false));

    {
        let done = done.clone();
        let cmd = engine.command_sender();
        ctrlc::set_handler(move || {
            done.store(true, Ordering::SeqCst);
            let _ = cmd.send(Command::Shutdown);
        })
        .expect("failed to install Ctrl-C handler");
    }

    if let Err(e) = engine.send(Command::Play(TrackSource::File(path))) {
        eprintln!("send failed: {e}");
        std::process::exit(1);
    }

    loop {
        if done.load(Ordering::SeqCst) {
            break;
        }
        match events.recv_timeout(Duration::from_millis(250)) {
            Ok(ev) => println!("[event] {ev:?}"),
            Err(RecvTimeoutError::Timeout) => println!(
                "[{:.1}s / {}] vol {:.2}  status {:?}",
                engine.position_secs(),
                engine
                    .duration_secs()
                    .map_or("??.?s".to_string(), |d| format!("{d:.1}s")),
                engine.volume(),
                engine.status(),
            ),
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    engine.shutdown();
    println!("bye");
}
