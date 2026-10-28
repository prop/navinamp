mod audio;
mod commands;
mod skin;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(audio::Audio::default())
        .manage(skin::SkinCache::default())
        .invoke_handler(tauri::generate_handler![
            commands::app_version,
            audio::audio_play,
            audio::audio_enqueue,
            audio::audio_pause,
            audio::audio_resume,
            audio::audio_stop,
            audio::audio_seek,
            audio::audio_set_volume,
            audio::audio_state,
            skin::skin_list,
            skin::skin_load
        ])
        .run(tauri::generate_context!())
        .expect("error while running NavAmp");
}
