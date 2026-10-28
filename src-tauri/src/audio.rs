//! Audio bridge: wires `navamp-audio` into the Tauri shell.
//!
//! The engine is started lazily on the first playback command (opening the
//! output device at app launch would be presumptuous). Once running, a
//! forwarder thread subscribes to engine events and re-emits them as Tauri
//! events (`audio-event`) for the UI. Dropping the handle (app exit) shuts the
//! engine down and joins its threads — see `EngineHandle`'s `Drop`.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use navamp_audio::{Command, EngineHandle, Event, PlaybackStatus, TrackSource};

/// Name of the Tauri event engine events are forwarded to.
const EVENT_NAME: &str = "audio-event";

/// Lazily-initialized engine, shared by all commands.
#[derive(Default)]
pub struct Audio(Mutex<Option<EngineHandle>>);

/// Snapshot returned by [`audio_state`] (polled by the UI at ~10 Hz).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStatePayload {
    pub status: String,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub volume: f32,
    pub underrun_frames: u64,
}

fn status_name(s: PlaybackStatus) -> &'static str {
    match s {
        PlaybackStatus::Stopped => "stopped",
        PlaybackStatus::Playing => "playing",
        PlaybackStatus::Paused => "paused",
    }
}

/// Start the engine if it is not running yet. Errors surface to the UI
/// (e.g. no output device).
fn ensure_engine(app: &AppHandle, state: &Audio) -> Result<(), String> {
    let mut guard = state.0.lock().unwrap();
    if guard.is_none() {
        let engine = EngineHandle::start().map_err(|e| e.to_string())?;
        spawn_event_forwarder(app.clone(), &engine);
        *guard = Some(engine);
    }
    Ok(())
}

/// Re-emit every engine event as a Tauri event with a JSON payload. The thread
/// exits when the engine (and thus the event source) is dropped.
fn spawn_event_forwarder(app: AppHandle, engine: &EngineHandle) {
    let events = engine.subscribe();
    std::thread::Builder::new()
        .name("navamp-audio-events".into())
        .spawn(move || {
            for event in events {
                let payload = match &event {
                    Event::TrackStarted { id, duration_secs } => serde_json::json!({
                        "type": "TrackStarted",
                        "id": id,
                        "durationSecs": duration_secs,
                    }),
                    Event::TrackFinished { id } => serde_json::json!({
                        "type": "TrackFinished",
                        "id": id,
                    }),
                    Event::EndOfQueue => serde_json::json!({ "type": "EndOfQueue" }),
                    Event::Error { id, msg } => serde_json::json!({
                        "type": "Error",
                        "id": id,
                        "msg": msg,
                    }),
                };
                // Emit failures (app shutting down) just end the forwarder.
                if app.emit(EVENT_NAME, payload).is_err() {
                    break;
                }
            }
        })
        .expect("spawn audio event forwarder");
}

/// Run a command against a running engine, or `Ok(())` when none exists yet
/// (pause/seek on a never-started engine are harmless no-ops).
fn send_if_running(state: &Audio, make: impl FnOnce() -> Command) -> Result<(), String> {
    let guard = state.0.lock().unwrap();
    match guard.as_ref() {
        Some(engine) => engine.send(make()).map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

#[tauri::command]
pub fn audio_play(
    app: AppHandle,
    state: State<'_, Audio>,
    path: String,
) -> Result<(), String> {
    ensure_engine(&app, &state)?;
    send_if_running(&state, || Command::Play(TrackSource::File(path.into())))
}

#[tauri::command]
pub fn audio_enqueue(state: State<'_, Audio>, path: String) -> Result<(), String> {
    send_if_running(&state, || Command::EnqueueNext(TrackSource::File(path.into())))
}

#[tauri::command]
pub fn audio_pause(state: State<'_, Audio>) -> Result<(), String> {
    send_if_running(&state, || Command::Pause)
}

#[tauri::command]
pub fn audio_resume(state: State<'_, Audio>) -> Result<(), String> {
    send_if_running(&state, || Command::Resume)
}

#[tauri::command]
pub fn audio_stop(state: State<'_, Audio>) -> Result<(), String> {
    send_if_running(&state, || Command::Stop)
}

#[tauri::command]
pub fn audio_seek(state: State<'_, Audio>, secs: f64) -> Result<(), String> {
    send_if_running(&state, || Command::Seek(secs))
}

#[tauri::command]
pub fn audio_set_volume(state: State<'_, Audio>, volume: f32) -> Result<(), String> {
    send_if_running(&state, || Command::SetVolume(volume.clamp(0.0, 1.0)))
}

#[tauri::command]
pub fn audio_state(state: State<'_, Audio>) -> AudioStatePayload {
    let guard = state.0.lock().unwrap();
    match guard.as_ref() {
        Some(engine) => AudioStatePayload {
            status: status_name(engine.status()).into(),
            position_secs: engine.position_secs(),
            duration_secs: engine.duration_secs(),
            volume: engine.volume(),
            underrun_frames: engine.underrun_frames(),
        },
        None => AudioStatePayload {
            status: status_name(PlaybackStatus::Stopped).into(),
            position_secs: 0.0,
            duration_secs: None,
            volume: 1.0,
            underrun_frames: 0,
        },
    }
}
