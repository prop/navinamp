//! OS media integration: native media keys and Now Playing metadata via
//! `souvlaki` (MPNowPlayingInfoCenter on macOS, SMTC on Windows, MPRIS on
//! Linux).
//!
//! The player window pushes state through two commands; hardware/OS
//! controls come back as the `media-command` event
//! (`{ command: "play" | "pause" | "stop" | "next" | "prev" }`, plus
//! `seekBy`/`seekTo` with a `secs` field for OS seek events), which the
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
    SeekDirection,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::library::LibraryState;
use crate::navidrome::NavidromeState;

/// Position pushes are coarsened to whole-ish seconds.
const POSITION_PUSH_SECS: f64 = 1.0;

/// `souvlaki`'s undetermined Seek events move by this many seconds.
const MEDIA_SEEK_STEP_SECS: f64 = 5.0;

/// A media event mapped to the frontend `media-command` contract.
#[derive(Debug, PartialEq)]
struct MappedCommand {
    command: &'static str,
    /// Present only for `seekBy` (signed delta) / `seekTo` (absolute).
    secs: Option<f64>,
}

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

/// Transport command for "next track", shared by the OS media-key mapping
/// and the global next-track shortcut so the two emit sites cannot drift.
pub(crate) const CMD_NEXT: &str = "next";

/// Map an OS media event to a contract command. `Toggle` (the play/pause
/// key on many keyboards) resolves against the last-known playing state.
/// Seek / SeekBy / SetPosition become relative or absolute seeks; events
/// the player cannot express (volume, shuffle, open-uri, …) are dropped.
fn map_event(event: &MediaControlEvent, playing: Option<bool>) -> Option<MappedCommand> {
    let command = |command| MappedCommand {
        command,
        secs: None,
    };
    let seek_by = |secs| MappedCommand {
        command: "seekBy",
        secs: Some(secs),
    };
    match event {
        MediaControlEvent::Play => Some(command("play")),
        MediaControlEvent::Pause => Some(command("pause")),
        MediaControlEvent::Stop => Some(command("stop")),
        MediaControlEvent::Next => Some(command(CMD_NEXT)),
        MediaControlEvent::Previous => Some(command("prev")),
        MediaControlEvent::Toggle => match playing {
            Some(true) => Some(command("pause")),
            _ => Some(command("play")),
        },
        MediaControlEvent::Seek(SeekDirection::Forward) => Some(seek_by(MEDIA_SEEK_STEP_SECS)),
        MediaControlEvent::Seek(SeekDirection::Backward) => Some(seek_by(-MEDIA_SEEK_STEP_SECS)),
        MediaControlEvent::SeekBy(SeekDirection::Forward, delta) => {
            Some(seek_by(delta.as_secs_f64()))
        }
        MediaControlEvent::SeekBy(SeekDirection::Backward, delta) => {
            Some(seek_by(-delta.as_secs_f64()))
        }
        MediaControlEvent::SetPosition(MediaPosition(position)) => Some(MappedCommand {
            command: "seekTo",
            secs: Some(position.as_secs_f64()),
        }),
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
    if let Some(mapped) = map_event(&event, playing) {
        let _ = app.emit(
            "media-command",
            MediaCommandPayload {
                command: mapped.command,
                secs: mapped.secs,
            },
        );
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
pub(crate) struct MediaCommandPayload {
    pub(crate) command: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) secs: Option<f64>,
}

/// Show a desktop "now playing" notification (issue #34), optionally with
/// the track's album art. Sent from the Rust side (notify-rust →
/// NSUserNotificationCenter on macOS — its default backend, which works
/// from unbundled `tauri dev` runs too, unlike UNUserNotificationCenter
/// which requires a bundled, authorized app) because the webview's Web
/// Notification API ignores images and cannot be granted permission in
/// WKWebView. NSUserNotificationCenter is deprecated but still delivers on
/// current macOS releases, including content images; the backend can be
/// switched to UNUserNotificationCenter via notify-rust's
/// `preview-macos-un` feature once notifications are only expected from
/// bundled builds. No authorization is requested or required on this
/// backend — and note the observable failure mode: while Do Not Disturb /
/// Focus is on, toasts are silently dropped.
///
/// Art resolution is backend-side (library art cache / Navidrome
/// getCoverArt — no webview fetch) and best-effort: failures notify
/// WITHOUT art rather than failing the notification. The Navidrome temp
/// file is intentionally left in place — Linux's D-Bus daemon reads it
/// lazily after `show()` returns, so deleting it immediately would race
/// the toast; `/tmp` is OS-cleaned. Local art points at the permanent
/// art-cache store and is never deleted.
#[tauri::command]
pub async fn notify_track_started(
    app: AppHandle,
    library: State<'_, LibraryState>,
    navidrome: State<'_, NavidromeState>,
    title: String,
    track_path: Option<String>,
) -> Result<(), String> {
    let (art_path, is_temp) = match track_path.as_deref() {
        Some(p) if p.starts_with("navidrome://") => {
            (crate::navidrome::notify_art_file(&navidrome, p).await, true)
        }
        Some(p) => (crate::library::notify_art_file(&app, &library, p), false),
        _ => (None, false),
    };
    let mut note = notify_rust::Notification::new();
    note.summary(&title);
    if let Some(path) = &art_path {
        note.image_path(path.to_string_lossy().as_ref());
    }
    note.show().map_err(|e| format!("notify: {e}"))?;
    // Only the fetched Navidrome cover (a temp file) is removed after
    // show(): macOS reads it eagerly so this is safe there, while Linux's
    // D-Bus daemon reads lazily and deleting would race the toast. Local
    // cache files are the permanent store and always stay.
    if is_temp {
        #[cfg(target_os = "macos")]
        if let Some(path) = art_path {
            std::fs::remove_file(path).ok();
        }
        #[cfg(not(target_os = "macos"))]
        drop(art_path);
    }
    Ok(())
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

    fn mapped(command: &'static str, secs: Option<f64>) -> Option<MappedCommand> {
        Some(MappedCommand { command, secs })
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
        assert_eq!(
            map_event(&MediaControlEvent::Play, None),
            mapped("play", None)
        );
        assert_eq!(
            map_event(&MediaControlEvent::Pause, None),
            mapped("pause", None)
        );
        assert_eq!(
            map_event(&MediaControlEvent::Stop, None),
            mapped("stop", None)
        );
        assert_eq!(
            map_event(&MediaControlEvent::Next, None),
            mapped("next", None)
        );
        assert_eq!(
            map_event(&MediaControlEvent::Previous, None),
            mapped("prev", None)
        );
    }

    #[test]
    fn map_event_toggle_resolves_against_playing_state() {
        assert_eq!(
            map_event(&MediaControlEvent::Toggle, Some(true)),
            mapped("pause", None)
        );
        assert_eq!(
            map_event(&MediaControlEvent::Toggle, Some(false)),
            mapped("play", None)
        );
        assert_eq!(
            map_event(&MediaControlEvent::Toggle, None),
            mapped("play", None)
        );
    }

    #[test]
    fn map_event_converts_os_seeks() {
        assert_eq!(
            map_event(&MediaControlEvent::Seek(SeekDirection::Forward), None),
            mapped("seekBy", Some(MEDIA_SEEK_STEP_SECS))
        );
        assert_eq!(
            map_event(&MediaControlEvent::Seek(SeekDirection::Backward), None),
            mapped("seekBy", Some(-MEDIA_SEEK_STEP_SECS))
        );
        assert_eq!(
            map_event(
                &MediaControlEvent::SeekBy(SeekDirection::Forward, Duration::from_millis(2500)),
                None
            ),
            mapped("seekBy", Some(2.5))
        );
        assert_eq!(
            map_event(
                &MediaControlEvent::SeekBy(SeekDirection::Backward, Duration::from_secs(10)),
                None
            ),
            mapped("seekBy", Some(-10.0))
        );
        assert_eq!(
            map_event(
                &MediaControlEvent::SetPosition(MediaPosition(Duration::from_secs(42))),
                None
            ),
            mapped("seekTo", Some(42.0))
        );
    }

    #[test]
    fn map_event_drops_unmapped_events() {
        assert_eq!(map_event(&MediaControlEvent::SetVolume(0.5), None), None);
        assert_eq!(map_event(&MediaControlEvent::SetShuffle(true), None), None);
        assert_eq!(
            map_event(
                &MediaControlEvent::OpenUri("https://example.test".into()),
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
