mod ai;
mod ai_rules;
mod art;
mod audio;
mod commands;
mod library;
mod media;
mod menu;
mod navidrome;
mod semantic;
mod skin;
mod smart;
mod streams;
mod windows;

#[cfg(desktop)]
use tauri::Emitter;
use tauri::Manager;

/// Global "raise now playing" shortcut. macOS uses Control+Command+O;
/// Windows/Linux use Control+Alt+O (the common cross-platform analogue).
#[cfg(all(desktop, target_os = "macos"))]
const NOTIFY_SHORTCUT: &str = "Control+Command+O";
#[cfg(all(desktop, not(target_os = "macos")))]
const NOTIFY_SHORTCUT: &str = "Control+Alt+O";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());
    let app = builder
        // Serves cached album-art bytes to the webview (`<img src>`), see
        // `art.rs` for the accepted URL shapes.
        .register_uri_scheme_protocol(art::SCHEME, art::handle)
        .manage(audio::Audio::default())
        .manage(skin::SkinCache::default())
        .manage(library::LibraryState::default())
        .manage(navidrome::NavidromeState::default())
        .manage(semantic::SemanticState::default())
        .manage(streams::StreamHeaders::default())
        .manage(windows::WindowManager::default())
        .manage(media::MediaHub::default())
        .setup(|app| {
            audio::init(app.handle());
            library::init(app.handle());
            navidrome::init(app.handle());
            // Creates the playlist/library windows, restores persisted
            // geometry and registers the queue-event relays.
            app.state::<windows::WindowManager>().init(app.handle());
            // Native Window-menu toggles; after the manager so the initial
            // checkmarks match the restored visibility.
            if let Err(e) = menu::init(app.handle()) {
                eprintln!("navinamp: cannot install menu: {e}");
            }
            // System-wide "raise now playing" shortcut. The player window
            // owns the notification, so only the event crosses the bridge.
            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
                if let Err(e) =
                    app.global_shortcut()
                        .on_shortcut(NOTIFY_SHORTCUT, |app, _shortcut, event| {
                            if event.state == ShortcutState::Pressed {
                                let _ = app.emit_to("main", "notify-now-request", ());
                            }
                        })
                {
                    eprintln!(
                        "navinamp: cannot register notification shortcut {NOTIFY_SHORTCUT}: {e}"
                    );
                }
            }
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
            audio::audio_set_upcoming,
            audio::audio_pause,
            audio::audio_resume,
            audio::audio_stop,
            audio::audio_seek,
            audio::audio_set_volume,
            audio::audio_state,
            audio::audio_set_notify_track,
            audio::eq_state,
            audio::eq_headroom,
            ai::ai_get_config,
            ai::ai_set_config,
            ai::ai_test,
            ai::ai_models,
            ai::ai_search,
            ai::ai_offline_search,
            semantic::semantic_status,
            semantic::semantic_index,
            semantic::semantic_search,
            streams::streams_parse_url,
            streams::streams_ytdlp_status,
            streams::streams_set_ytdlp_path,
            streams::streams_resolve,
            audio::eq_set_enabled,
            audio::eq_set_preamp,
            audio::eq_set_band,
            audio::eq_presets,
            audio::eq_save_preset,
            audio::eq_delete_preset,
            audio::audio_spectrum,
            skin::skin_list,
            skin::skin_load,
            skin::popular_skins,
            skin::skin_install_popular,
            library::library_scan,
            library::library_search,
            library::library_albums,
            library::library_artists,
            library::library_genres,
            library::library_album_tracks,
            library::library_tracks,
            library::library_stats,
            library::library_roots,
            library::expand_audio_paths,
            library::library_mark_played,
            library::library_set_rating,
            library::library_track,
            library::art_for_track,
            library::repair_tag_texts,
            smart::smart_views_list,
            smart::smart_view_save,
            smart::smart_view_delete,
            smart::smart_view_run,
            smart::smart_view_preview,
            smart::smart_view_query,
            navidrome::navidrome_status,
            navidrome::navidrome_profiles,
            navidrome::navidrome_connect,
            navidrome::navidrome_disconnect,
            navidrome::navidrome_switch,
            navidrome::navidrome_remove_profile,
            navidrome::navidrome_rename_profile,
            navidrome::navidrome_sync,
            navidrome::navidrome_stream_url,
            navidrome::navidrome_art_url,
            navidrome::navidrome_scrobble,
            navidrome::navidrome_star,
            navidrome::navidrome_rate,
            navidrome::navidrome_libraries,
            navidrome::navidrome_set_library,
            navidrome::navidrome_playlists,
            navidrome::navidrome_playlist_tracks,
            navidrome::navidrome_publish_smart_view,
            navidrome::navidrome_publish_tracks,
            navidrome::navidrome_set_streaming,
            windows::windows_get,
            windows::window_set_visible,
            windows::window_toggle_layout,
            windows::window_set_size,
            windows::window_set_always_on_top,
            windows::window_drag_region_ready,
            media::media_metadata,
            media::media_playback,
            media::notify_track_started
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
