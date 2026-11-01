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
//! - `navidrome-progress` — `{ scanned, total }` in **album** units,
//!   throttled to every 32 albums (plus the first and last);
//! - `navidrome-synced` — the final [`SyncReport`] (or `{ error }`).
//!
//! Credential hygiene: Subsonic stream/API URLs embed the auth token;
//! every error string that crosses IPC is scrubbed of URLs (`<url>`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use navinamp_library::{Library, RemoteTrack, Track, remote_path};
use navinamp_subsonic::{AlbumDetail, PlaylistSummary, SongSummary, StreamFormat, SubsonicClient};

use crate::library::{LibraryState, db_file, with_library};

/// `navidrome-progress` event name.
const PROGRESS_EVENT: &str = "navidrome-progress";
/// `navidrome-synced` event name.
const SYNCED_EVENT: &str = "navidrome-synced";
/// Emit progress at most every N albums (plus the very last one).
const PROGRESS_EVERY: u64 = 32;
/// The `source` value for synced rows.
const SOURCE: &str = "navidrome";

// ---------------------------------------------------------------------------
// persisted config
// ---------------------------------------------------------------------------

/// Contents of `<app_data_dir>/navidrome.json`. The five credential/pref
/// keys are the frozen on-disk shape; `syncedAt`/`serverVersion` are our
/// persisted extras (a superset keeps the file the single source of truth).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredConfig {
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
}

impl StoredConfig {
    fn stream_format(&self) -> Option<StreamFormat> {
        parse_format(&self.format)
    }
}

// ---------------------------------------------------------------------------
// state
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Inner {
    config: Option<StoredConfig>,
    /// Lazily built on first use (reconnect pings the server once).
    client: Option<Arc<SubsonicClient>>,
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
/// app launch; the first command that needs it reconnects.
pub fn init(app: &AppHandle) {
    let Ok(path) = config_file(app) else {
        return; // no app data dir: commands surface real errors later
    };
    let config: StoredConfig = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => return,
    };
    if config.url.is_empty() {
        return;
    }
    let state = app.state::<NavidromeState>();
    state.inner.lock().unwrap().config = Some(config);
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
    let config = guard.config.as_ref();
    Status {
        configured: config.is_some(),
        url: config.map(|c| c.url.clone()),
        user: config.map(|c| c.user.clone()),
        streaming: StreamingPrefs {
            format: config
                .map(|c| c.format.clone())
                .unwrap_or_else(|| "original".into()),
            bitrate_kbps: config.and_then(|c| c.bitrate_kbps),
        },
        synced_at: config.and_then(|c| c.synced_at),
        server_version: config.and_then(|c| c.server_version.clone()),
        // Unconfigured → 0; configured → the live count (an unopenable
        // library reads as 0 too — status must never fail).
        track_count: if config.is_some() {
            with_library(app, &library, |lib| {
                lib.track_count(Some(SOURCE)).map_err(|e| e.to_string())
            })
            .unwrap_or(0) as i64
        } else {
            0
        },
    }
}

// ---------------------------------------------------------------------------
// client cache
// ---------------------------------------------------------------------------

/// The shared client, (re)connecting with the stored credentials if needed.
/// Errors are scrubbed of URLs (Subsonic tokens ride in query strings).
async fn ensure_client(state: &NavidromeState) -> Result<Arc<SubsonicClient>, String> {
    {
        let guard = state.inner.lock().unwrap();
        if let Some(client) = guard.client.as_ref() {
            return Ok(Arc::clone(client));
        }
        if guard.config.is_none() {
            return Err("not connected".into());
        }
    }

    let (url, user, password) = {
        let guard = state.inner.lock().unwrap();
        let config = guard.config.as_ref().expect("checked above");
        (
            config.url.clone(),
            config.user.clone(),
            config.password.clone(),
        )
    };
    let client = Arc::new(
        SubsonicClient::connect(&url, &user, &password)
            .await
            .map_err(|e| scrub_urls(&e.to_string()))?,
    );

    let mut guard = state.inner.lock().unwrap();
    // Disconnected while we were connecting: drop the client, honor the
    // user's intent.
    let Some(config) = guard.config.as_mut() else {
        return Err("not connected".into());
    };
    if config.server_version.is_none() {
        config.server_version = client.server_version().map(str::to_owned);
    }
    Ok(guard
        .client
        .get_or_insert_with(|| Arc::clone(&client))
        .clone())
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

/// Connection + sync snapshot (polled by the UI).
#[tauri::command]
pub fn navidrome_status(app: AppHandle, state: State<'_, NavidromeState>) -> Status {
    current_status(&app, &state)
}

/// Ping + save credentials on success; returns the new status.
#[tauri::command]
pub async fn navidrome_connect(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    url: String,
    user: String,
    password: String,
) -> Result<Status, String> {
    let url = url.trim().trim_end_matches('/').to_owned();
    let user = user.trim().to_owned();
    if url.is_empty() || user.is_empty() {
        return Err("url and user are required".into());
    }

    // Ping first: nothing is persisted for a server that rejects us.
    let client = SubsonicClient::connect(&url, &user, &password)
        .await
        .map_err(|e| scrub_urls(&e.to_string()))?;

    let mut guard = state.inner.lock().unwrap();
    // Keep the previous streaming prefs across a reconnect; a fresh
    // configuration starts at `original` / unlimited.
    let (format, bitrate_kbps) = guard
        .config
        .as_ref()
        .map(|c| (c.format.clone(), c.bitrate_kbps))
        .unwrap_or_else(|| ("original".into(), None));
    let config = StoredConfig {
        url,
        user,
        password,
        format,
        bitrate_kbps,
        synced_at: None,
        server_version: client.server_version().map(str::to_owned),
    };
    save_config(&app, &config)?;
    *guard = Inner {
        config: Some(config),
        client: Some(Arc::new(client)),
    };
    drop(guard);
    Ok(current_status(&app, &state))
}

/// Forgets the credentials (file + memory) and deletes every synced row.
#[tauri::command]
pub async fn navidrome_disconnect(
    app: AppHandle,
    state: State<'_, NavidromeState>,
    library: State<'_, LibraryState>,
) -> Result<Status, String> {
    {
        let mut guard = state.inner.lock().unwrap();
        guard.config = None;
        guard.client = None;
    }
    if let Ok(path) = config_file(&app) {
        let _ = std::fs::remove_file(path);
    }
    // An empty batch is a "remove everything of this source" sync; local
    // rows are never touched by it.
    with_library(&app, &library, |lib| {
        lib.sync_remote(Vec::new()).map_err(|e| e.to_string())
    })?;
    Ok(current_status(&app, &state))
}

/// The stream URL for a remote track id, honoring the streaming prefs.
#[tauri::command]
pub async fn navidrome_stream_url(
    state: State<'_, NavidromeState>,
    remote_id: String,
) -> Result<String, String> {
    let client = ensure_client(&state).await?;
    let (format, bitrate) = {
        let guard = state.inner.lock().unwrap();
        let config = guard.config.as_ref().ok_or("not connected")?;
        (
            config.stream_format().unwrap_or(StreamFormat::Original),
            config.bitrate_kbps,
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
    remote_id: String,
    size: Option<u32>,
) -> Result<String, String> {
    let client = ensure_client(&state).await?;
    Ok(client.cover_art_url(&remote_id, size).to_string())
}

/// Scrobble a track: `submission=false` → "now playing", `true` → played.
#[tauri::command]
pub async fn navidrome_scrobble(
    state: State<'_, NavidromeState>,
    remote_id: String,
    submission: bool,
) -> Result<(), String> {
    let client = ensure_client(&state).await?;
    client
        .scrobble(&remote_id, submission, None)
        .await
        .map_err(|e| scrub_urls(&e.to_string()))
}

/// Star or unstar a track on the server.
#[tauri::command]
pub async fn navidrome_star(
    state: State<'_, NavidromeState>,
    remote_id: String,
    starred: bool,
) -> Result<(), String> {
    let client = ensure_client(&state).await?;
    let result = if starred {
        client.star(&remote_id).await
    } else {
        client.unstar(&remote_id).await
    };
    result.map_err(|e| scrub_urls(&e.to_string()))
}

/// Set the user rating (0 = clear, 1–5 stars) for a track on the server.
#[tauri::command]
pub async fn navidrome_rate(
    state: State<'_, NavidromeState>,
    remote_id: String,
    rating: u8,
) -> Result<(), String> {
    if rating > 5 {
        return Err(format!("rating {rating} out of range (0..=5)"));
    }
    let client = ensure_client(&state).await?;
    client
        .rate(&remote_id, rating)
        .await
        .map_err(|e| scrub_urls(&e.to_string()))
}

/// All playlists visible to the connected user (server order).
#[tauri::command]
pub async fn navidrome_playlists(
    state: State<'_, NavidromeState>,
) -> Result<Vec<PlaylistSummary>, String> {
    let client = ensure_client(&state).await?;
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
    playlist_id: String,
) -> Result<Vec<Track>, String> {
    let client = ensure_client(&state).await?;
    let songs = client
        .get_playlist(&playlist_id)
        .await
        .map_err(|e| scrub_urls(&e.to_string()))?;
    Ok(playlist_rows(&songs))
}

/// Map server playlist entries onto [`Track`] rows for the library table.
fn playlist_rows(songs: &[SongSummary]) -> Vec<Track> {
    songs
        .iter()
        .enumerate()
        .map(|(i, song)| Track {
            id: -((i as i64) + 1),
            path: remote_path(&song.id),
            mtime: 0,
            size: song
                .size
                .map(|s| i64::try_from(s).unwrap_or(i64::MAX))
                .unwrap_or(0),
            title: song.title.clone(),
            artist: song.artist.clone(),
            album_artist: song.artist.clone(),
            album: song.album.clone(),
            album_key: String::new(),
            genre: song.genre.clone().unwrap_or_default(),
            year: (song.year > 0).then_some(i64::from(song.year)),
            track_no: (song.track_no > 0).then_some(i64::from(song.track_no)),
            disc_no: (song.disc_no > 0).then_some(i64::from(song.disc_no)),
            duration_secs: song.duration_secs as f64,
            bitrate_kbps: (song.bitrate_kbps > 0).then_some(i64::from(song.bitrate_kbps)),
            samplerate: song.sampling_rate.map(i64::from),
            channels: song.channel_count.map(i64::from),
            codec: song.suffix.clone(),
            rating: i64::from(song.user_rating.min(5)),
            play_count: i64::try_from(song.play_count).unwrap_or(i64::MAX),
            last_played_at: None,
            date_added: 0,
            source: SOURCE.into(),
            remote_id: Some(song.id.clone()),
            art_id: song.cover_art_id.clone(),
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
) -> Result<PublishReport, String> {
    let client = ensure_client(&state).await?;
    let library = Library::open(db_file(&app)?).map_err(|e| format!("cannot open library: {e}"))?;
    let view = library
        .smart_views_list()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|v| v.id == id)
        .ok_or_else(|| format!("smart view {id} not found"))?;
    let tracks = library.smart_view_run(id).map_err(|e| e.to_string())?;
    let song_ids: Vec<String> = tracks
        .iter()
        .filter(|t| t.source == SOURCE)
        .filter_map(|t| t.remote_id.clone())
        .collect();
    if song_ids.is_empty() {
        return Err("this smart view has no Navidrome tracks to publish".into());
    }

    let name = format!("{SMART_PLAYLIST_PREFIX}{}", view.def.name);
    let mapped = library
        .smart_view_playlist_id(id)
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
        .set_smart_view_playlist_id(id, &playlist_id)
        .map_err(|e| e.to_string())?;

    Ok(PublishReport {
        playlist_id,
        name,
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

    let mut guard = state.inner.lock().unwrap();
    let config = guard.config.as_mut().ok_or("not connected")?;
    config.format = format_string(parsed).into();
    config.bitrate_kbps = bitrate;
    save_config(&app, config)?;
    drop(guard);
    Ok(current_status(&app, &state))
}

/// Full-library sync: `getArtists` → each artist's albums → `getAlbum` per
/// album → one `sync_remote` transaction. Runs on a dedicated thread; one
/// sync at a time (a second invoke fails with an error string). Progress in
/// `navidrome-progress` events (album units), final `navidrome-synced`.
#[tauri::command]
pub async fn navidrome_sync(
    app: AppHandle,
    state: State<'_, NavidromeState>,
) -> Result<SyncReport, String> {
    if state.inner.lock().unwrap().config.is_none() {
        return Err("not connected".into());
    }
    if state.syncing.swap(true, Ordering::SeqCst) {
        return Err("a navidrome sync is already running".into());
    }
    // Everything fallible happens before the thread spawns (or inside it),
    // and the thread's guard clears the flag whatever happens.
    let client = ensure_client(&state).await.inspect_err(|_| {
        state.syncing.store(false, Ordering::SeqCst);
    })?;
    let library = Library::open(db_file(&app).inspect_err(|_| {
        state.syncing.store(false, Ordering::SeqCst);
    })?)
    .map_err(|e| {
        state.syncing.store(false, Ordering::SeqCst);
        format!("cannot open library: {e}")
    })?;

    let (tx, rx) = std::sync::mpsc::channel::<Result<SyncReport, String>>();
    let emitter = app.clone();
    let flag = Arc::clone(&state.syncing);
    let inner = Arc::clone(&state.inner);

    std::thread::Builder::new()
        .name("navinamp-navidrome-sync".into())
        .spawn(move || {
            let _guard = SyncGuard(flag);
            let result = run_sync(&emitter, &client, &library);
            match &result {
                Ok(report) => {
                    // Persist the timestamp while we still own the app handle.
                    let mut guard = inner.lock().unwrap();
                    if let Some(config) = guard.config.as_mut() {
                        config.synced_at = Some(
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs() as i64)
                                .unwrap_or(0),
                        );
                        let _ = save_config(&emitter, config);
                    }
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

// ---------------------------------------------------------------------------
// the walk
// ---------------------------------------------------------------------------

/// The sync walk (runs on its own thread): fetch artists → their albums →
/// each album's songs, then hand the whole batch to `sync_remote`.
fn run_sync(
    app: &AppHandle,
    client: &SubsonicClient,
    library: &Library,
) -> Result<SyncReport, String> {
    let started = Instant::now();
    tauri::async_runtime::block_on(async {
        let mut report = SyncReport::default();

        let artists = client
            .get_artists()
            .await
            .map_err(|e| scrub_urls(&e.to_string()))?;
        report.artists = artists.len() as u64;
        let total: u64 = artists.iter().map(|a| u64::from(a.album_count)).sum();
        emit_progress(app, 0, total);

        let mut tracks: Vec<RemoteTrack> = Vec::new();
        let mut scanned: u64 = 0;
        for artist in &artists {
            // Per-artist failure ( vanished artist, network hiccup) is
            // reported and skipped: a sync never dies from one bad node.
            let albums = match client.get_artist(&artist.id).await {
                Ok(albums) => albums,
                Err(e) => {
                    report.errors.push(format!(
                        "artist `{}`: {}",
                        artist.name,
                        scrub_urls(&e.to_string())
                    ));
                    scanned += u64::from(artist.album_count);
                    continue;
                }
            };
            report.albums += albums.len() as u64;
            for album in albums {
                match client.get_album(&album.id).await {
                    Ok(detail) => {
                        tracks.extend(detail.songs.iter().map(|song| map_song(&detail, song)))
                    }
                    Err(e) => report.errors.push(format!(
                        "album `{}`: {}",
                        album.name,
                        scrub_urls(&e.to_string())
                    )),
                }
                scanned += 1;
                if scanned.is_multiple_of(PROGRESS_EVERY) || scanned == total {
                    emit_progress(app, scanned.min(total), total);
                }
            }
        }

        let db_report = library
            .sync_remote(tracks)
            .map_err(|e| format!("library sync failed: {e}"))?;
        report.tracks_added = db_report.added;
        report.tracks_updated = db_report.updated;
        report.tracks_removed = db_report.removed;
        report.duration_ms = started.elapsed().as_millis() as u64;
        Ok(report)
    })
}

/// Song + enclosing album → library row. `album_artist` comes from the
/// album (Subsonic songs rarely carry one); the album fills gaps in
/// per-song tags; `codec` from the suffix, duration from `song.duration`.
fn map_song(album: &AlbumDetail, song: &SongSummary) -> RemoteTrack {
    let album_artist = if album.artist.is_empty() {
        song.artist.clone()
    } else {
        album.artist.clone()
    };
    let year = (song.year > 0)
        .then_some(i64::from(song.year))
        .or_else(|| (album.year > 0).then_some(i64::from(album.year)));
    RemoteTrack {
        remote_id: song.id.clone(),
        title: song.title.clone(),
        artist: song.artist.clone(),
        album_artist,
        album: if song.album.is_empty() {
            album.name.clone()
        } else {
            song.album.clone()
        },
        genre: song
            .genre
            .clone()
            .or_else(|| album.genre.clone())
            .unwrap_or_default(),
        year,
        track_no: (song.track_no > 0).then_some(i64::from(song.track_no)),
        disc_no: (song.disc_no > 0).then_some(i64::from(song.disc_no)),
        duration_secs: song.duration_secs as f64,
        bitrate_kbps: (song.bitrate_kbps > 0).then_some(i64::from(song.bitrate_kbps)),
        samplerate: song.sampling_rate.map(i64::from),
        channels: song.channel_count.map(i64::from),
        codec: song.suffix.clone(),
        rating: i64::from(song.user_rating.min(5)),
        art_id: song
            .cover_art_id
            .clone()
            .or_else(|| album.cover_art_id.clone()),
        size: song.size.map(|s| i64::try_from(s).unwrap_or(i64::MAX)),
    }
}

fn emit_progress(app: &AppHandle, scanned: u64, total: u64) {
    let _ = app.emit(
        PROGRESS_EVENT,
        serde_json::json!({ "scanned": scanned, "total": total }),
    );
}

/// Blanks `http(s)://…` runs in `msg`: reqwest/Subsonic errors embed whole
/// request URLs, and ours carry auth tokens. Cheap scanner, no regex.
fn scrub_urls(msg: &str) -> String {
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
    fn stored_config_parses_the_frozen_shape() {
        let config: StoredConfig = serde_json::from_str(
            r#"{ "url": "https://n.example", "user": "me", "password": "p",
                "format": "mp3", "bitrateKbps": 192 }"#,
        )
        .unwrap();
        assert_eq!(config.url, "https://n.example");
        assert_eq!(config.bitrate_kbps, Some(192));
        assert_eq!(config.synced_at, None);
        // And the camelCase keys we write are the same shape.
        let text = serde_json::to_string(&StoredConfig {
            url: "u".into(),
            user: "me".into(),
            password: "p".into(),
            format: "opus".into(),
            bitrate_kbps: Some(128),
            synced_at: Some(1),
            server_version: Some("0.5x".into()),
        })
        .unwrap();
        assert!(text.contains("\"bitrateKbps\":128"), "{text}");
        assert!(text.contains("\"syncedAt\":1"), "{text}");
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
            cover_art_id: Some("al-1".into()),
            play_count: 7,
            user_rating: 4,
            starred: false,
        }
    }

    #[test]
    fn playlist_rows_are_ephemeral_playable_tracks() {
        let rows = playlist_rows(&[song("a"), song("b")]);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].id, rows[1].id), (-1, -2));
        assert_eq!(rows[0].path, "navidrome://a");
        assert_eq!(rows[0].remote_id.as_deref(), Some("a"));
        assert_eq!(rows[0].source, "navidrome");
        assert_eq!(rows[0].rating, 4);
        assert_eq!(rows[0].duration_secs, 293.0);
        assert_eq!(rows[0].art_id.as_deref(), Some("al-1"));
        assert_eq!(rows[0].codec, "flac");
    }
}
