//! macOS default-output-device changes feed the engine's device-change
//! stamp even while playback is suspended.
//!
//! #122 drops a paused-state OS `play` that lands near a device change:
//! macOS asks media apps to resume when the default output device changes
//! (the well-known Bluetooth-disconnect resume). That guard reads
//! [`crate::audio::last_output_device_change_ms`], which until now was only
//! stamped by the cpal stream's error callback.
//!
//! #131 suspends the output stream whenever playback is not live — and a
//! suspended CoreAudio unit does not render, so on a paused player the
//! stream never errors: the stamp went stale, the guard saw no device
//! change, and the OS `play` started playback. CoreAudio's
//! `kAudioHardwarePropertyDefaultOutputDevice` property always fires,
//! whatever our own stream is doing, so listening on it restores the
//! guard's invariant in every playback state.
//!
//! macOS-only by nature: it exists for macOS's auto-resume behaviour.

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2_core_audio::{
        AudioObjectAddPropertyListener, AudioObjectID, AudioObjectPropertyAddress,
        kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
        kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    };
    use tauri::{AppHandle, Manager};

    use crate::audio;

    /// CoreAudio calls this on its own thread whenever the default output
    /// device changes. Only the engine stamp is written (one atomic store);
    /// no audio work happens here.
    unsafe extern "C-unwind" fn on_default_output_device_changed(
        _object: AudioObjectID,
        _n_addresses: u32,
        _addresses: NonNull<AudioObjectPropertyAddress>,
        client_data: *mut c_void,
    ) -> i32 {
        // SAFETY: `client_data` is the leaked AppHandle from `install`; it
        // lives for the process lifetime and AppHandle is Send + Sync.
        let app = unsafe { &*(client_data as *const AppHandle) };
        eprintln!("navinamp device-watch: default output device changed");
        audio::note_output_device_change(&app.state::<audio::Audio>());
        0
    }

    /// Register the listener for the app's lifetime (process exit unregisters
    /// everything). Call after `audio::init` so the state exists.
    pub fn install(app: &AppHandle) {
        // Leak one handle for the callback: it must outlive every device
        // change, and freeing it from the audio thread would need a
        // synchronization dance this doesn't warrant.
        let handle = Box::into_raw(Box::new(app.clone()));

        // CoreAudio copies the address on registration, so the stack copy is
        // fine.
        let mut address = AudioObjectPropertyAddress {
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };
        let status = unsafe {
            AudioObjectAddPropertyListener(
                kAudioObjectSystemObject as AudioObjectID,
                NonNull::from(&mut address),
                Some(on_default_output_device_changed),
                handle.cast::<c_void>(),
            )
        };
        if status != 0 {
            eprintln!("navinamp: cannot watch default output device changes (OSStatus {status})");
            // Registration failed: reclaim the leaked handle instead of
            // keeping a dead clone for the process lifetime.
            // SAFETY: `handle` came from `Box::into_raw` above and CoreAudio
            // never took ownership (the call failed).
            drop(unsafe { Box::from_raw(handle) });
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::install;
