//! OS media integration: native media keys and Now Playing metadata via
//! `souvlaki` (MPNowPlayingInfoCenter on macOS, SMTC on Windows, MPRIS on
//! Linux).
//!
//! The player window pushes state through two commands; hardware/OS
//! controls come back as the `media-command` event
//! (`{ command: "play" | "pause" | "stop" | "next" | "prev" }`), which the
//! player window treats like a skin action.
//!
//! ## Construction (lazy, never fatal)
//!
//! `MediaControls` is created on the first `media_*` command — souvlaki
//! can fail headless (no D-Bus, no window server), so a failure just logs
//! once and degrades every later push to a no-op. Commands always succeed.
//!
//! ## Platform caveats (souvlaki 0.9)
//!
//! - **Windows**: `PlatformConfig::hwnd` is *required* — souvlaki panics
//!   on `None` (`GetForWindow` needs an HWND). We pass the main (player)
//!   window's HWND; if it cannot be resolved we never construct the
//!   controls. Frameless windows are fine — any HWND works.
//! - **macOS**: no config needed; works without an open window but needs
//!   an application delegate/event loop, which Tauri provides.
//! - **Linux**: `dbus_name` + `display_name` configure the MPRIS identity.
//!   We build with souvlaki's `use_zbus` backend (pure Rust, no system
//!   libdbus needed at build time); the default `dbus-crossroads` backend
//!   is considered slightly more stable but requires the libdbus dev
//!   package on Linux builds.
//!
//! ## Update coarsening
//!
//! souvlaki sets metadata wholesale, so pushes are diffed: metadata only
//! goes out when title/artist/album/duration actually changed, playback
//! when the playing flag flipped or the position moved ≥ 1 s (while a
//! state is known). Frontends may therefore call `media_metadata` every
//! tick cheaply.

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use tauri::{AppHandle, Emitter, Manager, State};

/// Position pushes are coarsened to whole-ish seconds.
const POSITION_PUSH_SECS: f64 = 1.0;

/// MPRIS identity on Linux (D-Bus name rules: alnum + dots).
const DBUS_NAME: &str = "com.prop.navinamp";
const DISPLAY_NAME: &str = "NaVinaMP";

/// Managed state: the lazily-created OS handle plus the last snapshot.
#[derive(Default)]
pub struct MediaHub {
    inner: Mutex<MediaInner>,
}

#[derive(Default)]
struct MediaInner {
    controls: Option<MediaControls>,
    /// True once construction failed — never retry (a headless failure
    /// will not heal while the app runs).
    unavailable: bool,
    snap: Snapshot,
    pushed_meta: Option<Snapshot>,
    pushed_play: Option<Snapshot>,
}

/// Last-known player state pushed by the player window.
#[derive(Default, Clone, PartialEq, Debug)]
struct Snapshot {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration_secs: Option<f64>,
    position_secs: Option<f64>,
    playing: Option<bool>,
}

// -- pure decision logic ------------------------------------------------------

/// Whether OS metadata must be re-pushed (wholesale API — only push on
/// real changes; position is deliberately excluded, it travels via the
/// playback status).
fn metadata_dirty(old: Option<&Snapshot>, new: &Snapshot) -> bool {
    let Some(old) = old else { return true };
    old.title != new.title
        || old.artist != new.artist
        || old.album != new.album
        || old.duration_secs != new.duration_secs
}

/// Whether the OS playback status must be re-pushed: the playing flag
/// changed (or became known), or the position advanced by at least
/// [`POSITION_PUSH_SECS`].
fn playback_dirty(old: Option<&Snapshot>, new: &Snapshot) -> bool {
    let Some(old) = old else {
        return new.playing.is_some();
    };
    if old.playing != new.playing {
        return new.playing.is_some();
    }
    match (old.position_secs, new.position_secs) {
        (Some(a), Some(b)) => (b - a).abs() >= POSITION_PUSH_SECS,
        (None, Some(_)) => true,
        _ => false,
    }
}

/// Map an OS media event to a contract command. `Toggle` (the play/pause
/// key on many keyboards) resolves against the last-known playing state;
/// everything else the player cannot express yet (seek, volume, …) is
/// dropped — see the report notes.
fn map_event(event: &MediaControlEvent, playing: Option<bool>) -> Option<&'static str> {
    match event {
        MediaControlEvent::Play => Some("play"),
        MediaControlEvent::Pause => Some("pause"),
        MediaControlEvent::Stop => Some("stop"),
        MediaControlEvent::Next => Some("next"),
        MediaControlEvent::Previous => Some("prev"),
        MediaControlEvent::Toggle => match playing {
            Some(true) => Some("pause"),
            _ => Some("play"),
        },
        _ => None,
    }
}

fn sane_secs(v: f64) -> Option<f64> {
    (v.is_finite() && v >= 0.0).then_some(v)
}

// -- OS glue -------------------------------------------------------------------

/// Ensure the controls exist (lazily); returns false when unavailable.
fn ensure_controls(app: &AppHandle, inner: &mut MediaInner) -> bool {
    if inner.controls.is_some() || inner.unavailable {
        return inner.controls.is_some();
    }
    let hwnd = main_hwnd(app);
    let config = PlatformConfig {
        dbus_name: DBUS_NAME,
        display_name: DISPLAY_NAME,
        hwnd,
    };
    match MediaControls::new(config) {
        Ok(mut controls) => {
            let app = app.clone();
            if let Err(e) = controls.attach(move |event| on_media_event(&app, event)) {
                eprintln!("navinamp media: cannot attach OS media controls: {e}");
                inner.unavailable = true;
                return false;
            }
            inner.controls = Some(controls);
            true
        }
        Err(e) => {
            eprintln!("navinamp media: OS media controls unavailable (degrading to no-op): {e}");
            inner.unavailable = true;
            false
        }
    }
}

/// souvlaki requires an HWND on Windows; other platforms take `None`.
#[cfg(target_os = "windows")]
fn main_hwnd(app: &AppHandle) -> Option<*mut c_void> {
    app.get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0)
}

#[cfg(not(target_os = "windows"))]
fn main_hwnd(_app: &AppHandle) -> Option<*mut c_void> {
    None
}

fn on_media_event(app: &AppHandle, event: MediaControlEvent) {
    let playing = app
        .state::<MediaHub>()
        .inner
        .lock()
        .map(|inner| inner.snap.playing)
        .unwrap_or(None);
    if let Some(command) = map_event(&event, playing) {
        let _ = app.emit("media-command", MediaCommandPayload { command });
    }
}

fn push_metadata(inner: &mut MediaInner) {
    let Some(controls) = inner.controls.as_mut() else {
        return;
    };
    let snap = &inner.snap;
    let metadata = MediaMetadata {
        title: snap.title.as_deref(),
        artist: snap.artist.as_deref(),
        album: snap.album.as_deref(),
        cover_url: None,
        duration: snap
            .duration_secs
            .and_then(sane_secs)
            .map(Duration::from_secs_f64),
    };
    if let Err(e) = controls.set_metadata(metadata) {
        eprintln!("navinamp media: set_metadata failed: {e}");
    }
}

fn push_playback(inner: &mut MediaInner) {
    let Some(controls) = inner.controls.as_mut() else {
        return;
    };
    let snap = &inner.snap;
    let Some(playing) = snap.playing else { return };
    let progress = snap
        .position_secs
        .and_then(sane_secs)
        .map(|p| MediaPosition(Duration::from_secs_f64(p)));
    let status = if playing {
        MediaPlayback::Playing { progress }
    } else {
        MediaPlayback::Paused { progress }
    };
    if let Err(e) = controls.set_playback(status) {
        eprintln!("navinamp media: set_playback failed: {e}");
    }
}

// -- IPC commands ---------------------------------------------------------------

#[derive(Serialize, Clone)]
struct MediaCommandPayload<'a> {
    command: &'a str,
}

/// Push track metadata + position from the player window. Position updates
/// are coarsened; see module docs.
#[tauri::command]
pub fn media_metadata(
    app: AppHandle,
    state: State<MediaHub>,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration_secs: Option<f64>,
    position_secs: Option<f64>,
) {
    let mut inner = state.inner.lock().unwrap_or_else(|e| e.into_inner());
    inner.snap.title = title;
    inner.snap.artist = artist;
    inner.snap.album = album;
    inner.snap.duration_secs = duration_secs.and_then(sane_secs);
    inner.snap.position_secs = position_secs.and_then(sane_secs);

    if !ensure_controls(&app, &mut inner) {
        return;
    }
    if metadata_dirty(inner.pushed_meta.as_ref(), &inner.snap) {
        push_metadata(&mut inner);
        inner.pushed_meta = Some(inner.snap.clone());
    }
    if playback_dirty(inner.pushed_play.as_ref(), &inner.snap) {
        push_playback(&mut inner);
        inner.pushed_play = Some(inner.snap.clone());
    }
}

/// Push the playing flag from the player window.
#[tauri::command]
pub fn media_playback(app: AppHandle, state: State<MediaHub>, playing: bool) {
    let mut inner = state.inner.lock().unwrap_or_else(|e| e.into_inner());
    inner.snap.playing = Some(playing);

    if !ensure_controls(&app, &mut inner) {
        return;
    }
    if playback_dirty(inner.pushed_play.as_ref(), &inner.snap) {
        push_playback(&mut inner);
        inner.pushed_play = Some(inner.snap.clone());
    }
}

// -- tests ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(playing: Option<bool>, position: Option<f64>) -> Snapshot {
        Snapshot {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            duration_secs: Some(180.0),
            position_secs: position,
            playing,
        }
    }

    #[test]
    fn metadata_dirty_detects_field_changes() {
        let old = snap(None, None);
        assert!(!metadata_dirty(Some(&old), &old), "identical -> no push");

        let mut new = old.clone();
        new.title = Some("Other".into());
        assert!(metadata_dirty(Some(&old), &new));

        let mut new = old.clone();
        new.duration_secs = Some(181.0);
        assert!(metadata_dirty(Some(&old), &new));

        let mut new = old.clone();
        new.position_secs = Some(99.0);
        assert!(
            !metadata_dirty(Some(&old), &new),
            "position is not part of metadata"
        );

        assert!(metadata_dirty(None, &old), "first push always goes out");
    }

    #[test]
    fn playback_dirty_detects_flag_and_coarse_position() {
        let stopped = snap(None, Some(0.0));
        assert!(!playback_dirty(Some(&stopped), &stopped));
        assert!(
            !playback_dirty(Some(&stopped), &snap(None, Some(0.4))),
            "sub-second position"
        );
        assert!(
            playback_dirty(Some(&stopped), &snap(None, Some(1.4))),
            "≥1s position"
        );

        let paused = snap(Some(false), Some(0.0));
        let playing = snap(Some(true), Some(0.0));
        assert!(playback_dirty(Some(&paused), &playing), "flag flip");
        assert!(playback_dirty(None, &playing), "first known state");

        let unknown = snap(None, None);
        assert!(
            !playback_dirty(Some(&playing), &unknown),
            "playing unknown -> no push"
        );
    }

    #[test]
    fn map_event_covers_contract_commands() {
        assert_eq!(map_event(&MediaControlEvent::Play, None), Some("play"));
        assert_eq!(map_event(&MediaControlEvent::Pause, None), Some("pause"));
        assert_eq!(map_event(&MediaControlEvent::Stop, None), Some("stop"));
        assert_eq!(map_event(&MediaControlEvent::Next, None), Some("next"));
        assert_eq!(map_event(&MediaControlEvent::Previous, None), Some("prev"));
    }

    #[test]
    fn map_event_toggle_resolves_against_playing_state() {
        assert_eq!(
            map_event(&MediaControlEvent::Toggle, Some(true)),
            Some("pause")
        );
        assert_eq!(
            map_event(&MediaControlEvent::Toggle, Some(false)),
            Some("play")
        );
        assert_eq!(map_event(&MediaControlEvent::Toggle, None), Some("play"));
    }

    #[test]
    fn map_event_drops_unmapped_events() {
        assert_eq!(
            map_event(
                &MediaControlEvent::Seek(souvlaki::SeekDirection::Forward),
                None
            ),
            None
        );
        assert_eq!(
            map_event(
                &MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(3))),
                None
            ),
            None
        );
    }

    #[test]
    fn sane_secs_rejects_garbage() {
        assert_eq!(sane_secs(0.0), Some(0.0));
        assert_eq!(sane_secs(-1.0), None);
        assert_eq!(sane_secs(f64::NAN), None);
        assert_eq!(sane_secs(f64::INFINITY), None);
    }
}
