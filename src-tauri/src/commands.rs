/// Returns the crate version, exposed over Tauri IPC as a smoke-test command.
#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
