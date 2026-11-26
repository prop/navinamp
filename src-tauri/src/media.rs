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
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::audio;
use crate::library::LibraryState;
use crate::navidrome::NavidromeState;

/// Position pushes are coarsened to whole-ish seconds.
const POSITION_PUSH_SECS: f64 = 1.0;

/// `souvlaki`'s undetermined Seek events move by this many seconds.
const MEDIA_SEEK_STEP_SECS: f64 = 5.0;

/// Defence against a system-generated Play on a route change: a paused-state
/// `play` waits [`PLAY_DEFER`] and is dropped when a device change landed
/// within [`DEVICE_CHANGE_WINDOW`] of the command.
///
/// This is NOT what resumes a paused player on a Bluetooth disconnect — that
/// was the system's **Pause**, which the player's toggle-style PAUSE action
/// used to turn back into playback. That is fixed twice over: the player
/// window routes a transport `pause` to a plain (idempotent) pause, and
/// [`map_event`] drops a Pause for an already-paused player. A system Play near a route change has never been observed in a
/// captured log, so this guard is kept only as a bounded defence; every OS
/// event is logged in [`on_media_event`], which will show one if it exists.
const DEVICE_CHANGE_WINDOW: Duration = Duration::from_secs(2);
/// A paused-state play inside this window after a device change is treated as
/// a possible *late* system resume — but only the first one per change (see
/// [`paused_play_decision`]). The 15.9 s play that motivated this window
/// turned out to be indistinguishable from a user press: the confirmed
/// device-change resume was the Pause toggle, not a Play. The latch is kept
/// as a bounded defence, and bounds its own cost to a single ignored press —
/// a genuine press shortly after plugging headphones in still works on the
/// second attempt.
const DEVICE_CHANGE_QUIET: Duration = Duration::from_secs(60);
/// Imperceptible delay that lets a device-change event overtake the command.
const PLAY_DEFER: Duration = Duration::from_millis(250);

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
    /// Deferred paused-state play commands (see [`defer_paused_play`]); the
    /// worker is started on first use.
    defer_tx: Option<mpsc::Sender<u64>>,
    /// Device-change stamp that already swallowed one *late* paused play
    /// (see [`is_late_device_change_resume`]); a new change restarts the
    /// one-shot.
    quiet_suppressed_for: Option<u64>,
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
        // Idempotent: a Pause for an already-paused player is dropped.
        // "Pause, the output device went away" is exactly what macOS sends
        // when headphones disconnect, and the player's PAUSE *skin action*
        // is a toggle (the classic button) — routing a system Pause through
        // it resumed a player the user had already paused. The player window
        // now routes a transport `pause` to a plain pause instead, so this
        // rule is a second, independent line of defence rather than the
        // only thing standing between a redundant Pause and a resume.
        MediaControlEvent::Pause => (playing != Some(false)).then(|| command("pause")),
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Whether a paused-state Play is the system's device-change resume rather
/// than a user command.
///
/// Suppress whenever the route change is within [`DEVICE_CHANGE_WINDOW`] of
/// the command, in either order. The window must be symmetric: the
/// CoreAudio property event fires at the *start* of a route change, while
/// the OS media Play can trail it by well over [`PLAY_DEFER`] (Bluetooth
/// teardown latency) — a one-sided window let exactly those resumes through.
///
/// The cost is that a genuine press within two seconds of a device change is
/// dropped; the user can simply press again. The window intentionally
/// accepts that a *small* forward wall-clock step (≤ the window) can also
/// swallow one paused press — the alternative (a one-sided upper guard)
/// re-opened the resume-on-device-change hole this window exists to close.
/// Changes beyond the window in either direction never suppress.
fn is_device_change_resume(command_ms: u64, changed_ms: Option<u64>) -> bool {
    let window_ms = DEVICE_CHANGE_WINDOW.as_millis() as u64;
    changed_ms.is_some_and(|changed| changed.abs_diff(command_ms) <= window_ms)
}

/// What the device-change guard does with one deferred paused-state play.
/// The worker emits **only** for [`PausedPlay::Forward`] — the decision is
/// the emit gate, not merely a log label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PausedPlay {
    /// Inside the certain symmetric window: definitely the system resume.
    SuppressCertain,
    /// The first play of a route-change *episode* inside the quiet window —
    /// a possible late system resume (never observed; see the module docs).
    SuppressLate { change: u64 },
    /// A genuine press: forward to the player.
    Forward,
}

/// Decide one deferred paused-state play.
///
/// `consumed` is the change stamp whose quiet-window one-shot has already
/// fired. CoreAudio reports one physical route change through two paths (the
/// property listener and the stream error callback), producing stamps a few
/// milliseconds apart — a stamp within [`PLAY_DEFER`] of `consumed` is the
/// same episode and must not re-arm the one-shot, or a second genuine press
/// would be swallowed for one physical change.
fn paused_play_decision(
    command_ms: u64,
    changed_ms: Option<u64>,
    consumed: Option<u64>,
) -> PausedPlay {
    if is_device_change_resume(command_ms, changed_ms) {
        return PausedPlay::SuppressCertain;
    }
    let quiet_ms = DEVICE_CHANGE_QUIET.as_millis() as u64;
    let episode_ms = PLAY_DEFER.as_millis() as u64;
    match changed_ms {
        Some(changed) => {
            let consumed_this_episode =
                consumed.is_some_and(|done| done.abs_diff(changed) <= episode_ms);
            if !consumed_this_episode && changed <= command_ms && command_ms - changed <= quiet_ms {
                PausedPlay::SuppressLate { change: changed }
            } else {
                PausedPlay::Forward
            }
        }
        None => PausedPlay::Forward,
    }
}

fn emit_media_command(app: &AppHandle, mapped: MappedCommand) {
    let _ = app.emit(
        "media-command",
        MediaCommandPayload {
            command: mapped.command,
            secs: mapped.secs,
        },
    );
}

/// Queue a paused-state Play for the device-change check. A single worker
/// serializes them: no thread per press, and broadcasts keep their order.
fn defer_paused_play(app: &AppHandle, command_ms: u64) {
    let hub = app.state::<MediaHub>();
    let mut inner = hub.inner.lock().unwrap();
    if inner.defer_tx.is_none() {
        let (tx, rx) = mpsc::channel();
        inner.defer_tx = Some(tx);
        spawn_deferred_play_worker(app.clone(), rx);
    }
    if let Some(tx) = inner.defer_tx.as_ref() {
        let _ = tx.send(command_ms);
    }
}

/// Waits out [`PLAY_DEFER`] once per burst, then emits every queued Play that
/// is not a device-change resume (in arrival order).
fn spawn_deferred_play_worker(app: AppHandle, rx: mpsc::Receiver<u64>) {
    let _ = std::thread::Builder::new()
        .name("navinamp-media-defer".into())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut batch = vec![first];
                let deadline = Instant::now() + PLAY_DEFER;
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match rx.recv_timeout(remaining) {
                        Ok(command_ms) => batch.push(command_ms),
                        Err(_) => break, // timeout or sender gone
                    }
                }
                let hub = app.state::<MediaHub>();
                let mut quiet_consumed = hub
                    .inner
                    .lock()
                    .ok()
                    .and_then(|inner| inner.quiet_suppressed_for);
                // One emit per queued press: duplicates are harmless (PLAY is
                // a no-op once playing) and collapsing them would discard the
                // ordering guarantee this worker exists for.
                for command_ms in batch {
                    // Fresh read per command: the quiet window's premise is
                    // that the stamp can be seconds old, so a batch must not
                    // pin one stale read for all of its commands.
                    let changed = audio::last_output_device_change_ms(&app.state::<audio::Audio>());
                    let decision = paused_play_decision(command_ms, changed, quiet_consumed);
                    let suppressed = decision != PausedPlay::Forward;
                    let late = matches!(decision, PausedPlay::SuppressLate { .. });
                    if let PausedPlay::SuppressLate { change } = decision {
                        quiet_consumed = Some(change);
                        if let Ok(mut inner) = hub.inner.lock() {
                            inner.quiet_suppressed_for = Some(change);
                        }
                    }
                    // The whole guard is invisible otherwise: log the decision
                    // so a resumed-on-device-change report can be diagnosed
                    // from stderr (Finder launch: `open --stderr …`).
                    match changed {
                        Some(changed_ms) => {
                            let delta = command_ms.abs_diff(changed_ms);
                            let side = if changed_ms <= command_ms {
                                "before"
                            } else {
                                "after"
                            };
                            eprintln!(
                                "navinamp media: paused play {} — device change {delta} ms {side} the command{}",
                                if suppressed {
                                    "suppressed"
                                } else {
                                    "forwarded"
                                },
                                if late {
                                    " (first play in the quiet window: system resume)"
                                } else {
                                    ""
                                },
                            )
                        }
                        None => eprintln!(
                            "navinamp media: paused play forwarded — no device change recorded"
                        ),
                    }
                    if !suppressed {
                        emit_media_command(
                            &app,
                            MappedCommand {
                                command: "play",
                                secs: None,
                            },
                        );
                    }
                }
            }
        });
}

/// The state every OS media decision is made on: the engine's, never the
/// media hub's snapshot (which is only as fresh as the renderer's last
/// transition push). With no engine yet nothing can be playing, so that is
/// `Some(false)` — there is deliberately no fallback to the stale snapshot.
fn engine_state(engine: Option<bool>) -> Option<bool> {
    Some(engine.unwrap_or(false))
}

fn on_media_event(app: &AppHandle, event: MediaControlEvent) {
    let playing = engine_state(audio::engine_playing(&app.state::<audio::Audio>()));
    let Some(mapped) = map_event(&event, playing) else {
        // Every OS media event is logged: the device-change resume hid for
        // three fixes behind a command this function forwarded silently.
        eprintln!("navinamp media: OS event {event:?} dropped — playing {playing:?}");
        return;
    };
    eprintln!(
        "navinamp media: OS event {event:?} → {} — playing {playing:?}",
        mapped.command
    );
    if mapped.command == "play" && playing == Some(false) {
        // Paused: a Play here may be the system's own device-change resume
        // rather than a press. The worker gives the device event a moment to
        // arrive, then drops the command when one did.
        defer_paused_play(app, now_ms());
        return;
    }
    emit_media_command(app, mapped);
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
    fn pause_for_an_already_paused_player_is_dropped() {
        // The player's PAUSE action is a toggle, so a redundant system Pause
        // would resume a paused player. macOS sends exactly that Pause when
        // an output device disappears (headphones disconnecting).
        assert_eq!(
            map_event(&MediaControlEvent::Pause, Some(false)),
            None,
            "already paused: must not toggle back to playing"
        );
        // While playing the Pause is forwarded (it pauses, as intended).
        assert_eq!(
            map_event(&MediaControlEvent::Pause, Some(true)),
            mapped("pause", None)
        );
        // Unknown state: keep the previous behaviour (the player decides).
        assert_eq!(
            map_event(&MediaControlEvent::Pause, None),
            mapped("pause", None)
        );
        // Play is unaffected by the idempotency rule: a paused-state Play is
        // still forwarded to the device-change guard.
        assert_eq!(
            map_event(&MediaControlEvent::Play, Some(false)),
            mapped("play", None)
        );
    }

    #[test]
    fn no_engine_never_falls_back_to_a_stale_snapshot() {
        // Before any engine exists nothing can be playing: the decision
        // state is `Some(false)`, never the hub's last renderer push. A
        // Pause on that path is dropped, so even a stale `true` snapshot
        // cannot turn a redundant system Pause into a resume.
        assert_eq!(engine_state(None), Some(false));
        assert_eq!(engine_state(Some(true)), Some(true));
        assert_eq!(
            map_event(&MediaControlEvent::Pause, engine_state(None)),
            None
        );
        // A Play with no engine is treated as a paused-state play and goes
        // to the device-change guard like any other.
        assert_eq!(
            map_event(&MediaControlEvent::Play, engine_state(None)),
            mapped("play", None)
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

    #[test]
    fn device_change_resume_window_is_symmetric() {
        let window_ms = DEVICE_CHANGE_WINDOW.as_millis() as u64;
        // Through the window on both sides of the command.
        assert!(is_device_change_resume(10_000, Some(10_200)));
        assert!(is_device_change_resume(10_000, Some(9_800)));
        assert!(is_device_change_resume(
            10_000,
            Some(10_000 - window_ms + 1)
        ));
        assert!(is_device_change_resume(10_000, Some(10_000 + window_ms)));
        // The regression this pins: the CoreAudio property event can precede
        // the OS media Play by more than PLAY_DEFER (Bluetooth teardown), and
        // must still be suppressed.
        assert!(is_device_change_resume(10_000, Some(9_500)));
        // Outside the window: a genuine user command.
        assert!(!is_device_change_resume(
            10_000,
            Some(10_000 - window_ms - 1)
        ));
        assert!(!is_device_change_resume(10_000, Some(7_900)));
        // A far-future clock step must not poison every later press (the
        // window is not open-ended; small forward steps are accepted).
        assert!(!is_device_change_resume(10_000, Some(10_000_000)));
        // No device change ever recorded.
        assert!(!is_device_change_resume(10_000, None));
    }

    #[test]
    fn quiet_window_swallows_only_the_first_late_play() {
        let change = 10_000u64;
        // The reported case: the system resume arrived 15.9 s after the
        // route change — suppressed, and the decision carries the stamp the
        // worker records in the hub.
        assert_eq!(
            paused_play_decision(25_900, Some(change), None),
            PausedPlay::SuppressLate { change }
        );
        // The one-shot: a second play within the same episode (the worker
        // stored the stamp) is forwarded — this is the arm the emit gate
        // reads, so a regression to "suppressed is just a log label" fails
        // here.
        assert_eq!(
            paused_play_decision(30_000, Some(change), Some(change)),
            PausedPlay::Forward
        );
        // The same physical route change reported twice (property listener +
        // stream error, stamps a few ms apart) is ONE episode: the second
        // stamp must not re-arm the one-shot.
        assert_eq!(
            paused_play_decision(
                30_000,
                Some(change + PLAY_DEFER.as_millis() as u64 - 1),
                Some(change)
            ),
            PausedPlay::Forward
        );
        // A genuinely newer route change (outside the episode tolerance)
        // gets its own one-shot.
        let later = change + 1_000;
        assert_eq!(
            paused_play_decision(later + 15_000, Some(later), Some(change)),
            PausedPlay::SuppressLate { change: later }
        );
        // Inside the certain window: suppressed regardless of the latch.
        assert_eq!(
            paused_play_decision(10_200, Some(change), Some(change)),
            PausedPlay::SuppressCertain
        );
        // Outside the quiet window: real presses are never swallowed.
        let quiet_ms = DEVICE_CHANGE_QUIET.as_millis() as u64;
        assert_eq!(
            paused_play_decision(change + quiet_ms + 1, Some(change), None),
            PausedPlay::Forward
        );
        // No recorded change, or a stamp far in the future (a clock step):
        // forwarded. (A near-future stamp is inside the certain window and
        // is suppressed by design.)
        assert_eq!(
            paused_play_decision(25_900, None, None),
            PausedPlay::Forward
        );
        assert_eq!(
            paused_play_decision(9_000, Some(10_000_000), None),
            PausedPlay::Forward
        );
    }
}
