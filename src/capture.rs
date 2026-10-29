//! Screen capture at moment T0: the display under the cursor, the frame
//! image and the visible window list.

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSScreen};

/// Find the screen that contains the mouse cursor; fall back to the main
/// screen, then to the first screen.
pub fn screen_under_cursor(mtm: MainThreadMarker) -> Option<Retained<NSScreen>> {
    let mouse = NSEvent::mouseLocation();
    let mx = mouse.x;
    let my = mouse.y;
    for screen in NSScreen::screens(mtm).iter() {
        let frame = screen.frame();
        let in_x = mx >= frame.origin.x && mx < frame.origin.x + frame.size.width;
        let in_y = my >= frame.origin.y && my < frame.origin.y + frame.size.height;
        if in_x && in_y {
            return Some(screen);
        }
    }
    NSScreen::mainScreen(mtm).or_else(|| NSScreen::screens(mtm).iter().next())
}

/// Height of the primary screen (origin at (0,0)) in points. ScreenCaptureKit
/// reports coordinates in the global top-left-origin space whose Y axis is
/// flipped relative to AppKit's; the primary screen height is the constant
/// of that flip.
pub fn primary_screen_height(mtm: MainThreadMarker) -> f64 {
    let screens = NSScreen::screens(mtm);
    let mut fallback = 0.0f64;
    for screen in screens.iter() {
        let frame = screen.frame();
        if frame.origin.x == 0.0 && frame.origin.y == 0.0 {
            return frame.size.height;
        }
        fallback = fallback.max(frame.size.height);
    }
    fallback
}
