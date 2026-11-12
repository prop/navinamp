//! Native menu wiring: app-wide Window-menu toggles for the auxiliary
//! windows (Media Library / Equalizer / Playlist Editor) plus playback
//! preferences ("Always on Top", "Notify on Track Change").
//!
//! The app uses Tauri's stock menu ([`Menu::default`]) and prepends the
//! toggles to its Window submenu, so File/Edit/View keep the standard
//! entries. Each item is checkable; [`sync_visibility`] keeps the
//! visibility checkmarks in step no matter where visibility changed (menu
//! click, skin button, window IPC, or the titlebar close button routing
//! into the window manager). The notification preference is owned by
//! `audio.json` ([`crate::audio`]) — the player window reads it from the
//! 10 Hz `audio_state` poll, so no extra sync path is needed.
//!
//! Accelerators are app-wide (`⌘⇧L` / `⌘⇧E` / `⌘⇧P` / `⌘⇧T` on macOS).
//! "Notify on Track Change" is menu-only on purpose: `⌘⇧N` is the
//! browser-incognito chord and would silently lose to the foreground app
//! for most users. The player window additionally keeps its
//! skin-independent `Alt+L` opener.

use tauri::menu::{CheckMenuItem, Menu, MenuItemKind, PredefinedMenuItem, WINDOW_SUBMENU_ID};
use tauri::{AppHandle, Manager, Wry};

use crate::audio;
use crate::windows::{WindowManager, WindowsInfo};

const MENU_LIBRARY: &str = "window_toggle_library";
const MENU_EQ: &str = "window_toggle_eq";
const MENU_PLAYLIST: &str = "window_toggle_playlist";
const MENU_ALWAYS_ON_TOP: &str = "window_always_on_top";
const MENU_NOTIFY_TRACK: &str = "playback_notify_track";
const MENU_RESUME_ON_LAUNCH: &str = "playback_resume_on_launch";

/// Handles for the checkable Window-menu items.
pub struct WindowMenu {
    library: CheckMenuItem<Wry>,
    eq: CheckMenuItem<Wry>,
    playlist: CheckMenuItem<Wry>,
    always_on_top: CheckMenuItem<Wry>,
    notify_track: CheckMenuItem<Wry>,
    resume_on_launch: CheckMenuItem<Wry>,
}

/// Installs the native menu. Must run after [`WindowManager::init`] so the
/// initial checkmarks reflect the persisted visibility.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::default(app)?;
    let Some(MenuItemKind::Submenu(window_menu)) = menu.get(WINDOW_SUBMENU_ID) else {
        // Platform without a Window submenu: leave the stock menu alone.
        return Ok(());
    };

    let info = app.state::<WindowManager>().info(app);
    let library = CheckMenuItem::with_id(
        app,
        MENU_LIBRARY,
        "Media Library",
        true,
        info.library.visible,
        Some("CmdOrCtrl+Shift+L"),
    )?;
    let eq = CheckMenuItem::with_id(
        app,
        MENU_EQ,
        "Equalizer",
        true,
        info.eq.visible,
        Some("CmdOrCtrl+Shift+E"),
    )?;
    let playlist = CheckMenuItem::with_id(
        app,
        MENU_PLAYLIST,
        "Playlist Editor",
        true,
        info.playlist.visible,
        Some("CmdOrCtrl+Shift+P"),
    )?;
    let always_on_top = CheckMenuItem::with_id(
        app,
        MENU_ALWAYS_ON_TOP,
        "Always on Top",
        true,
        app.state::<WindowManager>().all_always_on_top(),
        Some("CmdOrCtrl+Shift+T"),
    )?;
    let notify_track = CheckMenuItem::with_id(
        app,
        MENU_NOTIFY_TRACK,
        "Notify on Track Change",
        true,
        audio::notify_track(&app.state::<audio::Audio>()),
        None::<&str>,
    )?;
    let resume_on_launch = CheckMenuItem::with_id(
        app,
        MENU_RESUME_ON_LAUNCH,
        "Resume Playback on Launch",
        true,
        audio::resume_on_launch(&app.state::<audio::Audio>()),
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let separator2 = PredefinedMenuItem::separator(app)?;
    window_menu.prepend_items(&[
        &library,
        &eq,
        &playlist,
        &separator,
        &always_on_top,
        &separator2,
        &notify_track,
        &resume_on_launch,
    ])?;

    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id().as_ref() == MENU_ALWAYS_ON_TOP {
            let manager = app.state::<WindowManager>();
            let on = !manager.all_always_on_top();
            if let Err(e) = manager.set_all_always_on_top(app, on) {
                eprintln!("navinamp menu: always-on-top failed: {e}");
            }
            return;
        }
        if event.id().as_ref() == MENU_NOTIFY_TRACK {
            // Single source of truth is `audio.json`; the player picks the
            // new value up from its next `audio_state` poll (≤100 ms).
            let state = app.state::<audio::Audio>();
            let on = !audio::notify_track(&state);
            audio::set_notify_track(&state, on);
            if let Some(menu) = app.try_state::<WindowMenu>() {
                let _ = menu.notify_track.set_checked(on);
            }
            return;
        }
        if event.id().as_ref() == MENU_RESUME_ON_LAUNCH {
            // The player applies this on its next launch (state is polled;
            // toggling mid-session does not start playback by itself).
            let state = app.state::<audio::Audio>();
            let on = !audio::resume_on_launch(&state);
            audio::set_resume_on_launch(&state, on);
            if let Some(menu) = app.try_state::<WindowMenu>() {
                let _ = menu.resume_on_launch.set_checked(on);
            }
            return;
        }
        let Some(label) = label_for(event.id().as_ref()) else {
            return; // predefined items (minimize/zoom/close) handle themselves
        };
        let manager = app.state::<WindowManager>();
        let info = manager.info(app);
        let Some(visible) = visible_of(&info, label) else {
            return;
        };
        if let Err(e) = manager.set_visible(app, label, !visible) {
            eprintln!("navinamp menu: toggle '{label}' failed: {e}");
        }
    });
    app.manage(WindowMenu {
        library,
        eq,
        playlist,
        always_on_top,
        notify_track,
        resume_on_launch,
    });
    Ok(())
}

/// Keep one menu checkmark in step with a window-manager visibility change.
pub fn sync_visibility(app: &AppHandle, label: &str, visible: bool) {
    let Some(menu) = app.try_state::<WindowMenu>() else {
        return; // menu not installed yet (early startup)
    };
    let Some(item) = item_for(&menu, label) else {
        return; // "main" has no toggle (it is never hidden)
    };
    let _ = item.set_checked(visible);
}

/// Keep the "Always on Top" checkmark in step with the window manager.
pub fn sync_always_on_top(app: &AppHandle, on: bool) {
    let Some(menu) = app.try_state::<WindowMenu>() else {
        return;
    };
    let _ = menu.always_on_top.set_checked(on);
}

/// Menu id → window label (None for everything that is not ours).
fn label_for(id: &str) -> Option<&'static str> {
    match id {
        MENU_LIBRARY => Some("library"),
        MENU_EQ => Some("eq"),
        MENU_PLAYLIST => Some("playlist"),
        _ => None,
    }
}

/// The check item that belongs to a window label.
fn item_for<'a>(menu: &'a WindowMenu, label: &str) -> Option<&'a CheckMenuItem<Wry>> {
    match label {
        "library" => Some(&menu.library),
        "eq" => Some(&menu.eq),
        "playlist" => Some(&menu.playlist),
        _ => None,
    }
}

/// Current visibility for a menu-toggleable window label.
fn visible_of(info: &WindowsInfo, label: &str) -> Option<bool> {
    match label {
        "library" => Some(info.library.visible),
        "eq" => Some(info.eq.visible),
        "playlist" => Some(info.playlist.visible),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_their_windows() {
        assert_eq!(label_for(MENU_LIBRARY), Some("library"));
        assert_eq!(label_for(MENU_EQ), Some("eq"));
        assert_eq!(label_for(MENU_PLAYLIST), Some("playlist"));
        assert_eq!(label_for(MENU_NOTIFY_TRACK), None);
        assert_eq!(label_for("minimize"), None);
        assert_eq!(label_for(""), None);
    }
}
