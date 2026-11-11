//! Small on-disk state files that must survive restarts (the play queue).
//!
//! Writes are atomic (temp file + rename) and the temp file is fsynced before
//! the rename, so a crash mid-write cannot leave a truncated queue behind —
//! the previous file stays intact until the new one is fully on disk. The
//! file lives in the app data dir next to the library database
//! (`navinamp.db`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

/// File name inside the app data dir.
const QUEUE_FILE: &str = "queue.json";

fn queue_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join(QUEUE_FILE))
        .map_err(|e| format!("no app data dir: {e}"))
}

/// Persist the queue JSON. The frontend owns the schema; this side only
/// guarantees a durable, atomic write.
#[tauri::command]
pub fn state_save_queue(app: AppHandle, json: String) -> Result<(), String> {
    let path = queue_path(&app)?;
    save_at(&path, &json)
}

/// Read the saved queue JSON (`None` when nothing has been saved yet).
#[tauri::command]
pub fn state_load_queue(app: AppHandle) -> Result<Option<String>, String> {
    let path = queue_path(&app)?;
    load_at(&path)
}

fn save_at(path: &Path, json: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("queue path has no parent")?;
    fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    let mut file = fs::File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
    file.write_all(json.as_bytes())
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    // Flush the data before the rename replaces the live file, otherwise a
    // power loss could leave an empty queue.json behind the new name.
    file.sync_all()
        .map_err(|e| format!("sync {}: {e}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|e| format!("replace {}: {e}", path.display()))
}

fn load_at(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("navinamp-queue-{name}-{}", std::process::id()))
            .join("queue.json")
    }

    #[test]
    fn save_load_round_trip_leaves_no_temp_file() {
        let path = temp_path("roundtrip");
        save_at(&path, "{\"items\":[]}").expect("save");
        assert_eq!(
            load_at(&path).expect("load").as_deref(),
            Some("{\"items\":[]}")
        );
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn overwrite_replaces_previous_contents() {
        let path = temp_path("overwrite");
        save_at(&path, "first").expect("first save");
        save_at(&path, "second").expect("second save");
        assert_eq!(load_at(&path).expect("load").as_deref(), Some("second"));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_file_loads_as_none() {
        let path = temp_path("missing");
        assert_eq!(load_at(&path).expect("load"), None);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
