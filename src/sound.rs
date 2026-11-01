//! Shutter sound: best-effort playback of the system screenshot camera
//! shutter when a capture completes (spec: capture-output).

use std::cell::RefCell;
use std::path::Path;

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::NSSound;
use objc2_core_foundation::{CFBoolean, CFPreferencesCopyAppValue, CFString};
use objc2_foundation::NSString;

/// System files holding the screenshot shutter sound, best first. The
/// location moved across macOS releases: macOS 26+ ships the UI sounds
/// inside the CoreAudio component, older releases kept the shutter in
/// ScreenCapture.framework. A release with neither path stays silent.
fn shutter_sound_paths() -> Vec<&'static str> {
    vec![
        "/System/Library/Components/CoreAudio.component/Contents/SharedSupport/SystemSounds/system/Shutter.aif",
        "/System/Library/PrivateFrameworks/ScreenCapture.framework/Versions/A/Resources/camera-shutter.aiff",
    ]
}

/// The built-in screenshot utility silences its shutter when `disable-sound`
/// is true in the `com.apple.screencapture` domain; minisnap follows the
/// same switch. An unreadable or absent key means the sound is on.
fn sound_disabled_by_system() -> bool {
    let key = CFString::from_str("disable-sound");
    let domain = CFString::from_str("com.apple.screencapture");
    let Some(value) = CFPreferencesCopyAppValue(&key, &domain) else {
        return false;
    };
    value
        .downcast::<CFBoolean>()
        .ok()
        .is_some_and(|flag| flag.as_bool())
}

// NSSound goes silent when the object dies, so the player must outlive the
// call. One cached instance serves every shot: `play` restarts from the
// beginning, which is what rapid successive shots want.
thread_local! {
    static PLAYER: RefCell<Option<Retained<NSSound>>> = const { RefCell::new(None) };
}

/// Play the system shutter sound once. Best-effort: a missing file, a load
/// failure or a failed start only logs; the capture result is never affected.
pub fn play_shutter() {
    if MainThreadMarker::new().is_none() || sound_disabled_by_system() {
        return;
    }
    play_shutter_from(&shutter_sound_paths());
}

/// Playback over a given chain of candidates — split out so tests can
/// exercise the degradation paths without touching NSSound.
fn play_shutter_from(paths: &[&'static str]) {
    let Some(path) = paths.iter().find(|p| Path::new(p).exists()) else {
        return;
    };
    PLAYER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let Some(sound) = NSSound::initWithContentsOfFile_byReference(
                NSSound::alloc(),
                &NSString::from_str(path),
                true,
            ) else {
                eprintln!("sound: failed to load {path}");
                return;
            };
            *slot = Some(sound);
        }
        if !slot.as_ref().unwrap().play() {
            eprintln!("sound: play failed for {path}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_orders_macos26_shutter_first() {
        let paths = shutter_sound_paths();
        assert!(paths[0].ends_with("SystemSounds/system/Shutter.aif"));
        assert!(paths[1].ends_with("camera-shutter.aiff"));
    }

    #[test]
    fn empty_chain_is_silent() {
        play_shutter_from(&[]);
    }

    #[test]
    fn missing_files_are_silent() {
        play_shutter_from(&["/nonexistent/missing.aif", "/nonexistent/missing.aiff"]);
    }

    #[test]
    fn system_switch_reads_without_panic() {
        // The key is absent on a default system; the read must simply answer.
        let _ = sound_disabled_by_system();
    }

    #[test]
    fn disable_sound_key_is_honored_and_restored() {
        use objc2_core_foundation::{
            CFPreferencesAppSynchronize, CFPreferencesSetAppValue, CFRetained, CFPropertyList, Type,
        };

        let key = CFString::from_str("disable-sound");
        let domain = CFString::from_str("com.apple.screencapture");
        // SAFETY: CFBoolean is a legal CFPropertyList type, the unchecked cast
        // only relabels the same object.
        let flag: CFRetained<CFPropertyList> =
            unsafe { CFRetained::cast_unchecked(CFBoolean::new(true).retain()) };
        unsafe { CFPreferencesSetAppValue(&key, Some(&flag), &domain) };
        CFPreferencesAppSynchronize(&domain);
        assert!(sound_disabled_by_system());

        // Restore the default system state: the key absent again.
        unsafe { CFPreferencesSetAppValue(&key, None, &domain) };
        CFPreferencesAppSynchronize(&domain);
        assert!(!sound_disabled_by_system());
    }
}
