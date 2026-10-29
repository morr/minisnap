//! Screen Recording (TCC) permission checks and requests.
//!
//! Screen Recording is the only permission the app itself needs:
//! ScreenCaptureKit refuses to return content without it (error -3801).
//! Accessibility is not used by the app at all; it is only requested for
//! automated checks that synthesize keystrokes from a terminal.

use std::ffi::c_void;

use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFMutableDictionary, CFRetained, CFString,
    kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks,
};

extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

extern "C-unwind" {
    fn AXIsProcessTrustedWithOptions(options: *const CFDictionary) -> u8;
}

/// Cheap non-prompting check: has Screen Recording been granted?
#[allow(dead_code)] // wired into the startup check in section 7
pub fn preflight_screen_capture() -> bool {
    // SAFETY: plain C function, no arguments.
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Ask the system for Screen Recording. Triggers the TCC prompt (once) and
/// opens the System Settings pane; returns whether it is already granted.
pub fn request_screen_capture() -> bool {
    // SAFETY: plain C function, no arguments.
    unsafe { CGRequestScreenCaptureAccess() }
}

/// Check (and optionally prompt for) Accessibility trust of this process.
pub fn is_accessibility_trusted(prompt: bool) -> bool {
    // SAFETY: standard kCFType callbacks on a fresh dictionary.
    let dict = unsafe {
        CFMutableDictionary::new(
            None,
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    }
    .expect("options dictionary");
    let key = CFString::from_str("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::new(prompt);
    // SAFETY: both key and value are valid CFType objects.
    unsafe {
        CFMutableDictionary::set_value(
            Some(&dict),
            CFRetained::as_ptr(&key).as_ptr().cast::<c_void>(),
            std::ptr::from_ref(value).cast::<c_void>(),
        );
        AXIsProcessTrustedWithOptions(
            std::ptr::from_ref(&*dict).cast::<CFDictionary>(),
        ) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "triggers a system permission prompt; run explicitly"]
    fn request_screen_recording() {
        let preflight = preflight_screen_capture();
        eprintln!("screen recording preflight: {preflight}");
        let granted = request_screen_capture();
        eprintln!("screen recording request returned: {granted}");
    }

    #[test]
    #[ignore = "triggers the accessibility prompt; needed only for automated hotkey checks"]
    fn request_accessibility() {
        let trusted = is_accessibility_trusted(true);
        eprintln!("accessibility trusted: {trusted}");
    }
}
