mod art;
mod audio;
mod commands;
mod library;
mod media;
mod navidrome;
mod skin;
mod smart;
mod windows;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Serves cached album-art bytes to the webview (`<img src>`), see
        // `art.rs` for the accepted URL shapes.
        .register_uri_scheme_protocol(art::SCHEME, art::handle)
        .manage(audio::Audio::default())
        .manage(skin::SkinCache::default())
        .manage(library::LibraryState::default())
        .manage(navidrome::NavidromeState::default())
        .manage(windows::WindowManager::default())
        .manage(media::MediaHub::default())
        .setup(|app| {
            library::init(app.handle());
            navidrome::init(app.handle());
            // Creates the playlist/library windows, restores persisted
            // geometry and registers the queue-event relays.
            app.state::<windows::WindowManager>().init(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            window
                .app_handle()
                .state::<windows::WindowManager>()
                .on_window_event(window, event);
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
            library::expand_audio_paths,
            library::library_mark_played,
            library::library_set_rating,
            library::art_for_track,
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
            navidrome::navidrome_art_url,
            navidrome::navidrome_scrobble,
            navidrome::navidrome_star,
            navidrome::navidrome_rate,
            navidrome::navidrome_playlists,
            navidrome::navidrome_playlist_tracks,
            navidrome::navidrome_set_streaming,
            windows::windows_get,
            windows::window_set_visible,
            windows::window_toggle_layout,
            windows::window_set_size,
            windows::window_set_always_on_top,
            windows::window_drag_region_ready,
            media::media_metadata,
            media::media_playback
        ])
        .build(tauri::generate_context!())
        .expect("error while building NaVinaMP");

    app.run(|app_handle, event| {
        // Persist window geometry on any exit path (player close, last
        // window closed, app.exit) — positions survive restarts.
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            app_handle
                .state::<windows::WindowManager>()
                .flush(app_handle);
        }
    });
}
