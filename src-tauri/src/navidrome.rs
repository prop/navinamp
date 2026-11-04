//! Navidrome (Subsonic) bridge: credentials, connection state, library sync
//! and remote-playback helpers.
//!
//! Layout:
//! - credentials + streaming prefs live at `<app_data_dir>/navidrome.json`
//!   (**plaintext v1** — password and all; a keyring backend is a later
//!   milestone, flagged in the README). The file also carries `syncedAt`
//!   and `serverVersion` so the status survives restarts;
//! - the Subsonic client is async, so `connect`/`scrobble`/`star` and the
//!   lazily-built client cache run as async commands on Tauri's runtime.
//!   The **sync walk runs on a `std::thread`** (like `library_scan`), with
//!   each client call driven by `tauri::async_runtime::block_on` from that
//!   dedicated thread;
//! - synced rows land through `navinamp-library`'s `sync_remote` (one
//!   transaction, local rows untouched); the sync thread uses its **own**
//!   database connection so UI queries stay responsive (WAL).
//!
//! Events (frozen IPC contract):
//! - `navidrome-progress` — `{ scanned, total }` in **track** units,
//!   throttled to every 32 albums (plus the first and last). `total` is the
//!   sum of the server's album `songCount`s from a lightweight album-list
//!   pass; it is 0 when that pass fails, and the UI then shows the scanned
//!   track count without a denominator;
//! - `navidrome-synced` — the final [`SyncReport`] (or `{ error }`).
//!
//! Credential hygiene: Subsonic stream/API URLs embed the auth token;
//! every error string that crosses IPC is scrubbed of URLs (`<url>`).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use navinamp_library::{Library, RemoteAlbum, RemoteTrack, Track, remote_path, repair_tag_text};
use navinamp_subsonic::{
    AlbumDetail, AlbumSummary, MusicFolder, PlaylistSummary, SongSummary, StreamFormat,
    SubsonicClient,
};

use crate::library::{LibraryState, db_file, with_library};

/// `navidrome-progress` event name.
const PROGRESS_EVENT: &str = "navidrome-progress";
/// `navidrome-synced` event name.
const SYNCED_EVENT: &str = "navidrome-synced";
/// Emit progress at most every N albums (plus the very last one). Values in
/// the event are tracks, but the cadence stays album-based so a huge library
/// does not emit an event per song.
const PROGRESS_EVERY: u64 = 32;
/// Page size for the pre-walk `getAlbumList2` pass that sums `songCount`.
const TRACK_COUNT_PAGE: u32 = 500;
/// Concurrent `getAlbum` requests during a sync. Bounded to stay friendly to
/// modest servers while cutting a large full rescan's wall time.
const SYNC_CONCURRENCY: usize = 6;
/// Flush decoded rows to the library at least every N tracks (so the UI can
/// render partial results during a long sync) or on every progress tick.
const SYNC_BATCH_TRACKS: usize = 512;
/// The `source` value for synced rows.
const SOURCE: &str = "navidrome";

// ---------------------------------------------------------------------------
// persisted config
// ---------------------------------------------------------------------------

/// One saved Navidrome server. The streaming prefs, library selection and
/// sync bookkeeping are per server so switching between profiles keeps each
/// server's setup. `id` is stable for the life of the profile (playlist
/// mappings are keyed by it); `label` is user-editable (`None` = derive
/// `user@host`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredProfile {
    id: String,
    label: Option<String>,
    /// Server base URL (`https://music.example.com` — `/rest` is optional).
    url: String,
    user: String,
    /// Plaintext v1; see the module docs.
    password: String,
    /// `original` | `mp3` | `opus`.
    format: String,
    bitrate_kbps: Option<u32>,
    synced_at: Option<i64>,
    server_version: Option<String>,
    /// Selected Navidrome library (music folder); `None` = all libraries.
    library_id: Option<String>,
}

impl StoredProfile {
    /// The name shown in the UI: the custom label, else `user@host`.
    fn name(&self) -> String {
        self.label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{}@{}", self.user, host_of(&self.url)))
    }

    fn stream_format(&self) -> Option<StreamFormat> {
        parse_format(&self.format)
    }
}

/// Contents of `<app_data_dir>/navidrome.json` (v2): a list of saved servers
/// and the one currently active. The v1 file was a single [`StoredProfile`]
/// shape; [`parse_config`] wraps it as one `default` profile so 0.6 installs
/// keep their credentials and library selection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredConfig {
    profiles: Vec<StoredProfile>,
    active_id: Option<String>,
}

impl StoredConfig {
    fn active(&self) -> Option<&StoredProfile> {
        let id = self.active_id.as_deref()?;
        self.profiles.iter().find(|profile| profile.id == id)
    }

    fn active_mut(&mut self) -> Option<&mut StoredProfile> {
        let id = self.active_id.clone()?;
        self.profiles.iter_mut().find(|profile| profile.id == id)
    }

    fn profile(&self, id: &str) -> Option<&StoredProfile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// Fresh, stable profile id. `p1`, `p2`, … (collision-safe if a migrated
    /// `default` profile or a removed id left gaps).
    fn next_id(&self) -> String {
        let mut n = self.profiles.len() + 1;
        loop {
            let id = format!("p{n}");
            if self.profile(&id).is_none() {
                return id;
            }
            n += 1;
        }
    }
}

/// Id assigned to the single server migrated from a pre-profiles config.
const LEGACY_PROFILE_ID: &str = "default";

/// `host[:port]` of a server URL, for the default profile label.
fn host_of(url: &str) -> &str {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    rest.split('/').next().unwrap_or(rest)
}

/// Trimmed custom label; an empty label clears it (fall back to `user@host`).
fn normalized_label(label: &str) -> Option<String> {
    let label = label.trim();
    (!label.is_empty()).then(|| label.to_owned())
}

/// Parse `navidrome.json`, tolerating the v1 single-server shape (which is
/// wrapped as one `default` profile and made active).
fn parse_config(text: &str) -> Option<StoredConfig> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("profiles").is_some() {
        let mut config: StoredConfig = serde_json::from_value(value).ok()?;
        config.profiles.retain(|profile| !profile.id.is_empty());
        if config
            .active_id
            .as_deref()
            .is_some_and(|id| config.profile(id).is_none())
        {
            config.active_id = None;
        }
        return Some(config);
    }
    let mut profile: StoredProfile = serde_json::from_value(value).ok()?;
    if profile.url.is_empty() {
        return None;
    }
    profile.id = LEGACY_PROFILE_ID.into();
    Some(StoredConfig {
        active_id: Some(profile.id.clone()),
        profiles: vec![profile],
    })
}

// ---------------------------------------------------------------------------
// state
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Inner {
    /// Saved servers + the active (default) one (empty until a profile is
    /// added). With aggregation, every profile's rows are synced; the active
    /// profile drives playlists, publishing and the status card.
    config: StoredConfig,
    /// Lazily built per-profile clients (connecting pings the server once).
    clients: HashMap<String, Arc<SubsonicClient>>,
}

/// Shared Navidrome state: persisted config + client cache, and the
/// one-sync-at-a-time flag (an `Arc` so the sync thread can clear it).
#[derive(Default)]
pub struct NavidromeState {
    inner: Arc<Mutex<Inner>>,
    syncing: Arc<AtomicBool>,
}

/// Clears the sync flag when the thread finishes, however it finishes.
struct SyncGuard(Arc<AtomicBool>);

impl Drop for SyncGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Called from the app's `setup` hook: loads `navidrome.json` if present.
/// The client is not built here — a slow/unreachable server must not stall
/// app launch; the first command that needs it reconnects. Legacy v1 files
/// are rewritten in the profiles shape, and their single server's playlist
/// mappings are re-keyed from `''` to that profile id.
pub fn init(app: &AppHandle) {
    let Ok(path) = config_file(app) else {
        return; // no app data dir: commands surface real errors later
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Some(config) = parse_config(&text) else {
        eprintln!(
            "navinamp navidrome: ignoring unreadable config `{}`",
            path.display()
        );
        return;
    };
    // Normalize the on-disk shape (v1 → profiles) on first boot.
    if let Err(e) = save_config(app, &config) {
        eprintln!("navinamp navidrome: cannot rewrite config: {e}");
    }
    // Pre-aggregation remote rows used `navidrome://<remote_id>` and no
    // server id. They all belonged to the active (or only) profile; qualify
    // them so multi-server routing and per-server pruning work.
    let legacy_server = config
        .active_id
        .clone()
        .or_else(|| (config.profiles.len() == 1).then(|| config.profiles[0].id.clone()));
    if let Some(server_id) = legacy_server {
        let _ = with_library(app, &app.state::<LibraryState>(), |library| {
            library
                .migrate_remote_server(&server_id)
                .map_err(|e| e.to_string())
        });
    }
    if let Some(profile) = config
        .profiles
        .first()
        .filter(|_| config.profiles.len() == 1)
    {
        let profile_id = profile.id.clone();
        let _ = with_library(app, &app.state::<LibraryState>(), |library| {
            library
                .remap_smart_view_playlists("", &profile_id)
                .map_err(|e| e.to_string())
        });
    }
    app.state::<NavidromeState>().inner.lock().unwrap().config = config;
}

/// `<app_data_dir>/navidrome.json`, with the directory created.
fn config_file(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create `{}`: {e}", dir.display()))?;
    Ok(dir.join("navidrome.json"))
}

fn save_config(app: &AppHandle, config: &StoredConfig) -> Result<(), String> {
    let path = config_file(app)?;
    let text =
        serde_json::to_string_pretty(config).map_err(|e| format!("cannot encode config: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write `{}`: {e}", path.display()))
}

/// Persist the profile list; an empty list removes the file (nothing is
/// configured, same as v1's disconnect).
fn save_or_remove(app: &AppHandle, config: &StoredConfig) -> Result<(), String> {
    if config.profiles.is_empty() {
        if let Ok(path) = config_file(app) {
            let _ = std::fs::remove_file(path);
        }
        Ok(())
    } else {
        save_config(app, config)
    }
}

/// Profile-scope mutations must not race the sync thread (it captures the
/// active client/library at start and would write rows for the wrong server).
fn ensure_not_syncing(state: &NavidromeState) -> Result<(), String> {
    if state.syncing.load(Ordering::SeqCst) {
        Err("a navidrome sync is running; try again when it finishes".into())
    } else {
        Ok(())
    }
}

/// Delete one server's synced rows (profile removal). Other servers and
/// local rows are untouched.
fn delete_remote_rows(app: &AppHandle, library: &LibraryState, server: &str) -> Result<(), String> {
    with_library(app, library, |lib| {
        lib.delete_remote_server(server)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

/// Drop the playlist mappings of a server that is gone (or switched away).
fn clear_publish_mappings(
    app: &AppHandle,
    library: &LibraryState,
    server: &str,
) -> Result<(), String> {
    with_library(app, library, |lib| {
        lib.clear_smart_view_playlists(server)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

// ---------------------------------------------------------------------------
// IPC payload shapes (frozen contract — do not deviate)
// ---------------------------------------------------------------------------

/// Current streaming preferences, as shown by [`navidrome_status`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamingPrefs {
    /// `original` | `mp3` | `opus`.
    pub format: String,
    pub bitrate_kbps: Option<u32>,
}

/// Snapshot returned by every navidrome command.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub configured: bool,
    pub url: Option<String>,
    pub user: Option<String>,
    pub streaming: StreamingPrefs,
    /// Unix seconds of the last completed sync (`null` before the first).
    pub synced_at: Option<i64>,
    pub server_version: Option<String>,
    /// Navidrome rows currently in the library.
    pub track_count: i64,
    /// Selected library (music folder) id, or `null` for all libraries.
    pub library_id: Option<String>,
    /// Active server profile id (stable across restarts).
    pub profile_id: Option<String>,
    /// Active profile's display name (custom label, else `user@host`).
    pub profile_label: Option<String>,
}

/// One saved server, as listed by [`navidrome_profiles`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub id: String,
    /// Effective display name (custom label or `user@host`).
    pub name: String,
    /// User-editable label; `null` means "derive from user@host".
    pub label: Option<String>,
    pub url: String,
    pub user: String,
    pub active: bool,
    pub synced_at: Option<i64>,
}

/// Everything the UI needs to know about one completed sync.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub artists: u64,
    pub albums: u64,
    pub tracks_added: u64,
    pub tracks_updated: u64,
    pub tracks_removed: u64,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

fn parse_format(s: &str) -> Option<StreamFormat> {
    match s {
        "original" => Some(StreamFormat::Original),
        "mp3" => Some(StreamFormat::Mp3),
        "opus" => Some(StreamFormat::Opus),
        _ => None,
    }
}

fn format_string(f: StreamFormat) -> &'static str {
    match f {
        StreamFormat::Original => "original",
        StreamFormat::Mp3 => "mp3",
        StreamFormat::Opus => "opus",
    }
}

/// Status snapshot from state + library row count. Never fails: an
/// unopenable library reads as `trackCount: 0`.
fn current_status(app: &AppHandle, state: &NavidromeState) -> Status {
    let library = app.state::<LibraryState>();
    let guard = state.inner.lock().unwrap();
    let profile = guard.config.active();
    Status {
        configured: profile.is_some(),
        url: profile.map(|p| p.url.clone()),
        user: profile.map(|p| p.user.clone()),
        streaming: StreamingPrefs {
            format: profile
                .map(|p| p.format.clone())
                .unwrap_or_else(|| "original".into()),
            bitrate_kbps: profile.and_then(|p| p.bitrate_kbps),
        },
        synced_at: profile.and_then(|p| p.synced_at),
        server_version: profile.and_then(|p| p.server_version.clone()),
        library_id: profile.and_then(|p| p.library_id.clone()),
        profile_id: profile.map(|p| p.id.clone()),
        profile_label: profile.map(StoredProfile::name),
        // Unconfigured → 0; configured → the active server's live count (an
        // unopenable library reads as 0 too — status must never fail).
        track_count: match profile {
            Some(profile) => with_library(app, &library, |lib| {
                lib.track_count_for_server(&profile.id)
                    .map_err(|e| e.to_string())
            })
            .unwrap_or(0) as i64,
            None => 0,
        },
    }
}

// ---------------------------------------------------------------------------
// client cache
// ---------------------------------------------------------------------------

/// The active profile's client, connecting lazily. Errors are scrubbed of
/// URLs (Subsonic tokens ride in query strings).
async fn ensure_client(state: &NavidromeState) -> Result<Arc<SubsonicClient>, String> {
    let server_id = active_server_id(state)?;
    ensure_client_for(state, &server_id).await
}

/// The shared client for one saved profile, connecting lazily and caching
/// per profile so aggregation can route rows to their own server.
async fn ensure_client_for(
    state: &NavidromeState,
    server_id: &str,
) -> Result<Arc<SubsonicClient>, String> {
    let (url, user, password) = {
        let guard = state.inner.lock().unwrap();
        if let Some(client) = guard.clients.get(server_id) {
            return Ok(Arc::clone(client));
        }
        let profile = guard
            .config
            .profile(server_id)
            .ok_or_else(|| format!("unknown server `{server_id}`"))?;
        (
            profile.url.clone(),
            profile.user.clone(),
            profile.password.clone(),
        )
    };

    let client = Arc::new(
        SubsonicClient::connect(&url, &user, &password)
            .await
            .map_err(|e| scrub_urls(&e.to_string()))?,
    );

    let mut guard = state.inner.lock().unwrap();
    if let Some(existing) = guard.clients.get(server_id) {
        return Ok(Arc::clone(existing)); // a concurrent caller won
    }
    let Some(profile) = guard
        .config
        .profiles
        .iter_mut()
        .find(|profile| profile.id == server_id)
    else {
        return Err(format!("server `{server_id}` is no longer configured"));
    };
    if profile.server_version.is_none() {
        profile.server_version = client.server_version().map(str::to_owned);
    }
    guard
        .clients
        .insert(server_id.to_owned(), Arc::clone(&client));
    Ok(client)
}

/// Id of the active/default profile, if one is selected.
fn active_server_id(state: &NavidromeState) -> Result<String, String> {
    state
        .inner
        .lock()
        .unwrap()
        .config
        .active_id
        .clone()
        .ok_or_else(|| "not connected".to_string())
}

/// Resolve an optional command server id: a known profile, else the active
/// one (keeps legacy callers/queue entries working).
fn resolve_server(state: &NavidromeState, requested: Option<String>) -> Result<String, String> {
    let requested = requested
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let guard = state.inner.lock().unwrap();
    match requested {
        Some(id) => {
            guard
                .config
                .profile(&id)
                .ok_or_else(|| format!("unknown server `{id}`"))?;
            Ok(id)
        }
        None => guard
            .config
            .active_id
            .clone()
            .ok_or_else(|| "not connected".to_string()),
    }
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

/// Connection + sync snapshot (polled by the UI).
#[tauri::command]
pub fn navidrome_status(app: AppHandle, state: State<'_, NavidromeState>) -> Status {
    current_status(&app, &state)
}

/// `(display name, profile id)` pairs for prompt/context builders (AI search).
pub(crate) fn profile_names(state: &NavidromeState) -> Vec<(String, String)> {
    let guard = state.inner.lock().unwrap();
    guard
        .config
        .profiles
        .iter()
        .map(|profile| (profile.name(), profile.id.clone()))
        .collect()
}

/// Saved servers (in creation order) with the active one flagged.
#[tauri::command]
pub fn navidrome_profiles(state: State<'_, NavidromeState>) -> Vec<ProfileSummary> {
    let guard = state.inner.lock().unwrap();
    let active = guard.config.active_id.as_deref();
    guard
        .config
        .profiles
        .iter()
        .map(|profile| ProfileSummary {
            id: profile.id.clone(),
            name: profile.name(),
            label: profile.label.clone(),
            url: profile.url.clone(),
            user: profile.user.clone(),
            active: active == Some(profile.id.as_str()),
            synced_at: profile.synced_at,
        })
        .collect()
}

/// Ping + save credentials on success. Connecting to a known `url` + `user`
/// updates that profile (password, version, optional label); a new pair
/// appends a profile. The profile becomes the active/default one; other
/// servers' synced rows stay untouched.
#[tauri::command]
pub async fn navidrome_connect(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    url: String,
    user: String,
    password: String,
    label: Option<String>,
) -> Result<Status, String> {
    let url = url.trim().trim_end_matches('/').to_owned();
    let user = user.trim().to_owned();
    if url.is_empty() || user.is_empty() {
        return Err("url and user are required".into());
    }
    ensure_not_syncing(&state)?;

    // Ping first: nothing is persisted for a server that rejects us.
    let client = SubsonicClient::connect(&url, &user, &password)
        .await
        .map_err(|e| scrub_urls(&e.to_string()))?;
    // A sync may have started while the ping ran; re-check before mutating.
    ensure_not_syncing(&state)?;

    let server_version = client.server_version().map(str::to_owned);
    let (config, profile_id) = {
        let guard = state.inner.lock().unwrap();
        let mut config = guard.config.clone();
        let profile_id = match config
            .profiles
            .iter_mut()
            .find(|p| p.url == url && p.user == user)
        {
            Some(profile) => {
                profile.password = password;
                if let Some(label) = label.as_deref() {
                    profile.label = normalized_label(label);
                }
                if server_version.is_some() {
                    profile.server_version = server_version.clone();
                }
                profile.id.clone()
            }
            None => {
                let profile = StoredProfile {
                    id: config.next_id(),
                    label: label.as_deref().and_then(normalized_label),
                    url: url.clone(),
                    user: user.clone(),
                    password,
                    format: "original".into(),
                    bitrate_kbps: None,
                    synced_at: None,
                    server_version: server_version.clone(),
                    library_id: None,
                };
                let id = profile.id.clone();
                config.profiles.push(profile);
                id
            }
        };
        config.active_id = Some(profile_id.clone());
        (config, profile_id)
    };

    save_config(&app, &config)?;
    let mut guard = state.inner.lock().unwrap();
    guard.config = config;
    // Replace only this profile's cached client; other servers keep theirs.
    guard.clients.insert(profile_id, Arc::new(client));
    drop(guard);
    Ok(current_status(&app, &state))
}

/// Forgets the active profile (credentials + its synced rows). Other saved
/// servers stay in place; the first remaining one becomes the default.
/// Playlist mappings are per server, so the removed profile's are dropped.
#[tauri::command]
pub async fn navidrome_disconnect(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    library: State<'_, LibraryState>,
) -> Result<Status, String> {
    let active = state.inner.lock().unwrap().config.active_id.clone();
    match active {
        Some(id) => navidrome_remove_profile(app, state, library, id).await,
        None => Ok(current_status(&app, &state)),
    }
}

/// Make a saved server the active/default one. With aggregation every
/// server's rows stay visible; active only drives playlists, publishing and
/// the status card, so nothing is cleared or re-synced here.
#[tauri::command]
pub async fn navidrome_switch(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    id: String,
) -> Result<Status, String> {
    ensure_not_syncing(&state)?;
    let config = {
        let guard = state.inner.lock().unwrap();
        let mut config = guard.config.clone();
        if config.profile(&id).is_none() {
            return Err(format!("unknown server profile `{id}`"));
        }
        config.active_id = Some(id);
        config
    };
    save_config(&app, &config)?;
    state.inner.lock().unwrap().config = config;
    Ok(current_status(&app, &state))
}

/// Remove one saved server: its synced rows, credentials and playlist
/// mappings. Other servers' rows stay visible.
#[tauri::command]
pub async fn navidrome_remove_profile(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    library: State<'_, LibraryState>,
    id: String,
) -> Result<Status, String> {
    // A running sync may still be writing this server's rows.
    ensure_not_syncing(&state)?;
    let config = {
        let guard = state.inner.lock().unwrap();
        if guard.config.profile(&id).is_none() {
            drop(guard);
            return Ok(current_status(&app, &state));
        }
        let was_active = guard.config.active_id.as_deref() == Some(id.as_str());
        let mut config = guard.config.clone();
        config.profiles.retain(|profile| profile.id != id);
        if was_active {
            // Keep a usable default: the next saved server, or none.
            config.active_id = config.profiles.first().map(|profile| profile.id.clone());
        }
        config
    };
    delete_remote_rows(&app, &library, &id)?;
    clear_publish_mappings(&app, &library, &id)?;
    save_or_remove(&app, &config)?;
    let mut guard = state.inner.lock().unwrap();
    guard.clients.remove(&id);
    guard.config = config;
    drop(guard);
    Ok(current_status(&app, &state))
}

/// Rename a saved server (`null`/blank label = fall back to `user@host`).
#[tauri::command]
pub fn navidrome_rename_profile(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    id: String,
    label: Option<String>,
) -> Result<Status, String> {
    let config = {
        let guard = state.inner.lock().unwrap();
        let mut config = guard.config.clone();
        let profile = config
            .profiles
            .iter_mut()
            .find(|profile| profile.id == id)
            .ok_or_else(|| format!("unknown server profile `{id}`"))?;
        profile.label = label.as_deref().and_then(normalized_label);
        config
    };
    save_config(&app, &config)?;
    state.inner.lock().unwrap().config = config;
    Ok(current_status(&app, &state))
}

/// The stream URL for a remote track id, honoring the streaming prefs.
#[tauri::command]
pub async fn navidrome_stream_url(
    state: State<'_, NavidromeState>,
    server_id: Option<String>,
    remote_id: String,
) -> Result<String, String> {
    let server = resolve_server(&state, server_id)?;
    let client = ensure_client_for(&state, &server).await?;
    let (format, bitrate) = {
        let guard = state.inner.lock().unwrap();
        let profile = guard.config.profile(&server).ok_or("not connected")?;
        (
            profile.stream_format().unwrap_or(StreamFormat::Original),
            profile.bitrate_kbps,
        )
    };
    Ok(client.stream_url(&remote_id, format, bitrate).to_string())
}

/// The cover-art URL for a remote art id (a song or album `coverArt` id —
/// exactly what the sync stores in `tracks.art_id` for navidrome rows).
/// Mirrors [`navidrome_stream_url`]: a pure builder over the connected
/// client; `size` optionally requests a server-side square scale in
/// pixels (thumbnails pass a small one to keep payloads tiny). The URL
/// embeds the auth token, so it is returned, never logged.
#[tauri::command]
pub async fn navidrome_art_url(
    state: State<'_, NavidromeState>,
    server_id: Option<String>,
    remote_id: String,
    size: Option<u32>,
) -> Result<String, String> {
    let server = resolve_server(&state, server_id)?;
    let client = ensure_client_for(&state, &server).await?;
    Ok(client.cover_art_url(&remote_id, size).to_string())
}

/// Scrobble a track: `submission=false` → "now playing", `true` → played.
#[tauri::command]
pub async fn navidrome_scrobble(
    state: State<'_, NavidromeState>,
    server_id: Option<String>,
    remote_id: String,
    submission: bool,
) -> Result<(), String> {
    let server = resolve_server(&state, server_id)?;
    let client = ensure_client_for(&state, &server).await?;
    client
        .scrobble(&remote_id, submission, None)
        .await
        .map_err(|e| scrub_urls(&e.to_string()))
}

/// Star or unstar a track on the server, then mirror the flag into the
/// local row so it survives restarts and shows in every view.
#[tauri::command]
pub async fn navidrome_star(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    server_id: Option<String>,
    remote_id: String,
    starred: bool,
) -> Result<(), String> {
    let server = resolve_server(&state, server_id)?;
    let client = ensure_client_for(&state, &server).await?;
    let result = if starred {
        client.star(&remote_id).await
    } else {
        client.unstar(&remote_id).await
    };
    result.map_err(|e| scrub_urls(&e.to_string()))?;

    // The server already changed; a local mirror failure must not turn the
    // action into an error (the UI would revert a successful star).
    if let Ok(path) = db_file(&app) {
        match Library::open(path) {
            Ok(library) => {
                if let Err(e) = library.set_starred(&server, &remote_id, starred) {
                    eprintln!("navinamp navidrome: cannot mirror star locally: {e}");
                }
            }
            Err(e) => eprintln!("navinamp navidrome: cannot open library to mirror star: {e}"),
        }
    }
    Ok(())
}

/// Set the user rating (0 = clear, 1–5 stars) for a track on the server.
#[tauri::command]
pub async fn navidrome_rate(
    state: State<'_, NavidromeState>,
    server_id: Option<String>,
    remote_id: String,
    rating: u8,
) -> Result<(), String> {
    if rating > 5 {
        return Err(format!("rating {rating} out of range (0..=5)"));
    }
    let server = resolve_server(&state, server_id)?;
    let client = ensure_client_for(&state, &server).await?;
    client
        .rate(&remote_id, rating)
        .await
        .map_err(|e| scrub_urls(&e.to_string()))
}

/// Libraries (music folders) accessible to the connected user.
#[tauri::command]
pub async fn navidrome_libraries(
    state: State<'_, NavidromeState>,
) -> Result<Vec<MusicFolder>, String> {
    let client = ensure_client(&state).await?;
    client
        .get_music_folders()
        .await
        .map_err(|e| scrub_urls(&e.to_string()))
}

/// Select one library for sync; `null` means all accessible libraries.
/// Validates the id against `getMusicFolders` before persisting it.
#[tauri::command]
pub async fn navidrome_set_library(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    library_id: Option<String>,
) -> Result<Status, String> {
    let client = ensure_client(&state).await?;
    ensure_not_syncing(&state)?;
    let library_id = match library_id
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        Some(id) => {
            let folders = client
                .get_music_folders()
                .await
                .map_err(|e| scrub_urls(&e.to_string()))?;
            if !folders.iter().any(|folder| folder.id == id) {
                return Err(format!("unknown library `{id}`"));
            }
            Some(id)
        }
        None => None,
    };
    let config = {
        let guard = state.inner.lock().unwrap();
        let mut config = guard.config.clone();
        let profile = config.active_mut().ok_or("not connected")?;
        profile.library_id = library_id;
        config
    };
    save_config(&app, &config)?;
    state.inner.lock().unwrap().config = config;
    Ok(current_status(&app, &state))
}

/// All playlists visible to the connected user (server order).
#[tauri::command]
pub async fn navidrome_playlists(
    state: State<'_, NavidromeState>,
    server_id: Option<String>,
) -> Result<Vec<PlaylistSummary>, String> {
    let server = resolve_server(&state, server_id)?;
    let client = ensure_client_for(&state, &server).await?;
    client
        .get_playlists()
        .await
        .map_err(|e| scrub_urls(&e.to_string()))
}

/// One server playlist's tracks as ephemeral library rows — nothing is
/// written to the local DB, so the playlist view always reflects the server.
/// Negative ids keep these rows distinct from stored ids (both are positive),
/// and `remote_id` makes them playable through the normal remote path.
#[tauri::command]
pub async fn navidrome_playlist_tracks(
    state: State<'_, NavidromeState>,
    server_id: Option<String>,
    playlist_id: String,
) -> Result<Vec<Track>, String> {
    let server = resolve_server(&state, server_id)?;
    let client = ensure_client_for(&state, &server).await?;
    let songs = client
        .get_playlist(&playlist_id)
        .await
        .map_err(|e| scrub_urls(&e.to_string()))?;
    Ok(playlist_rows(&songs, &server))
}

/// Map server playlist entries onto [`Track`] rows for the library table.
fn playlist_rows(songs: &[SongSummary], server_id: &str) -> Vec<Track> {
    songs
        .iter()
        .enumerate()
        .map(|(i, song)| {
            let title = repair_tag_text(&song.title).into_owned();
            let artist = repair_tag_text(&song.artist).into_owned();
            let album = repair_tag_text(&song.album).into_owned();
            let genre = song
                .genre
                .as_deref()
                .map(|genre| repair_tag_text(genre).into_owned())
                .unwrap_or_default();
            Track {
                id: -((i as i64) + 1),
                path: remote_path(server_id, &song.id),
                mtime: 0,
                size: song
                    .size
                    .map(|s| i64::try_from(s).unwrap_or(i64::MAX))
                    .unwrap_or(0),
                title,
                artist: artist.clone(),
                album_artist: artist,
                album,
                album_key: String::new(),
                genre,
                year: (song.year > 0).then_some(i64::from(song.year)),
                track_no: (song.track_no > 0).then_some(i64::from(song.track_no)),
                disc_no: (song.disc_no > 0).then_some(i64::from(song.disc_no)),
                duration_secs: song.duration_secs as f64,
                bitrate_kbps: (song.bitrate_kbps > 0).then_some(i64::from(song.bitrate_kbps)),
                samplerate: song.sampling_rate.map(i64::from),
                channels: song.channel_count.map(i64::from),
                codec: song.suffix.clone(),
                rating: i64::from(song.user_rating.min(5)),
                starred: song.starred,
                play_count: i64::try_from(song.play_count).unwrap_or(i64::MAX),
                last_played_at: None,
                date_added: 0,
                source: SOURCE.into(),
                remote_id: Some(song.id.clone()),
                server_id: Some(server_id.to_owned()),
                remote_path: (!song.path.is_empty()).then(|| song.path.clone()),
                art_id: song.cover_art_id.clone(),
            }
        })
        .collect()
}

/// Prefix on playlists this app creates: visible provenance on the server and
/// no accidental collision with a user playlist that shares the name.
const SMART_PLAYLIST_PREFIX: &str = "[NaVinaMP] ";

/// Result of publishing one smart view as a Navidrome playlist.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishReport {
    pub playlist_id: String,
    pub name: String,
    pub track_count: u64,
    /// True when the server playlist was created; false when the remembered
    /// one was updated in place.
    pub created: bool,
}

/// Publish a smart view as a Navidrome playlist: create on the first publish,
/// update the mapped playlist in place afterwards (its id and play history
/// survive). Only `source=navidrome` rows are publishable — their `remote_id`
/// becomes the server's `songId`. The view's own limit/sort apply.
#[tauri::command]
pub async fn navidrome_publish_smart_view(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    id: i64,
    server_id: Option<String>,
) -> Result<PublishReport, String> {
    let server = resolve_server(&state, server_id)?;
    let client = ensure_client_for(&state, &server).await?;
    let library = Library::open(db_file(&app)?).map_err(|e| format!("cannot open library: {e}"))?;
    let view = library
        .smart_views_list()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|v| v.id == id)
        .ok_or_else(|| format!("smart view {id} not found"))?;
    let tracks = library.smart_view_run(id).map_err(|e| e.to_string())?;
    // Publish only the active server's rows: the server playlist can't carry
    // song ids from another server.
    let song_ids: Vec<String> = tracks
        .iter()
        .filter(|t| t.source == SOURCE)
        .filter(|t| t.server_id.as_deref() == Some(server.as_str()))
        .filter_map(|t| t.remote_id.clone())
        .collect();
    if song_ids.is_empty() {
        return Err("this smart view has no Navidrome tracks to publish".into());
    }

    let name = format!("{SMART_PLAYLIST_PREFIX}{}", view.def.name);
    let mapped = library
        .smart_view_playlist_id(&server, id)
        .map_err(|e| e.to_string())?;
    let (playlist_id, created) = match mapped {
        Some(pid) => {
            let exists = client
                .get_playlists()
                .await
                .map_err(|e| scrub_urls(&e.to_string()))?
                .iter()
                .any(|p| p.id == pid);
            if exists {
                client
                    .update_playlist(&pid, &song_ids)
                    .await
                    .map_err(|e| scrub_urls(&e.to_string()))?;
                (pid, false)
            } else {
                // Deleted on the server since the last publish: recreate it.
                let created = client
                    .create_playlist(&name, &song_ids)
                    .await
                    .map_err(|e| scrub_urls(&e.to_string()))?;
                (created.id, true)
            }
        }
        None => {
            let created = client
                .create_playlist(&name, &song_ids)
                .await
                .map_err(|e| scrub_urls(&e.to_string()))?;
            (created.id, true)
        }
    };
    library
        .set_smart_view_playlist_id(&server, id, &playlist_id)
        .map_err(|e| e.to_string())?;

    Ok(PublishReport {
        playlist_id,
        name,
        track_count: song_ids.len() as u64,
        created,
    })
}

/// Publish an explicit set of Subsonic song ids as a Navidrome playlist.
///
/// Unlike [`navidrome_publish_smart_view`], this is not tied to a saved smart
/// view: the playlist is found by its `[NaVinaMP] ` name (updated in place) or
/// created. Advanced/AI search results use it for “save to server”.
#[tauri::command]
pub async fn navidrome_publish_tracks(
    state: State<'_, NavidromeState>,
    name: String,
    song_ids: Vec<String>,
    server_id: Option<String>,
) -> Result<PublishReport, String> {
    let server = resolve_server(&state, server_id)?;
    let client = ensure_client_for(&state, &server).await?;
    let clean = name.trim();
    if clean.is_empty() {
        return Err("playlist name is required".into());
    }
    if song_ids.is_empty() {
        return Err("no Navidrome tracks to publish".into());
    }
    let playlist_name = format!("{SMART_PLAYLIST_PREFIX}{clean}");
    let existing = client
        .get_playlists()
        .await
        .map_err(|e| scrub_urls(&e.to_string()))?
        .into_iter()
        .find(|p| p.name == playlist_name);
    let (playlist_id, created) = match existing {
        Some(playlist) => {
            client
                .update_playlist(&playlist.id, &song_ids)
                .await
                .map_err(|e| scrub_urls(&e.to_string()))?;
            (playlist.id, false)
        }
        None => {
            let playlist = client
                .create_playlist(&playlist_name, &song_ids)
                .await
                .map_err(|e| scrub_urls(&e.to_string()))?;
            (playlist.id, true)
        }
    };
    Ok(PublishReport {
        playlist_id,
        name: playlist_name,
        track_count: song_ids.len() as u64,
        created,
    })
}

/// Set the streaming format/bitrate used by [`navidrome_stream_url`].
/// `original` forces the bitrate off (the file is served as stored).
#[tauri::command]
pub fn navidrome_set_streaming(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    format: String,
    bitrate_kbps: Option<u32>,
) -> Result<Status, String> {
    let parsed = parse_format(&format)
        .ok_or_else(|| format!("unknown format `{format}` (expected original|mp3|opus)"))?;
    let bitrate = match bitrate_kbps {
        None => None,
        Some(0) => None, // spec: maxBitRate 0 = unlimited
        Some(b) if (16..=3200).contains(&b) => Some(b),
        Some(b) => return Err(format!("bitrate {b} kbps out of range (16..=3200)")),
    };
    let bitrate = (parsed != StreamFormat::Original)
        .then_some(bitrate)
        .flatten();

    let config = {
        let guard = state.inner.lock().unwrap();
        let mut config = guard.config.clone();
        let profile = config.active_mut().ok_or("not connected")?;
        profile.format = format_string(parsed).into();
        profile.bitrate_kbps = bitrate;
        config
    };
    save_config(&app, &config)?;
    state.inner.lock().unwrap().config = config;
    Ok(current_status(&app, &state))
}

/// One server captured for a sync run: its client and per-profile settings.
struct SyncTarget {
    server_id: String,
    label: String,
    library_id: Option<String>,
    client: Arc<SubsonicClient>,
}

/// Borrowed inputs shared by the album-index and artist-fallback walks.
#[derive(Clone, Copy)]
struct SyncJob<'a> {
    app: &'a AppHandle,
    client: &'a SubsonicClient,
    library: &'a Library,
    library_id: Option<&'a str>,
    server_id: &'a str,
    label: &'a str,
    force_full: bool,
}

/// Library sync for one or all saved servers. A quick sync (default) pages
/// the cheap album index and only calls `getAlbum` for new/changed albums;
/// `full = true` re-fetches every album. Albums are fetched concurrently.
/// With no `server_id` every saved profile is walked sequentially; with one,
/// just that profile. Runs on a dedicated thread; one sync at a time (a
/// second invoke fails with an error string). Progress in
/// `navidrome-progress` events (track units, tagged with the server), final
/// `navidrome-synced`.
#[tauri::command]
pub async fn navidrome_sync(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    server_id: Option<String>,
    full: Option<bool>,
) -> Result<SyncReport, String> {
    let force_full = full.unwrap_or(false);
    if state.syncing.swap(true, Ordering::SeqCst) {
        return Err("a navidrome sync is already running".into());
    }
    let prepared = prepare_sync_targets(&state, server_id).await;
    let (targets, initial_errors) = match prepared {
        Ok(value) => value,
        Err(e) => {
            state.syncing.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };
    if targets.is_empty() {
        state.syncing.store(false, Ordering::SeqCst);
        return Err(if initial_errors.is_empty() {
            "no servers to sync".into()
        } else {
            initial_errors.join("; ")
        });
    }
    let library = match db_file(&app)
        .and_then(|path| Library::open(path).map_err(|e| format!("cannot open library: {e}")))
    {
        Ok(library) => library,
        Err(e) => {
            state.syncing.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };

    let started = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel::<Result<SyncReport, String>>();
    let emitter = app.clone();
    let flag = Arc::clone(&state.syncing);
    let inner = Arc::clone(&state.inner);

    std::thread::Builder::new()
        .name("navinamp-navidrome-sync".into())
        .spawn(move || {
            let _guard = SyncGuard(flag);
            let mut aggregate = SyncReport::default();
            let mut errors = initial_errors;
            let mut successes = 0u32;
            for target in targets {
                match run_sync(
                    &emitter,
                    &target.client,
                    &library,
                    target.library_id.as_deref(),
                    &target.server_id,
                    &target.label,
                    force_full,
                ) {
                    Ok(report) => {
                        successes += 1;
                        aggregate.artists += report.artists;
                        aggregate.albums += report.albums;
                        aggregate.tracks_added += report.tracks_added;
                        aggregate.tracks_updated += report.tracks_updated;
                        aggregate.tracks_removed += report.tracks_removed;
                        errors.extend(
                            report
                                .errors
                                .into_iter()
                                .map(|e| format!("[{}] {e}", target.label)),
                        );
                        mark_synced(&emitter, &inner, &target.server_id);
                    }
                    Err(e) => errors.push(format!("[{}] {e}", target.label)),
                }
            }
            aggregate.errors = errors;
            aggregate.duration_ms = started.elapsed().as_millis() as u64;

            let result = if successes == 0 {
                Err(aggregate.errors.join("; "))
            } else {
                Ok(aggregate)
            };
            match &result {
                Ok(report) => {
                    let _ = emitter.emit(SYNCED_EVENT, report);
                }
                Err(e) => {
                    let _ = emitter.emit(SYNCED_EVENT, serde_json::json!({ "error": e }));
                }
            }
            let _ = tx.send(result);
        })
        .map_err(|e| {
            state.syncing.store(false, Ordering::SeqCst);
            format!("cannot spawn sync thread: {e}")
        })?;

    let report = tauri::async_runtime::spawn_blocking(move || {
        rx.recv().map_err(|_| "sync thread died".to_string())?
    })
    .await
    .map_err(|e| format!("sync join failed: {e}"))??;
    Ok(report)
}

/// Resolve the sync target list. A requested id must exist and its client
/// must connect (single-server failure is fatal). A sync-all skips servers
/// that cannot connect, reporting them in the returned errors.
async fn prepare_sync_targets(
    state: &NavidromeState,
    requested: Option<String>,
) -> Result<(Vec<SyncTarget>, Vec<String>), String> {
    let requested = requested
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let profiles: Vec<(String, String, Option<String>)> = {
        let guard = state.inner.lock().unwrap();
        match requested.as_deref() {
            Some(id) => {
                let profile = guard
                    .config
                    .profile(id)
                    .ok_or_else(|| format!("unknown server `{id}`"))?;
                vec![(
                    profile.id.clone(),
                    profile.name(),
                    profile.library_id.clone(),
                )]
            }
            None => guard
                .config
                .profiles
                .iter()
                .map(|p| (p.id.clone(), p.name(), p.library_id.clone()))
                .collect(),
        }
    };
    if profiles.is_empty() {
        return Err("not connected".into());
    }

    let mut targets = Vec::new();
    let mut errors = Vec::new();
    for (server_id, label, library_id) in profiles {
        match ensure_client_for(state, &server_id).await {
            Ok(client) => targets.push(SyncTarget {
                server_id,
                label,
                library_id,
                client,
            }),
            Err(e) if requested.is_none() => errors.push(format!("{label}: {e}")),
            Err(e) => return Err(format!("{label}: {e}")),
        }
    }
    Ok((targets, errors))
}

fn mark_synced(app: &AppHandle, inner: &Arc<Mutex<Inner>>, server_id: &str) {
    let mut guard = inner.lock().unwrap();
    {
        let Some(profile) = guard
            .config
            .profiles
            .iter_mut()
            .find(|profile| profile.id == server_id)
        else {
            return;
        };
        profile.synced_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        );
    }
    let _ = save_config(app, &guard.config);
}

// ---------------------------------------------------------------------------
// the walk
// ---------------------------------------------------------------------------

/// The sync walk for one server (runs on its own thread). A quick rescan
/// compares the cheap album index against stored fingerprints and only
/// fetches changed albums; a full rescan fetches every album. Either way the
/// album walk is concurrent and prunes only this server after a clean pass.
fn run_sync(
    app: &AppHandle,
    client: &SubsonicClient,
    library: &Library,
    library_id: Option<&str>,
    server_id: &str,
    label: &str,
    force_full: bool,
) -> Result<SyncReport, String> {
    let started = Instant::now();
    tauri::async_runtime::block_on(async {
        let mut report = SyncReport::default();
        // Immediate feedback while the album index is paged.
        emit_progress(app, 0, 0, server_id, label);
        let job = SyncJob {
            app,
            client,
            library,
            library_id,
            server_id,
            label,
            force_full,
        };

        match fetch_album_index(client, library_id).await {
            Some(albums) => {
                sync_album_index(job, albums, &mut report).await?;
            }
            // Servers without a usable getAlbumList2 fall back to the old
            // artist walk (full sync, no fingerprints).
            None => {
                sync_by_artist(job, &mut report).await?;
            }
        }

        report.duration_ms = started.elapsed().as_millis() as u64;
        Ok(report)
    })
}

/// Page the whole album index for one library, deduped by album id. `None`
/// means the endpoint failed or ignored `offset`; the caller falls back to
/// the artist walk.
async fn fetch_album_index(
    client: &SubsonicClient,
    library_id: Option<&str>,
) -> Option<Vec<AlbumSummary>> {
    let mut albums = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut offset = 0u32;
    loop {
        let page = match client
            .get_album_list2(offset, TRACK_COUNT_PAGE, library_id)
            .await
        {
            Ok(page) => page,
            Err(e) => {
                eprintln!(
                    "navinamp navidrome: album index failed: {}",
                    scrub_urls(&e.to_string())
                );
                return None;
            }
        };
        if page.is_empty() {
            return Some(albums);
        }
        let fetched = page.len() as u32;
        let mut fresh = 0usize;
        for album in page {
            if seen.insert(album.id.clone()) {
                albums.push(album);
                fresh += 1;
            }
        }
        // A conforming server may return fewer than `size` albums on any
        // page, so only an empty page ends the list. `fresh == 0` is the
        // defensive stop for a server that ignored `offset` entirely.
        if fresh == 0 {
            return None;
        }
        offset = offset.saturating_add(fetched);
    }
}

/// Sync from the album index. Quick rescans compare each album's cheap
/// fingerprint against the stored one and only fetch changed albums; a full
/// rescan fetches everything. Fetches run concurrently (bounded).
async fn sync_album_index(
    job: SyncJob<'_>,
    albums: Vec<AlbumSummary>,
    report: &mut SyncReport,
) -> Result<(), String> {
    let SyncJob {
        app,
        client,
        library,
        library_id,
        server_id,
        label,
        force_full,
    } = job;
    let total_albums = albums.len() as u64;
    let total_tracks: u64 = albums.iter().map(|a| u64::from(a.song_count)).sum();
    report.artists = client
        .get_artists(library_id)
        .await
        .map(|artists| artists.len() as u64)
        .unwrap_or(0);
    emit_progress(app, 0, total_tracks, server_id, label);

    // Fingerprints + existing track ids are only useful for quick rescans.
    let mut stored_albums = if force_full {
        HashMap::new()
    } else {
        library
            .remote_albums(server_id)
            .map_err(|e| format!("cannot read album fingerprints: {e}"))?
    };
    let mut known_tracks = if force_full {
        HashMap::new()
    } else {
        library
            .remote_track_ids_by_album(server_id)
            .map_err(|e| format!("cannot read remote track ids: {e}"))?
    };
    // A track without an album id (legacy/partial sync) cannot be attributed
    // to a skipped album and would be pruned. Force a full walk to repair it.
    if !force_full {
        let known_total: usize = known_tracks.values().map(Vec::len).sum();
        let all_total = library
            .track_count_for_server(server_id)
            .map_err(|e| format!("cannot count remote tracks: {e}"))?;
        if known_total as u64 != all_total {
            stored_albums.clear();
            known_tracks.clear();
        }
    }

    let mut tracks: Vec<RemoteTrack> = Vec::new();
    let mut album_updates: Vec<RemoteAlbum> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut seen_albums: HashSet<String> = HashSet::new();
    let mut scanned_albums: u64 = 0;
    let mut scanned_tracks: u64 = 0;

    let mut stream = futures::stream::iter(albums.into_iter().map(|album| {
        let known = known_tracks.get(&album.id).cloned().unwrap_or_default();
        let skip = !force_full && album_matches(stored_albums.get(&album.id), &album, &known);
        async move {
            if skip {
                return (album, known, None);
            }
            let result = client.get_album(&album.id).await;
            (album, known, Some(result))
        }
    }))
    .buffer_unordered(SYNC_CONCURRENCY);

    while let Some((album, known, fetched)) = stream.next().await {
        seen_albums.insert(album.id.clone());
        scanned_albums += 1;
        match fetched {
            // Fingerprint matched: retain the album's existing rows without
            // spending a getAlbum request.
            None => {
                seen.extend(known);
                scanned_tracks += u64::from(album.song_count);
            }
            Some(Ok(detail)) => {
                for song in &detail.songs {
                    if !song.id.is_empty() {
                        seen.insert(song.id.clone());
                    }
                    tracks.push(map_song(&detail, song, server_id));
                }
                scanned_tracks += if total_tracks > 0 {
                    u64::from(album.song_count)
                } else {
                    detail.songs.len() as u64
                };
                album_updates.push(remote_album_from_summary(server_id, &album));
            }
            Some(Err(e)) => {
                report.errors.push(format!(
                    "album `{}`: {}",
                    album.name,
                    scrub_urls(&e.to_string())
                ));
                scanned_tracks += u64::from(album.song_count);
            }
        }

        if scanned_albums.is_multiple_of(PROGRESS_EVERY)
            || (total_albums > 0 && scanned_albums >= total_albums)
        {
            // Persist decoded rows before announcing progress: the library
            // pane refreshes on these events.
            flush_remote_batch(library, &mut tracks, &mut album_updates, report)?;
            let shown = if total_tracks == 0 {
                scanned_tracks
            } else {
                scanned_tracks.min(total_tracks)
            };
            emit_progress(app, shown, total_tracks, server_id, label);
        } else if tracks.len() >= SYNC_BATCH_TRACKS {
            flush_remote_batch(library, &mut tracks, &mut album_updates, report)?;
        }
    }

    flush_remote_batch(library, &mut tracks, &mut album_updates, report)?;
    let shown = if total_tracks == 0 {
        scanned_tracks
    } else {
        scanned_tracks.min(total_tracks)
    };
    emit_progress(app, shown, total_tracks, server_id, label);
    report.albums = total_albums;

    // A partial walk (any failed album) must not prune: the failed albums'
    // rows and fingerprints are still valid.
    if report.errors.is_empty() {
        report.tracks_removed = library
            .prune_remote(server_id, &seen)
            .map_err(|e| format!("library sync prune failed: {e}"))?;
        library
            .prune_remote_albums(server_id, &seen_albums)
            .map_err(|e| format!("album fingerprint prune failed: {e}"))?;
    }
    Ok(())
}

/// Fallback walk for servers without `getAlbumList2`: artists → albums →
/// songs, sequential as before. Progress is in tracks with no denominator
/// (the index that would provide one failed).
async fn sync_by_artist(job: SyncJob<'_>, report: &mut SyncReport) -> Result<(), String> {
    let SyncJob {
        app,
        client,
        library,
        library_id,
        server_id,
        label,
        force_full: _,
    } = job;
    let artists = client
        .get_artists(library_id)
        .await
        .map_err(|e| scrub_urls(&e.to_string()))?;
    report.artists = artists.len() as u64;
    let total_albums: u64 = artists.iter().map(|a| u64::from(a.album_count)).sum();

    let mut tracks: Vec<RemoteTrack> = Vec::new();
    let mut album_updates: Vec<RemoteAlbum> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut visited_albums: HashSet<String> = HashSet::new();
    let mut scanned_albums: u64 = 0;
    let mut scanned_tracks: u64 = 0;

    for artist in &artists {
        let albums = match client.get_artist(&artist.id).await {
            Ok(albums) => albums,
            Err(e) => {
                report.errors.push(format!(
                    "artist `{}`: {}",
                    artist.name,
                    scrub_urls(&e.to_string())
                ));
                scanned_albums += u64::from(artist.album_count);
                continue;
            }
        };
        for album in albums {
            if !visited_albums.insert(album.id.clone()) {
                continue;
            }
            report.albums += 1;
            match client.get_album(&album.id).await {
                Ok(detail) => {
                    for song in &detail.songs {
                        if !song.id.is_empty() {
                            seen.insert(song.id.clone());
                        }
                        tracks.push(map_song(&detail, song, server_id));
                    }
                    scanned_tracks += detail.songs.len() as u64;
                }
                Err(e) => report.errors.push(format!(
                    "album `{}`: {}",
                    album.name,
                    scrub_urls(&e.to_string())
                )),
            }
            scanned_albums += 1;
            if scanned_albums.is_multiple_of(PROGRESS_EVERY)
                || (total_albums > 0 && scanned_albums >= total_albums)
            {
                flush_remote_batch(library, &mut tracks, &mut album_updates, report)?;
                emit_progress(app, scanned_tracks, 0, server_id, label);
            } else if tracks.len() >= SYNC_BATCH_TRACKS {
                flush_remote_batch(library, &mut tracks, &mut album_updates, report)?;
            }
        }
    }

    flush_remote_batch(library, &mut tracks, &mut album_updates, report)?;
    emit_progress(app, scanned_tracks, 0, server_id, label);
    if report.errors.is_empty() {
        report.tracks_removed = library
            .prune_remote(server_id, &seen)
            .map_err(|e| format!("library sync prune failed: {e}"))?;
    }
    Ok(())
}

/// Whether a quick rescan can keep an album without a `getAlbum` request.
/// The fingerprint uses only the cheap album-index fields; the track-count
/// check also catches a previous partial/legacy sync.
fn album_matches(stored: Option<&RemoteAlbum>, album: &AlbumSummary, known: &[String]) -> bool {
    let Some(stored) = stored else {
        return false;
    };
    let year = (album.year > 0).then_some(i64::from(album.year));
    stored.song_count == album.song_count
        && stored.duration_secs == album.duration_secs
        && stored.name == album.name
        && stored.artist == album.artist
        && stored.year == year
        && stored.cover_art_id == album.cover_art_id
        && stored.created == album.created
        && known.len() as u32 == album.song_count
}

fn remote_album_from_summary(server_id: &str, album: &AlbumSummary) -> RemoteAlbum {
    RemoteAlbum {
        server_id: server_id.to_owned(),
        album_id: album.id.clone(),
        song_count: album.song_count,
        duration_secs: album.duration_secs,
        name: album.name.clone(),
        artist: album.artist.clone(),
        year: (album.year > 0).then_some(i64::from(album.year)),
        cover_art_id: album.cover_art_id.clone(),
        created: album.created.clone(),
    }
}

/// Upsert one decoded batch so the library pane can render partial results
/// while the walk continues; the final prune only runs after a full success.
fn flush_remote_batch(
    library: &Library,
    tracks: &mut Vec<RemoteTrack>,
    albums: &mut Vec<RemoteAlbum>,
    report: &mut SyncReport,
) -> Result<(), String> {
    if !tracks.is_empty() {
        let batch = std::mem::take(tracks);
        let db = library
            .upsert_remote(&batch)
            .map_err(|e| format!("library sync failed: {e}"))?;
        report.tracks_added += db.added;
        report.tracks_updated += db.updated;
    }
    if !albums.is_empty() {
        let batch = std::mem::take(albums);
        library
            .upsert_remote_albums(&batch)
            .map_err(|e| format!("album fingerprint save failed: {e}"))?;
    }
    Ok(())
}

/// Song + enclosing album → library row. `album_artist` comes from the
/// album (Subsonic songs rarely carry one); the album fills gaps in
/// per-song tags; `codec` from the suffix, duration from `song.duration`.
fn map_song(album: &AlbumDetail, song: &SongSummary, server_id: &str) -> RemoteTrack {
    let album_artist = if album.artist.is_empty() {
        song.artist.clone()
    } else {
        album.artist.clone()
    };
    let year = (song.year > 0)
        .then_some(i64::from(song.year))
        .or_else(|| (album.year > 0).then_some(i64::from(album.year)));
    // Subsonic servers commonly expose legacy ID3 text without re-encoding
    // it, so the same charset repair as local tags applies here.
    let album_title = if song.album.is_empty() {
        album.name.clone()
    } else {
        song.album.clone()
    };
    let genre = song
        .genre
        .clone()
        .or_else(|| album.genre.clone())
        .unwrap_or_default();
    RemoteTrack {
        remote_id: song.id.clone(),
        server_id: server_id.to_owned(),
        remote_album_id: album.id.clone(),
        remote_path: (!song.path.is_empty()).then(|| song.path.clone()),
        title: repair_tag_text(&song.title).into_owned(),
        artist: repair_tag_text(&song.artist).into_owned(),
        album_artist: repair_tag_text(&album_artist).into_owned(),
        album: repair_tag_text(&album_title).into_owned(),
        genre: repair_tag_text(&genre).into_owned(),
        year,
        track_no: (song.track_no > 0).then_some(i64::from(song.track_no)),
        disc_no: (song.disc_no > 0).then_some(i64::from(song.disc_no)),
        duration_secs: song.duration_secs as f64,
        bitrate_kbps: (song.bitrate_kbps > 0).then_some(i64::from(song.bitrate_kbps)),
        samplerate: song.sampling_rate.map(i64::from),
        channels: song.channel_count.map(i64::from),
        codec: song.suffix.clone(),
        rating: i64::from(song.user_rating.min(5)),
        starred: song.starred,
        art_id: song
            .cover_art_id
            .clone()
            .or_else(|| album.cover_art_id.clone()),
        size: song.size.map(|s| i64::try_from(s).unwrap_or(i64::MAX)),
    }
}

fn emit_progress(app: &AppHandle, scanned: u64, total: u64, server: &str, label: &str) {
    let _ = app.emit(
        PROGRESS_EVENT,
        serde_json::json!({
            "scanned": scanned,
            "total": total,
            "server": server,
            "label": label,
        }),
    );
}

/// Blanks `http(s)://…` runs in `msg`: reqwest/Subsonic errors embed whole
/// request URLs, and ours carry auth tokens. Cheap scanner, no regex.
pub(crate) fn scrub_urls(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    loop {
        // First occurrence of either scheme.
        let pos = rest
            .find("http://")
            .zip(rest.find("https://"))
            .map(|(a, b)| a.min(b))
            .or_else(|| rest.find("http://"))
            .or_else(|| rest.find("https://"));
        let Some(pos) = pos else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..pos]);
        out.push_str("<url>");
        let tail = &rest[pos..];
        let end = tail
            .find(|c: char| c.is_whitespace() || c == ')' || c == ']')
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_round_trip() {
        for name in ["original", "mp3", "opus"] {
            assert_eq!(format_string(parse_format(name).unwrap()), name);
        }
        assert!(parse_format("aac").is_none());
    }

    #[test]
    fn scrubbing_removes_urls_with_queries() {
        let msg = "error sending request for url (https://nd.example/rest/stream.view?u=me&t=abc123&s=def&id=1): connect timeout";
        let scrubbed = scrub_urls(msg);
        assert!(scrubbed.contains("<url>"), "{scrubbed}");
        assert!(
            !scrubbed.contains("abc123"),
            "token must not survive: {scrubbed}"
        );
        assert!(scrubbed.ends_with(": connect timeout"), "{scrubbed}");
        assert_eq!(scrub_urls("no urls here"), "no urls here");
        // The ')' after a URL is its delimiter (reqwest's "for url (...)"),
        // not part of it — it survives.
        assert_eq!(scrub_urls("http://x and http://y)"), "<url> and <url>)");
    }

    #[test]
    fn legacy_single_server_config_migrates_to_one_profile() {
        let config = parse_config(
            r#"{ "url": "https://n.example", "user": "me", "password": "p",
                "format": "mp3", "bitrateKbps": 192, "libraryId": "lib-2" }"#,
        )
        .expect("legacy config parses");
        assert_eq!(config.active_id.as_deref(), Some(LEGACY_PROFILE_ID));
        assert_eq!(config.profiles.len(), 1);
        let profile = &config.profiles[0];
        assert_eq!(profile.id, LEGACY_PROFILE_ID);
        assert_eq!(profile.url, "https://n.example");
        assert_eq!(profile.bitrate_kbps, Some(192));
        assert_eq!(profile.synced_at, None);
        assert_eq!(profile.library_id.as_deref(), Some("lib-2"));
        assert_eq!(profile.name(), "me@n.example");
    }

    #[test]
    fn profiles_config_round_trips_with_camel_case() {
        let config = StoredConfig {
            active_id: Some("p2".into()),
            profiles: vec![
                StoredProfile {
                    id: "p2".into(),
                    label: Some("Home".into()),
                    url: "https://nd.example".into(),
                    user: "me".into(),
                    password: "p".into(),
                    format: "opus".into(),
                    bitrate_kbps: Some(128),
                    synced_at: Some(1),
                    server_version: Some("0.5x".into()),
                    library_id: Some("lib-2".into()),
                },
                StoredProfile {
                    id: "p3".into(),
                    url: "http://other.example:4533/music".into(),
                    user: "you".into(),
                    format: "original".into(),
                    ..StoredProfile::default()
                },
            ],
        };
        let text = serde_json::to_string(&config).unwrap();
        assert!(text.contains("\"activeId\":\"p2\""), "{text}");
        assert!(text.contains("\"label\":\"Home\""), "{text}");
        assert!(text.contains("\"bitrateKbps\":128"), "{text}");
        assert!(text.contains("\"libraryId\":\"lib-2\""), "{text}");
        assert_eq!(parse_config(&text).unwrap(), config);

        // A missing active id (or one that no longer exists) reads as none.
        let mut orphan = config.clone();
        orphan.active_id = Some("gone".into());
        assert!(
            parse_config(&serde_json::to_string(&orphan).unwrap())
                .unwrap()
                .active()
                .is_none()
        );
    }

    #[test]
    fn profile_ids_are_fresh_and_collision_safe() {
        let mut config = StoredConfig::default();
        assert_eq!(config.next_id(), "p1");
        config.profiles.push(StoredProfile {
            id: "p1".into(),
            ..StoredProfile::default()
        });
        assert_eq!(config.next_id(), "p2");
        // A gap/odd id must not make the generated id collide.
        config.profiles.push(StoredProfile {
            id: "p3".into(),
            ..StoredProfile::default()
        });
        assert_eq!(config.next_id(), "p4");
    }

    #[test]
    fn labels_fall_back_to_user_at_host() {
        let mut profile = StoredProfile {
            url: "https://music.example.com/".into(),
            user: "me".into(),
            ..StoredProfile::default()
        };
        assert_eq!(profile.name(), "me@music.example.com");
        profile.label = Some("  ".into());
        assert_eq!(profile.name(), "me@music.example.com");
        profile.label = Some(" Home ".into());
        assert_eq!(profile.name(), "Home");
        assert_eq!(normalized_label(" Home ").as_deref(), Some("Home"));
        assert_eq!(normalized_label("   "), None);
    }

    fn song(id: &str) -> SongSummary {
        SongSummary {
            id: id.into(),
            parent_id: "al-1".into(),
            title: format!("Song {id}"),
            artist: "Aphex Twin".into(),
            artist_id: "ar-1".into(),
            album: "Selected Ambient Works".into(),
            album_id: "al-1".into(),
            track_no: 1,
            disc_no: 0,
            year: 1992,
            genre: Some("Electronic".into()),
            duration_secs: 293,
            bitrate_kbps: 906,
            suffix: "flac".into(),
            content_type: "audio/flac".into(),
            sampling_rate: Some(44_100),
            channel_count: Some(2),
            size: Some(31_615_915),
            path: "Aphex Twin/Selected Ambient Works/01 Xtal.flac".into(),
            cover_art_id: Some("al-1".into()),
            play_count: 7,
            user_rating: 4,
            starred: false,
        }
    }

    #[test]
    fn playlist_rows_are_ephemeral_playable_tracks() {
        let rows = playlist_rows(&[song("a"), song("b")], "p1");
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].id, rows[1].id), (-1, -2));
        assert_eq!(rows[0].path, "navidrome://p1/a");
        assert_eq!(rows[0].remote_id.as_deref(), Some("a"));
        assert_eq!(rows[0].server_id.as_deref(), Some("p1"));
        assert_eq!(
            rows[0].remote_path.as_deref(),
            Some("Aphex Twin/Selected Ambient Works/01 Xtal.flac")
        );
        assert_eq!(rows[0].source, "navidrome");
        assert_eq!(rows[0].rating, 4);
        assert_eq!(rows[0].duration_secs, 293.0);
        assert_eq!(rows[0].art_id.as_deref(), Some("al-1"));
        assert_eq!(rows[0].codec, "flac");
    }

    #[test]
    fn remote_text_is_charset_repaired() {
        // "Песня" / "Альбом" encoded as CP1251 and read through Latin-1.
        let mut s = song("a");
        s.title = "Ïåñíÿ".into();
        s.album = "Àëüáîì".into();
        let album = AlbumDetail {
            id: "al-1".into(),
            name: String::new(),
            artist: "Èñïîëíèòåëü".into(),
            artist_id: "ar-1".into(),
            year: 0,
            genre: None,
            song_count: 1,
            duration_secs: 293,
            cover_art_id: Some("al-1".into()),
            songs: vec![],
        };

        let row = map_song(&album, &s, "p1");
        assert_eq!(row.title, "Песня");
        assert_eq!(row.album, "Альбом");
        assert_eq!(row.album_artist, "Исполнитель");
        assert_eq!(row.artist, "Aphex Twin", "ASCII tags stay untouched");

        let rows = playlist_rows(&[s], "p1");
        assert_eq!(rows[0].title, "Песня");
        assert_eq!(rows[0].album, "Альбом");
    }

    fn album_summary() -> AlbumSummary {
        AlbumSummary {
            id: "al-1".into(),
            name: "Debut".into(),
            artist: "Björk".into(),
            artist_id: "ar-1".into(),
            song_count: 11,
            duration_secs: 2900,
            cover_art_id: Some("al-1".into()),
            year: 1993,
            created: Some("2024-01-01T00:00:00Z".into()),
        }
    }

    #[test]
    fn album_fingerprint_requires_every_field_and_all_tracks() {
        let summary = album_summary();
        let stored = remote_album_from_summary("p1", &summary);
        let known = vec!["t1".to_string(); 11];
        assert!(album_matches(Some(&stored), &summary, &known));

        // A changed cheap field invalidates the fingerprint.
        let mut changed = summary.clone();
        changed.name = "Debut (Deluxe)".into();
        assert!(!album_matches(Some(&stored), &changed, &known));

        // A partial/legacy track set must be re-fetched even if metadata
        // matches: pruning needs every remote id for the album.
        assert!(!album_matches(Some(&stored), &summary, &known[..10]));

        // No stored fingerprint = new album.
        assert!(!album_matches(None, &summary, &known));
    }
}
