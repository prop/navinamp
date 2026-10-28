//! Smart View bridge: thin wrappers over `navamp_library`'s rule engine,
//! sharing the library connection/state with `library.rs`.
//!
//! Validation happens inside the library (before anything is persisted);
//! its messages name the offending field/op/rule index and are surfaced
//! verbatim as `Err(String)` for inline display in the rule builder.

use tauri::{AppHandle, State};

use navamp_library::{SavedSmartView, SmartViewDef, Track};

use crate::library::{LibraryState, with_library};

/// All saved smart views, in tree order.
#[tauri::command]
pub fn smart_views_list(
    app: AppHandle,
    state: State<'_, LibraryState>,
) -> Result<Vec<SavedSmartView>, String> {
    with_library(&app, &state, |library| {
        library.smart_views_list().map_err(|e| e.to_string())
    })
}

/// Validates `def` and upserts it: `id: null` appends, `id: number`
/// updates that row. Returns the stored id.
#[tauri::command]
pub fn smart_view_save(
    app: AppHandle,
    state: State<'_, LibraryState>,
    id: Option<i64>,
    def: SmartViewDef,
) -> Result<i64, String> {
    with_library(&app, &state, |library| {
        library.smart_view_save(id, &def).map_err(|e| e.to_string())
    })
}

/// Deletes a saved view (unknown ids are a no-op).
#[tauri::command]
pub fn smart_view_delete(
    app: AppHandle,
    state: State<'_, LibraryState>,
    id: i64,
) -> Result<(), String> {
    with_library(&app, &state, |library| {
        library.smart_view_delete(id).map_err(|e| e.to_string())
    })
}

/// Runs a saved view by id.
#[tauri::command]
pub fn smart_view_run(
    app: AppHandle,
    state: State<'_, LibraryState>,
    id: i64,
) -> Result<Vec<Track>, String> {
    with_library(&app, &state, |library| {
        library.smart_view_run(id).map_err(|e| e.to_string())
    })
}

/// Validates and evaluates `def` without persisting it (capped at 500
/// rows, or the def's own smaller limit).
#[tauri::command]
pub fn smart_view_preview(
    app: AppHandle,
    state: State<'_, LibraryState>,
    def: SmartViewDef,
) -> Result<Vec<Track>, String> {
    with_library(&app, &state, |library| {
        library.smart_view_preview(&def).map_err(|e| e.to_string())
    })
}
