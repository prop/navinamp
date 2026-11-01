//! Audio bridge: wires `navinamp-audio` into the Tauri shell.
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

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use navinamp_audio::{
    Command, EQ_BAND_COUNT, EQ_RANGE_DB, EngineHandle, EqState, Event, PlaybackStatus, Spectrum,
    TrackSource, eq_presets as eq_preset_table,
};

/// Name of the Tauri event engine events are forwarded to.
const EVENT_NAME: &str = "audio-event";
/// Volume + EQ preferences file inside the app config dir.
const PREFS_FILE: &str = "audio.json";

/// Lazily-initialized engine, shared by all commands. The caches hold the
/// volume/EQ params configured before the engine booted (the engine starts
/// flat; they are replayed on start so the panels work without playback
/// running) and are the values persisted across restarts.
pub struct Audio {
    engine: Mutex<Option<EngineHandle>>,
    eq: Mutex<EqState>,
    volume: Mutex<f32>,
    /// User-saved EQ curves (name + bands), persisted in `audio.json`.
    presets: Mutex<Vec<StoredPreset>>,
    /// Debounced writer channel; `None` before [`init`] (and in tests).
    prefs_tx: Mutex<Option<Sender<StoredPrefs>>>,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            engine: Mutex::new(None),
            eq: Mutex::new(EqState::default()),
            volume: Mutex::new(1.0),
            presets: Mutex::new(Vec::new()),
            prefs_tx: Mutex::new(None),
        }
    }
}

/// On-disk shape of `audio.json` (tolerant: missing fields default).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredPrefs {
    volume: f32,
    eq: StoredEq,
    /// User-saved presets (built-in Winamp curves stay in the audio crate).
    eq_presets: Vec<StoredPreset>,
}

impl Default for StoredPrefs {
    fn default() -> Self {
        Self {
            volume: 1.0,
            eq: StoredEq::default(),
            eq_presets: Vec::new(),
        }
    }
}

/// One user-saved EQ curve.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredPreset {
    name: String,
    bands_db: Vec<f32>,
}

/// EQ portion of the prefs file. Bands are a Vec so a short/long array in a
/// hand-edited file cannot fail the parse; [`Self::sanitized`] pads/clamps.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredEq {
    enabled: bool,
    preamp_db: f32,
    bands_db: Vec<f32>,
}

impl From<EqState> for StoredEq {
    fn from(state: EqState) -> Self {
        Self {
            enabled: state.enabled,
            preamp_db: state.preamp_db,
            bands_db: state.bands_db.to_vec(),
        }
    }
}

/// Clamp a bands array into the engine range, padding/truncating to 10.
fn normalize_bands(values: &[f32]) -> [f32; EQ_BAND_COUNT] {
    let db = |value: f32| {
        if value.is_finite() {
            value.clamp(-EQ_RANGE_DB, EQ_RANGE_DB)
        } else {
            0.0
        }
    };
    let mut bands = [0.0; EQ_BAND_COUNT];
    for (slot, value) in bands.iter_mut().zip(values.iter()) {
        *slot = db(*value);
    }
    bands
}

impl StoredPrefs {
    /// Clamp everything into the engine's accepted ranges.
    fn sanitized(self) -> (f32, EqState, Vec<StoredPreset>) {
        let volume = if self.volume.is_finite() {
            self.volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let eq = EqState {
            enabled: self.eq.enabled,
            preamp_db: normalize_bands(&[self.eq.preamp_db])[0],
            bands_db: normalize_bands(&self.eq.bands_db),
        };

        // Names are trimmed/deduped case-insensitively; built-in names are
        // reserved. Malformed entries are dropped rather than failing open.
        let mut presets: Vec<StoredPreset> = Vec::new();
        for preset in self.eq_presets {
            let name = preset.name.trim().to_owned();
            if name.is_empty() || name.chars().count() > 40 {
                continue;
            }
            if eq_preset_table()
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            let entry = StoredPreset {
                name,
                bands_db: normalize_bands(&preset.bands_db).to_vec(),
            };
            match presets
                .iter_mut()
                .find(|p| p.name.eq_ignore_ascii_case(&entry.name))
            {
                Some(existing) => *existing = entry,
                None => presets.push(entry),
            }
        }
        presets.sort_by_key(|p| p.name.to_lowercase());
        (volume, eq, presets)
    }
}

/// Load persisted volume/EQ and start the debounced writer. Called from the
/// app setup hook, before any window can issue audio commands.
pub fn init(app: &AppHandle) {
    let Ok(dir) = app.path().app_config_dir() else {
        return; // no config dir: run with defaults, nothing to persist
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(PREFS_FILE);
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(stored) = serde_json::from_str::<StoredPrefs>(&text)
    {
        let state = app.state::<Audio>();
        let (volume, eq, presets) = stored.sanitized();
        *state.volume.lock().unwrap() = volume;
        *state.eq.lock().unwrap() = eq;
        *state.presets.lock().unwrap() = presets;
    }

    let (tx, rx) = channel();
    *app.state::<Audio>().prefs_tx.lock().unwrap() = Some(tx);
    spawn_prefs_writer(path, rx);
}

/// Coalesce rapid changes (slider drags) into at most one write per quiet
/// window; a final write happens when the channel disconnects at app exit.
fn spawn_prefs_writer(path: PathBuf, rx: Receiver<StoredPrefs>) {
    let _ = std::thread::Builder::new()
        .name("navinamp-audio-prefs".into())
        .spawn(move || {
            let mut pending: Option<StoredPrefs> = None;
            loop {
                match rx.recv_timeout(Duration::from_millis(400)) {
                    Ok(prefs) => pending = Some(prefs),
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(prefs) = pending.take() {
                            write_prefs(&path, &prefs);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        if let Some(prefs) = pending.take() {
                            write_prefs(&path, &prefs);
                        }
                        return;
                    }
                }
            }
        });
}

fn write_prefs(path: &Path, prefs: &StoredPrefs) {
    let Ok(text) = serde_json::to_string_pretty(prefs) else {
        return;
    };
    if let Err(e) = std::fs::write(path, text) {
        eprintln!("navinamp audio: cannot write `{}`: {e}", path.display());
    }
}

/// Queue the current volume/EQ state for persistence (debounced by the
/// writer thread). A no-op before [`init`].
fn persist(state: &Audio) {
    let prefs = StoredPrefs {
        volume: *state.volume.lock().unwrap(),
        eq: StoredEq::from(*state.eq.lock().unwrap()),
        eq_presets: state.presets.lock().unwrap().clone(),
    };
    if let Some(tx) = state.prefs_tx.lock().unwrap().as_ref() {
        let _ = tx.send(prefs);
    }
}

/// Snapshot returned by [`audio_state`] (polled by the UI at ~10 Hz).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStatePayload {
    pub status: String,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub volume: f32,
    pub underrun_frames: u64,
    /// Average bitrate of the current track in kbps (classic kbps readout).
    pub bitrate_kbps: Option<u32>,
    /// Source sample rate in Hz (classic kHz readout).
    pub sample_rate_hz: Option<u32>,
    /// Source channel count (mono/stereo indicators).
    pub channels: Option<u16>,
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
    let mut guard = state.engine.lock().unwrap();
    if guard.is_none() {
        let engine = EngineHandle::start().map_err(|e| e.to_string())?;
        spawn_event_forwarder(app.clone(), &engine);
        // Replay the volume/EQ configured before the engine existed.
        let cached_volume = *state.volume.lock().unwrap();
        let _ = engine.send(Command::SetVolume(cached_volume));
        let cached = *state.eq.lock().unwrap();
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
        .name("navinamp-audio-events".into())
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
    let guard = state.engine.lock().unwrap();
    match guard.as_ref() {
        Some(engine) => engine.send(make()).map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

/// Run an EQ mutation against a running engine (no-op before the first
/// playback command starts the engine — the device is deliberately not
/// opened just for EQ twiddling; settings live for the engine's lifetime).
fn eq_mutate(state: &Audio, f: impl FnOnce(&EngineHandle)) -> Result<(), String> {
    let guard = state.engine.lock().unwrap();
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
    let volume = volume.clamp(0.0, 1.0);
    *state.volume.lock().unwrap() = volume;
    let result = send_if_running(&state, || Command::SetVolume(volume));
    persist(&state);
    result
}

#[tauri::command]
pub fn audio_state(state: State<'_, Audio>) -> AudioStatePayload {
    let guard = state.engine.lock().unwrap();
    match guard.as_ref() {
        Some(engine) => {
            let meta = engine.metadata();
            AudioStatePayload {
                status: status_name(engine.status()).into(),
                position_secs: engine.position_secs(),
                duration_secs: engine.duration_secs(),
                volume: engine.volume(),
                underrun_frames: engine.underrun_frames(),
                bitrate_kbps: meta.and_then(|m| m.bitrate_kbps),
                sample_rate_hz: meta.map(|m| m.sample_rate_hz),
                channels: meta.map(|m| m.channels),
            }
        }
        None => AudioStatePayload {
            status: status_name(PlaybackStatus::Stopped).into(),
            position_secs: 0.0,
            duration_secs: None,
            // The cached target before the engine boots (persisted value).
            volume: *state.volume.lock().unwrap(),
            underrun_frames: 0,
            bitrate_kbps: None,
            sample_rate_hz: None,
            channels: None,
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
    let guard = state.engine.lock().unwrap();
    match guard.as_ref() {
        Some(engine) => engine.eq_state(),
        None => *state.eq.lock().unwrap(),
    }
}

#[tauri::command]
pub fn eq_set_enabled(state: State<'_, Audio>, enabled: bool) -> Result<(), String> {
    state.eq.lock().unwrap().enabled = enabled;
    let result = eq_mutate(&state, |engine| engine.eq_set_enabled(enabled));
    persist(&state);
    result
}

/// Preamp in dB; clamped server-side to −12..=12.
#[tauri::command]
pub fn eq_set_preamp(state: State<'_, Audio>, db: f32) -> Result<(), String> {
    let db = db.clamp(-EQ_RANGE_DB, EQ_RANGE_DB);
    state.eq.lock().unwrap().preamp_db = db;
    let result = eq_mutate(&state, |engine| engine.eq_set_preamp(db));
    persist(&state);
    result
}

/// One band's gain in dB; `index` 0..=9 (else `Err`), value clamped to
/// −12..=12.
#[tauri::command]
pub fn eq_set_band(state: State<'_, Audio>, index: u32, db: f32) -> Result<(), String> {
    let db = db.clamp(-EQ_RANGE_DB, EQ_RANGE_DB);
    {
        let mut cache = state.eq.lock().unwrap();
        let Some(slot) = cache.bands_db.get_mut(index as usize) else {
            return Err("eq band index must be 0..=9".into());
        };
        *slot = db;
    }
    let result = {
        let guard = state.engine.lock().unwrap();
        match guard.as_ref() {
            Some(engine) => engine.eq_set_band(index, db).map_err(|e| e.to_string()),
            None => Ok(()), // cached; replayed when the engine boots
        }
    };
    persist(&state);
    result
}

/// One entry in the preset dropdown: a built-in classic curve or a user save.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqPresetEntry {
    pub name: String,
    pub bands_db: [f32; EQ_BAND_COUNT],
    /// True for presets saved by the user (deletable, editable by re-saving).
    pub user: bool,
}

fn preset_entries(state: &Audio) -> Vec<EqPresetEntry> {
    let mut out: Vec<EqPresetEntry> = eq_preset_table()
        .iter()
        .map(|p| EqPresetEntry {
            name: p.name.to_owned(),
            bands_db: p.bands_db,
            user: false,
        })
        .collect();
    out.extend(state.presets.lock().unwrap().iter().map(|p| EqPresetEntry {
        name: p.name.clone(),
        bands_db: normalize_bands(&p.bands_db),
        user: true,
    }));
    out
}

/// The built-in classic Winamp curves plus the user's saved presets.
#[tauri::command]
pub fn eq_presets(state: State<'_, Audio>) -> Vec<EqPresetEntry> {
    preset_entries(&state)
}

/// Save the given band curve under `name` (upsert by case-insensitive name).
/// Built-in names are reserved; the name is trimmed and capped at 40 chars.
#[tauri::command]
pub fn eq_save_preset(
    state: State<'_, Audio>,
    name: String,
    bands_db: Vec<f32>,
) -> Result<Vec<EqPresetEntry>, String> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err("preset name must not be empty".into());
    }
    if name.chars().count() > 40 {
        return Err("preset name is too long (max 40 characters)".into());
    }
    if eq_preset_table()
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case(&name))
    {
        return Err(format!("`{name}` is a built-in preset"));
    }
    let entry = StoredPreset {
        name: name.clone(),
        bands_db: normalize_bands(&bands_db).to_vec(),
    };
    {
        let mut presets = state.presets.lock().unwrap();
        match presets
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(&name))
        {
            Some(existing) => *existing = entry,
            None => presets.push(entry),
        }
        presets.sort_by_key(|p| p.name.to_lowercase());
    }
    persist(&state);
    Ok(preset_entries(&state))
}

/// Delete a user preset by name. Built-in curves cannot be deleted.
#[tauri::command]
pub fn eq_delete_preset(
    state: State<'_, Audio>,
    name: String,
) -> Result<Vec<EqPresetEntry>, String> {
    let name = name.trim();
    {
        let mut presets = state.presets.lock().unwrap();
        let before = presets.len();
        presets.retain(|p| !p.name.eq_ignore_ascii_case(name));
        if presets.len() == before {
            return Err(format!("no saved preset named `{name}`"));
        }
    }
    persist(&state);
    Ok(preset_entries(&state))
}

// ---------------------------------------------------------------------------
// Analyser (M6a). Computed on demand from the post-DSP tap; the audio
// callback only ever copies frames into it.
// ---------------------------------------------------------------------------

/// One spectrum/waveform snapshot (`active` is false unless playing).
#[tauri::command]
pub fn audio_spectrum(state: State<'_, Audio>) -> Spectrum {
    let guard = state.engine.lock().unwrap();
    match guard.as_ref() {
        Some(engine) => engine.spectrum(),
        None => Spectrum::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_state_payload_serializes_metadata_camel_case() {
        let payload = AudioStatePayload {
            status: "playing".into(),
            position_secs: 12.5,
            duration_secs: Some(210.0),
            volume: 0.8,
            underrun_frames: 0,
            bitrate_kbps: Some(192),
            sample_rate_hz: Some(44_100),
            channels: Some(2),
        };
        let v = serde_json::to_value(&payload).expect("serialize");
        assert_eq!(v["bitrateKbps"], 192);
        assert_eq!(v["sampleRateHz"], 44_100);
        assert_eq!(v["channels"], 2);
    }

    #[test]
    fn idle_payload_serializes_null_metadata() {
        let payload = AudioStatePayload {
            status: "stopped".into(),
            position_secs: 0.0,
            duration_secs: None,
            volume: 1.0,
            underrun_frames: 0,
            bitrate_kbps: None,
            sample_rate_hz: None,
            channels: None,
        };
        let v = serde_json::to_value(&payload).expect("serialize");
        assert!(v["bitrateKbps"].is_null());
        assert!(v["sampleRateHz"].is_null());
        assert!(v["channels"].is_null());
    }

    #[test]
    fn stored_prefs_sanitize_into_engine_ranges() {
        let stored: StoredPrefs = serde_json::from_str(
            r#"{ "volume": 1.7, "eq": { "enabled": true, "preampDb": -30,
                "bandsDb": [1, -99] },
                "eqPresets": [
                    { "name": "  My curve  ", "bandsDb": [3, 99] },
                    { "name": "my CURVE", "bandsDb": [4] },
                    { "name": "Rock", "bandsDb": [1] },
                    { "name": "   ", "bandsDb": [1] }
                ] }"#,
        )
        .expect("tolerant parse");
        let (volume, eq, presets) = stored.sanitized();
        assert_eq!(volume, 1.0, "volume clamps to 0..=1");
        assert!(eq.enabled);
        assert_eq!(eq.preamp_db, -EQ_RANGE_DB);
        assert_eq!(eq.bands_db[0], 1.0);
        assert_eq!(eq.bands_db[1], -EQ_RANGE_DB);
        // A short array pads with flat bands.
        assert_eq!(&eq.bands_db[2..], &[0.0; EQ_BAND_COUNT - 2]);

        // Case-insensitive dedupe keeps the last copy; built-in names and
        // blank names are dropped.
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "my CURVE");
        assert_eq!(presets[0].bands_db[0], 4.0);
        assert_eq!(presets[0].bands_db[1], 0.0);
    }

    #[test]
    fn stored_prefs_defaults_and_round_trips_camel_case() {
        let stored: StoredPrefs = serde_json::from_str("{}").expect("empty object");
        let (volume, eq, presets) = stored.sanitized();
        assert_eq!(volume, 1.0, "default volume is unity");
        assert_eq!(eq, EqState::default());
        assert!(presets.is_empty());

        let value = serde_json::to_value(StoredPrefs {
            volume: 0.5,
            eq: StoredEq::from(EqState {
                enabled: true,
                preamp_db: 1.5,
                bands_db: [2.0; EQ_BAND_COUNT],
            }),
            eq_presets: vec![StoredPreset {
                name: "Mine".into(),
                bands_db: vec![1.0; EQ_BAND_COUNT],
            }],
        })
        .expect("serialize");
        assert_eq!(value["volume"], 0.5);
        assert_eq!(value["eq"]["enabled"], true);
        assert_eq!(value["eq"]["preampDb"], 1.5);
        assert_eq!(value["eq"]["bandsDb"][0], 2.0);
        assert_eq!(value["eqPresets"][0]["name"], "Mine");
    }
}
