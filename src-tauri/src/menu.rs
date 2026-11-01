//! Native menu wiring: app-wide Window-menu toggles for the auxiliary
//! windows (Media Library / Equalizer / Playlist Editor).
//!
//! The app uses Tauri's stock menu ([`Menu::default`]) and prepends the
//! three toggles to its Window submenu, so File/Edit/View keep the standard
//! entries. Each item is checkable; [`sync_visibility`] keeps the
//! checkmarks in step no matter where visibility changed (menu click, skin
//! button, window IPC, or the titlebar close button routing into the
//! window manager).
//!
//! Accelerators are app-wide (`⌘⇧L` / `⌘⇧E` / `⌘⇧P` on macOS). The player
//! window additionally keeps its skin-independent `Alt+L` opener.

use tauri::menu::{CheckMenuItem, Menu, MenuItemKind, PredefinedMenuItem, WINDOW_SUBMENU_ID};
use tauri::{AppHandle, Manager, Wry};

use crate::windows::{WindowManager, WindowsInfo};

const MENU_LIBRARY: &str = "window_toggle_library";
const MENU_EQ: &str = "window_toggle_eq";
const MENU_PLAYLIST: &str = "window_toggle_playlist";

/// Handles for the three checkable Window-menu items.
pub struct WindowMenu {
    library: CheckMenuItem<Wry>,
    eq: CheckMenuItem<Wry>,
    playlist: CheckMenuItem<Wry>,
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
    let separator = PredefinedMenuItem::separator(app)?;
    window_menu.prepend_items(&[&library, &eq, &playlist, &separator])?;

    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
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
        assert_eq!(label_for("minimize"), None);
        assert_eq!(label_for(""), None);
    }
}
