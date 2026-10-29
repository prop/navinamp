//! Audio bridge: wires `navamp-audio` into the Tauri shell.
//!
//! The engine is started lazily on the first playback command (opening the
//! output device at app launch would be presumptuous). Once running, a
//! forwarder thread subscribes to engine events and re-emits them as Tauri
//! events (`audio-event`) for the UI. Dropping the handle (app exit) shuts the
//! engine down and joins its threads — see `EngineHandle`'s `Drop`.
//!
//! `audio_play`/`audio_enqueue` take a path **or** an `http(s)://` URL (a
//! Navidrome stream URL); URLs play through the engine's remote-stream
//! transport with gapless/seek parity.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use navamp_audio::{
    Command, EngineHandle, EqPreset, EqState, Event, PlaybackStatus, Spectrum, TrackSource,
    eq_presets as eq_preset_table,
};

/// Name of the Tauri event engine events are forwarded to.
const EVENT_NAME: &str = "audio-event";

/// Lazily-initialized engine, shared by all commands. The second slot caches
/// EQ params set before the engine booted (the engine starts flat; the cache
/// is replayed on start so the EQ panel works without playback running).
#[derive(Default)]
pub struct Audio(Mutex<Option<EngineHandle>>, Mutex<EqState>);

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
        // Replay any EQ params configured before the engine existed.
        let cached = *state.1.lock().unwrap();
        engine.eq_set_enabled(cached.enabled);
        engine.eq_set_preamp(cached.preamp_db);
        for (index, db) in cached.bands_db.iter().enumerate() {
            let _ = engine.eq_set_band(index as u32, *db);
        }
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

/// Run an EQ mutation against a running engine (no-op before the first
/// playback command starts the engine — the device is deliberately not
/// opened just for EQ twiddling; settings live for the engine's lifetime).
fn eq_mutate(state: &Audio, f: impl FnOnce(&EngineHandle)) -> Result<(), String> {
    let guard = state.0.lock().unwrap();
    if let Some(engine) = guard.as_ref() {
        f(engine);
    }
    Ok(())
}

/// Maps a UI-provided `path` to a engine source: `http(s)://` URLs play as
/// remote streams (Navidrome), everything else is a local file. The IPC
/// signature stays a plain string on the JS side.
fn track_source(path: String) -> TrackSource {
    if path.starts_with("http://") || path.starts_with("https://") {
        TrackSource::Remote { url: path }
    } else {
        TrackSource::File(path.into())
    }
}

#[tauri::command]
pub fn audio_play(app: AppHandle, state: State<'_, Audio>, path: String) -> Result<(), String> {
    ensure_engine(&app, &state)?;
    send_if_running(&state, || Command::Play(track_source(path)))
}

#[tauri::command]
pub fn audio_enqueue(state: State<'_, Audio>, path: String) -> Result<(), String> {
    send_if_running(&state, || Command::EnqueueNext(track_source(path)))
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

// ---------------------------------------------------------------------------
// Equaliser (M6a). State lives in the engine's lock-free parameter store;
// the RT chain snapshots it per block with a generation counter.
// ---------------------------------------------------------------------------

/// The current EQ state (cached params before an engine exists, so the
/// panel reflects and remembers pre-playback configuration).
#[tauri::command]
pub fn eq_state(state: State<'_, Audio>) -> EqState {
    let guard = state.0.lock().unwrap();
    match guard.as_ref() {
        Some(engine) => engine.eq_state(),
        None => *state.1.lock().unwrap(),
    }
}

#[tauri::command]
pub fn eq_set_enabled(state: State<'_, Audio>, enabled: bool) -> Result<(), String> {
    state.1.lock().unwrap().enabled = enabled;
    eq_mutate(&state, |engine| engine.eq_set_enabled(enabled))
}

/// Preamp in dB; clamped server-side to −12..=12.
#[tauri::command]
pub fn eq_set_preamp(state: State<'_, Audio>, db: f32) -> Result<(), String> {
    state.1.lock().unwrap().preamp_db = db.clamp(-12.0, 12.0);
    eq_mutate(&state, |engine| engine.eq_set_preamp(db))
}

/// One band's gain in dB; `index` 0..=9 (else `Err`), value clamped to
/// −12..=12.
#[tauri::command]
pub fn eq_set_band(state: State<'_, Audio>, index: u32, db: f32) -> Result<(), String> {
    {
        let mut cache = state.1.lock().unwrap();
        let Some(slot) = cache.bands_db.get_mut(index as usize) else {
            return Err("eq band index must be 0..=9".into());
        };
        *slot = db.clamp(-12.0, 12.0);
    }
    let guard = state.0.lock().unwrap();
    match guard.as_ref() {
        Some(engine) => engine.eq_set_band(index, db).map_err(|e| e.to_string()),
        None => Ok(()), // cached; replayed when the engine boots
    }
}

/// The classic Winamp preset curves (static; no engine needed).
#[tauri::command]
pub fn eq_presets() -> Vec<EqPreset> {
    eq_preset_table().to_vec()
}

// ---------------------------------------------------------------------------
// Analyser (M6a). Computed on demand from the post-DSP tap; the audio
// callback only ever copies frames into it.
// ---------------------------------------------------------------------------

/// One spectrum/waveform snapshot (`active` is false unless playing).
#[tauri::command]
pub fn audio_spectrum(state: State<'_, Audio>) -> Spectrum {
    let guard = state.0.lock().unwrap();
    match guard.as_ref() {
        Some(engine) => engine.spectrum(),
        None => Spectrum::default(),
    }
}
