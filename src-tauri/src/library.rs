//! Library bridge: wires `navamp-library` into the Tauri shell.
//!
//! Layout:
//! - the SQLite DB lives at `<app_data_dir>/navamp.db`;
//! - the configured scan roots live at `<app_data_dir>/library-roots.json`
//!   (read at startup, rewritten by every [`library_scan`]);
//! - queries go through one long-lived connection (behind a brief mutex),
//!   while a scan runs on its **own** connection in a `std::thread` — the
//!   library's WAL mode lets the two coexist, so queries stay responsive
//!   (they see the pre-scan snapshot until the scan's transaction commits).
//!
//! Events:
//! - `library-progress` — `{ scanned, total }` while a scan runs (emitted
//!   after discovery and then throttled to every 32 files, plus the last
//!   one);
//! - `library-scanned` — the final [`ScanReport`] (or `{ error }` if the
//!   scan itself failed). [`library_scan`] also returns the report to the
//!   caller; the events exist so the UI can update without awaiting the
//!   invoke promise.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

use navamp_library::{Library, ScanProgress, ScanReport, Track, TrackFilter};

/// `library-progress` event name.
const PROGRESS_EVENT: &str = "library-progress";
/// `library-scanned` event name.
const SCANNED_EVENT: &str = "library-scanned";

/// Emit progress at most every N files (plus the very last one) so a
/// 100k-track scan doesn't fire 100k IPC events.
const PROGRESS_EVERY: u64 = 32;

/// Shared library state: the query connection (opened at startup or on
/// first use), the configured roots, and the one-scan-at-a-time flag.
///
/// `scanning` is an `Arc` so the scan thread can clear it even if the
/// invoking command future goes away (window reload, app quit).
#[derive(Default)]
pub struct LibraryState {
    query: Mutex<Option<Library>>,
    roots: Mutex<Vec<String>>,
    scanning: Arc<AtomicBool>,
}

/// Clears the scan flag when the thread finishes, however it finishes.
struct ScanGuard(Arc<AtomicBool>);

impl Drop for ScanGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Called from the app's `setup` hook: opens the database (creating
/// `<app_data_dir>` if needed) and loads the persisted roots. Both are
/// best-effort — commands re-open lazily and surface real errors.
pub fn init(app: &AppHandle) {
    let state = app.state::<LibraryState>();
    *state.roots.lock().unwrap() = read_roots(&roots_file(app).unwrap_or_else(|e| {
        eprintln!("navamp library: cannot resolve roots file: {e}");
        PathBuf::new()
    }));
    match db_file(app).and_then(|path| Library::open(path).map_err(|e| e.to_string())) {
        Ok(library) => *state.query.lock().unwrap() = Some(library),
        Err(e) => eprintln!("navamp library: deferring open: {e}"),
    }
}

/// `<app_data_dir>/navamp.db`, with the directory created.
pub(crate) fn db_file(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create `{}`: {e}", dir.display()))?;
    Ok(dir.join("navamp.db"))
}

/// `<app_data_dir>/library-roots.json`.
fn roots_file(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))?;
    Ok(dir.join("library-roots.json"))
}

/// Reads the persisted roots; a missing or malformed file is an empty list
/// (first run, or a hand-edited file we refuse to crash on).
fn read_roots(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Persists the roots as pretty JSON.
fn write_roots(path: &std::path::Path, roots: &[String]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create `{}`: {e}", parent.display()))?;
    }
    let text =
        serde_json::to_string_pretty(roots).map_err(|e| format!("cannot encode roots: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("cannot write `{}`: {e}", path.display()))
}

/// Runs `f` against the shared query connection, opening it on first use.
/// (Also used by `smart.rs` — the Smart View commands share this state.)
pub(crate) fn with_library<T>(
    app: &AppHandle,
    state: &LibraryState,
    f: impl FnOnce(&Library) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = state.query.lock().unwrap();
    if guard.is_none() {
        let library = Library::open(db_file(app)?).map_err(|e| e.to_string())?;
        *guard = Some(library);
    }
    f(guard.as_ref().expect("just opened"))
}

/// Scans `roots` incrementally in the background and returns the report.
///
/// Progress streams as `library-progress` events; the final report arrives
/// both as the `library-scanned` event and as this command's return value
/// (the command resolves when the scan finishes — the UI never has to await
/// it to stay informed). A second call while a scan runs fails with an
/// error string.
#[tauri::command]
pub async fn library_scan(
    app: AppHandle,
    state: State<'_, LibraryState>,
    roots: Vec<String>,
) -> Result<ScanReport, String> {
    let roots: Vec<String> = roots
        .into_iter()
        .map(|r| r.trim().to_owned())
        .filter(|r| !r.is_empty())
        .collect();
    if roots.is_empty() {
        return Err("no scan roots provided".into());
    }

    // Everything that can fail happens *before* the one-scan flag is
    // taken, so an early error can never leave `scanning` stuck. (Two
    // concurrent calls may briefly each open a scan connection; the loser's
    // is simply dropped — SQLite doesn't mind.)
    let roots_path = roots_file(&app)?;
    write_roots(&roots_path, &roots)?;
    let scan_library =
        Library::open(db_file(&app)?).map_err(|e| format!("cannot open library: {e}"))?;

    if state.scanning.swap(true, Ordering::SeqCst) {
        return Err("a library scan is already running".into());
    }
    *state.roots.lock().unwrap() = roots.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<ScanReport, String>>();
    let emitter = app.clone();
    let flag = Arc::clone(&state.scanning);

    std::thread::Builder::new()
        .name("navamp-library-scan".into())
        .spawn(move || {
            let _guard = ScanGuard(flag);
            let result = scan_library.scan_roots(
                &roots.iter().map(PathBuf::from).collect::<Vec<_>>(),
                |progress| match progress {
                    ScanProgress::Progress { scanned, total } => {
                        if scanned % PROGRESS_EVERY == 0 || scanned == total {
                            let _ = emitter.emit(
                                PROGRESS_EVENT,
                                json!({ "scanned": scanned, "total": total }),
                            );
                        }
                    }
                    ScanProgress::Discovered { total } => {
                        let _ =
                            emitter.emit(PROGRESS_EVENT, json!({ "scanned": 0, "total": total }));
                    }
                },
            );
            match &result {
                Ok(report) => {
                    let _ = emitter.emit(SCANNED_EVENT, report);
                }
                Err(e) => {
                    let _ = emitter.emit(SCANNED_EVENT, json!({ "error": e.to_string() }));
                }
            }
            let _ = tx.send(result.map_err(|e| e.to_string()));
        })
        .map_err(|e| {
            state.scanning.store(false, Ordering::SeqCst);
            format!("cannot spawn scan thread: {e}")
        })?;

    let report = tauri::async_runtime::spawn_blocking(move || {
        rx.recv().map_err(|_| "scan thread died".to_string())?
    })
    .await
    .map_err(|e| format!("scan join failed: {e}"))??;
    Ok(report)
}

/// FTS5 search over title/artist/album/genre (ranked, prefix-friendly).
#[tauri::command]
pub fn library_search(
    app: AppHandle,
    state: State<'_, LibraryState>,
    q: String,
    limit: Option<u32>,
) -> Result<Vec<Track>, String> {
    with_library(&app, &state, |library| {
        library
            .search(&q, limit.unwrap_or(50))
            .map_err(|e| e.to_string())
    })
}

/// All albums grouped by album key, ordered artist → year → title.
#[tauri::command]
pub fn library_albums(
    app: AppHandle,
    state: State<'_, LibraryState>,
) -> Result<Vec<navamp_library::Album>, String> {
    with_library(&app, &state, |library| {
        library.albums().map_err(|e| e.to_string())
    })
}

/// One album's tracks in disc/track order (`albumKey` on the JS side).
#[tauri::command]
pub fn library_album_tracks(
    app: AppHandle,
    state: State<'_, LibraryState>,
    album_key: String,
) -> Result<Vec<Track>, String> {
    with_library(&app, &state, |library| {
        library.album_tracks(&album_key).map_err(|e| e.to_string())
    })
}

/// Tracks filtered by optional artist/genre/source with limit/offset paging.
#[tauri::command]
pub fn library_tracks(
    app: AppHandle,
    state: State<'_, LibraryState>,
    artist: Option<String>,
    genre: Option<String>,
    source: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<Track>, String> {
    with_library(&app, &state, |library| {
        library
            .tracks_filtered(&TrackFilter {
                artist,
                genre,
                source,
                limit,
                offset: offset.unwrap_or(0),
                ..TrackFilter::default()
            })
            .map_err(|e| e.to_string())
    })
}

/// Library counters (`lastScan` is null until the first scan).
#[tauri::command]
pub fn library_stats(
    app: AppHandle,
    state: State<'_, LibraryState>,
) -> Result<navamp_library::LibraryStats, String> {
    with_library(&app, &state, |library| {
        library.stats().map_err(|e| e.to_string())
    })
}

/// The persisted scan roots, as configured by the last [`library_scan`].
/// Handy for the UI's "rescan my library" button, which reuses them.
#[tauri::command]
pub fn library_roots(state: State<'_, LibraryState>) -> Vec<String> {
    state.roots.lock().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("navamp-roots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library-roots.json");

        assert!(read_roots(&path).is_empty()); // missing file -> empty

        let roots = vec!["/music/a".into(), "/music/b".into()];
        write_roots(&path, &roots).unwrap();
        assert_eq!(read_roots(&path), roots);

        std::fs::write(&path, "{not json").unwrap();
        assert!(read_roots(&path).is_empty()); // malformed -> empty, no panic

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
