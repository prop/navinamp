mod audio;
mod commands;
mod library;
mod navidrome;
mod skin;
mod smart;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(audio::Audio::default())
        .manage(skin::SkinCache::default())
        .manage(library::LibraryState::default())
        .manage(navidrome::NavidromeState::default())
        .setup(|app| {
            library::init(app.handle());
            navidrome::init(app.handle());
            Ok(())
        })
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
            audio::eq_state,
            audio::eq_set_enabled,
            audio::eq_set_preamp,
            audio::eq_set_band,
            audio::eq_presets,
            audio::audio_spectrum,
            skin::skin_list,
            skin::skin_load,
            library::library_scan,
            library::library_search,
            library::library_albums,
            library::library_album_tracks,
            library::library_tracks,
            library::library_stats,
            library::library_roots,
            library::library_mark_played,
            smart::smart_views_list,
            smart::smart_view_save,
            smart::smart_view_delete,
            smart::smart_view_run,
            smart::smart_view_preview,
            navidrome::navidrome_status,
            navidrome::navidrome_connect,
            navidrome::navidrome_disconnect,
            navidrome::navidrome_sync,
            navidrome::navidrome_stream_url,
            navidrome::navidrome_scrobble,
            navidrome::navidrome_star,
            navidrome::navidrome_set_streaming
        ])
        .run(tauri::generate_context!())
        .expect("error while running NavAmp");
}
