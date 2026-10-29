//! Screen capture at moment T0: the display under the cursor, the frame
//! image and the visible window list.

use std::sync::mpsc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSEvent, NSScreen};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{CGDisplayCopyDisplayMode, CGDisplayMode, CGImage};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
    SCCaptureResolutionType,
};

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

/// A visible regular window at the moment of capture (T0).
#[derive(Debug, Clone)]
pub struct T0Window {
    /// Frame in ScreenCaptureKit global points (top-left origin, front-to-
    /// back list order as reported by ScreenCaptureKit).
    // Consumed by the overlay's click-to-window hit-testing.
    #[allow(dead_code)]
    pub frame: CGRect,
    #[allow(dead_code)]
    pub window_id: u32,
    #[allow(dead_code)]
    pub title: Option<String>,
}

fn error_text(error: *mut NSError) -> String {
    if error.is_null() {
        return "unknown error".to_string();
    }
    // SAFETY: checked non-null above; the object is retained by the caller
    // (ScreenCaptureKit) for the duration of the handler call.
    let error = unsafe { &*error };
    format!("{} (code {})", error.localizedDescription(), error.code())
}

/// Fetch the shareable content (displays + windows) once; blocking on the
/// ScreenCaptureKit completion handler. Call from a background thread.
pub fn shareable_content() -> Result<Retained<SCShareableContent>, String> {
    let (tx, rx) = mpsc::channel::<Result<Retained<SCShareableContent>, String>>();
    let handler = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        let result = if !error.is_null() {
            Err(format!("shareable content: {}", error_text(error)))
        } else if content.is_null() {
            Err("shareable content: nothing returned".to_string())
        } else {
            // SAFETY: ScreenCaptureKit passes a +1 retained, non-null object
            // to the handler; take ownership of it.
            let retained = unsafe { Retained::retain(content) }
                .expect("handler received non-null shareable content");
            Ok(retained)
        };
        let _ = tx.send(result);
    });
    // SAFETY: plain class method with an owned handler block.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            false,
            true,
            &handler,
        );
    }
    rx.recv().map_err(|_| "shareable content: handler dropped".to_string())?
}

/// Display whose frame contains `mouse_sckit` (ScreenCaptureKit global
/// points); fallback — the first display (the main one).
pub fn pick_display(
    content: &SCShareableContent,
    mouse_sckit: CGPoint,
) -> Option<Retained<SCDisplay>> {
    let mut first: Option<Retained<SCDisplay>> = None;
    for display in unsafe { content.displays() }.iter() {
        // SAFETY: plain property access on a live SCDisplay object.
        let frame = unsafe { display.frame() };
        let inside = mouse_sckit.x >= frame.origin.x
            && mouse_sckit.x < frame.origin.x + frame.size.width
            && mouse_sckit.y >= frame.origin.y
            && mouse_sckit.y < frame.origin.y + frame.size.height;
        if inside {
            return Some(display);
        }
        if first.is_none() {
            first = Some(display);
        }
    }
    first
}

/// Regular visible windows of other apps, front-to-back: layer 0 only, our
/// own process excluded. Used for click-to-window hit-testing at T0.
pub fn visible_windows(content: &SCShareableContent) -> Vec<T0Window> {
    let own_pid = std::process::id() as i32;
    let mut windows = Vec::new();
    for window in unsafe { content.windows() }.iter() {
        // SAFETY: plain property access on live SCWindow/SCRunningApplication
        // objects returned by this shareable content.
        let (layer, on_screen, app, frame, window_id, title) = unsafe {
            (
                window.windowLayer(),
                window.isOnScreen(),
                window.owningApplication(),
                window.frame(),
                window.windowID(),
                window.title(),
            )
        };
        if layer != 0 || !on_screen {
            continue;
        }
        let Some(app) = app else {
            continue;
        };
        // SAFETY: plain property access on a live SCRunningApplication object.
        if unsafe { app.processID() } == own_pid {
            continue;
        }
        windows.push(T0Window {
            frame,
            window_id,
            title: title.map(|t| t.to_string()),
        });
    }
    windows
}

/// Everything captured at the moment the mode starts (T0).
pub struct T0Capture {
    /// Full display frame in display pixels (top-left origin image space).
    pub image: Retained<CGImage>,
    /// Visible regular windows, front-to-back.
    // Consumed by the overlay's click-to-window hit-testing.
    #[allow(dead_code)]
    pub windows: Vec<T0Window>,
    /// Captured display's frame in ScreenCaptureKit global points.
    pub display_frame: CGRect,
}

/// One-shot screenshot of a whole display at its full pixel resolution
/// (from the display mode), blocking until the image arrives. Call from a
/// background thread.
pub fn capture_display_image(display: &SCDisplay) -> Result<Retained<CGImage>, String> {
    let empty = NSArray::new();
    // SAFETY: plain init on a freshly allocated filter; empty window list.
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            display,
            &empty,
        )
    };
    // SAFETY: plain constructor.
    let config = unsafe { SCStreamConfiguration::new() };
    // SCDisplay.width reports the scaled (point) size on current macOS, so
    // the capture resolution comes from the display mode's true pixel size.
    // SAFETY: plain C queries on a live display id.
    let display_id = unsafe { display.displayID() };
    let mode = CGDisplayCopyDisplayMode(display_id).ok_or("screenshot: no display mode")?;
    let pixels = (
        CGDisplayMode::pixel_width(Some(&mode)),
        CGDisplayMode::pixel_height(Some(&mode)),
    );
    // SAFETY: plain property access on a live SCDisplay object.
    let frame_pt = unsafe { display.frame() };
    // SAFETY: plain property setters on a freshly created configuration.
    unsafe {
        config.setCaptureResolution(SCCaptureResolutionType::Best);
        config.setWidth(pixels.0);
        config.setHeight(pixels.1);
        config.setShowsCursor(false);
        config.setSourceRect(CGRect::new(CGPoint::ZERO, frame_pt.size));
    }

    let (tx, rx) = mpsc::channel::<Result<Retained<CGImage>, String>>();
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let result = if !error.is_null() {
            Err(format!("screenshot: {}", error_text(error)))
        } else if image.is_null() {
            Err("screenshot: no image returned".to_string())
        } else {
            // SAFETY: SCScreenshotManager passes a +1 retained image.
            Ok(unsafe { Retained::retain(image) }.expect("non-null screenshot image"))
        };
        let _ = tx.send(result);
    });
    // SAFETY: class method with an owned completion handler.
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            &filter,
            &config,
            Some(&handler),
        );
    }
    rx.recv().map_err(|_| "screenshot: handler dropped".to_string())?
}

/// Capture the whole T0 state for the display under `mouse_sckit`:
/// shareable content (display + window list) and the display screenshot at
/// `display_pixels` resolution (points × backing scale). Blocking; call
/// from a background thread.
pub fn capture_t0(mouse_sckit: CGPoint) -> Result<T0Capture, String> {
    let content = shareable_content()?;
    let display =
        pick_display(&content, mouse_sckit).ok_or("capture: no display under cursor")?;
    let image = capture_display_image(&display)?;
    let windows = visible_windows(&content);
    // SAFETY: plain property access on a live SCDisplay object.
    let display_frame = unsafe { display.frame() };
    Ok(T0Capture {
        image,
        windows,
        display_frame,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::{NSDictionary, NSString};

    #[test]
    #[ignore = "requires Screen Recording permission for the calling process"]
    fn capture_saves_frame_to_tmp() {
        // Cargo tests run off the main thread, so no NSScreen here: the
        // expected pixel size comes from the display mode.
        let content = shareable_content().expect("shareable content");
        let display = pick_display(&content, CGPoint::new(0.0, 0.0)).expect("a display");
        let display_id = unsafe { display.displayID() };
        let mode = CGDisplayCopyDisplayMode(display_id).expect("display mode");
        let expected = (
            CGDisplayMode::pixel_width(Some(&mode)),
            CGDisplayMode::pixel_height(Some(&mode)),
        );
        eprintln!("expected {expected:?} px");
        let image = capture_display_image(&display).expect("screenshot");
        let (w, h) = (CGImage::width(Some(&image)), CGImage::height(Some(&image)));
        eprintln!("frame {w}x{h} px");
        assert_eq!(w, expected.0);
        assert_eq!(h, expected.1);

        // Dump the frame as PNG so it can be eyeballed.
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
        let rep = NSBitmapImageRep::initWithCGImage(objc2_app_kit::NSBitmapImageRep::alloc(), &image);
        // SAFETY: plain encoding call on a live bitmap representation.
        let data = unsafe {
            rep.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::<NSString, objc2::runtime::AnyObject>::new(),
            )
        }
        .expect("png data");
        let path = std::env::temp_dir().join("minisnap-capture-test.png");
        std::fs::write(&path, data.to_vec()).expect("write file");
        eprintln!("saved {}", path.display());
    }
}

/// Coordinate conversions between the three spaces the capture pipeline
/// touches. This is the single converter in the codebase — pixel-precision
/// of the crop depends on it.
///
/// Spaces:
/// - *AppKit global points*: origin at the bottom-left corner of the primary
///   screen, Y grows up (`NSScreen.frame`, `NSEvent.mouseLocation`).
/// - *ScreenCaptureKit global points*: origin at the top-left corner of the
///   primary screen, Y grows down (`SCWindow.frame`, `SCDisplay.frame`).
///   The two are related by `y_sckit = primary_height_pt - y_appkit`.
/// - *Image pixels*: origin at the top-left corner of the captured display,
///   Y grows down; point coordinates on that display scale by the display's
///   `backingScaleFactor`.
// Consumed by the overlay (selection, magnifier) and output (crop) modules.
#[allow(dead_code)]
pub mod geometry {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::CGRectContainsPoint;

    /// AppKit global point → ScreenCaptureKit global point.
    pub fn point_to_sckit(point: CGPoint, primary_height_pt: f64) -> CGPoint {
        CGPoint::new(point.x, primary_height_pt - point.y)
    }

    /// Clip a rect to `bounds`, returning `CGRect::ZERO` when they do not
    /// intersect.
    pub fn intersect(rect: CGRect, bounds: CGRect) -> CGRect {
        let r = rect.standardize();
        let b = bounds.standardize();
        let x0 = r.origin.x.max(b.origin.x);
        let y0 = r.origin.y.max(b.origin.y);
        let x1 = r.max().x.min(b.max().x);
        let y1 = r.max().y.min(b.max().y);
        if x1 <= x0 || y1 <= y0 {
            return CGRect::ZERO;
        }
        CGRect::new(
            CGPoint::new(x0, y0),
            CGSize::new(x1 - x0, y1 - y0),
        )
    }

    /// Clamp a point into the `bounds` rect.
    pub fn clamp_point(point: CGPoint, bounds: CGRect) -> CGPoint {
        let b = bounds.standardize();
        CGPoint::new(
            point.x.clamp(b.origin.x, b.max().x),
            point.y.clamp(b.origin.y, b.max().y),
        )
    }

    /// A selection rect given in display-local AppKit points (bottom-left
    /// origin) → the corresponding pixel rect of the captured image
    /// (top-left origin). Edges round inward-safe: origin down, extent up,
    /// then the result is clamped to the image bounds.
    pub fn selection_to_pixel_rect(
        selection: CGRect,
        display_size_pt: CGSize,
        scale: f64,
    ) -> CGRect {
        let s = selection.standardize();
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let x0 = (s.origin.x * scale).floor();
        let x1 = ((s.origin.x + s.size.width) * scale).ceil();
        // Flip Y: bottom of the selection in points maps to the bottom of
        // the pixel rect (larger Y in image space).
        let y_top_px = (display_size_pt.height - (s.origin.y + s.size.height)) * scale;
        let y_bottom_px = (display_size_pt.height - s.origin.y) * scale;
        let y0 = y_top_px.floor().max(0.0);
        let y1 = y_bottom_px.ceil();
        let image_bounds = CGRect::new(
            CGPoint::ZERO,
            CGSize::new(display_size_pt.width * scale, display_size_pt.height * scale),
        );
        let px = CGRect::new(CGPoint::new(x0, y0), CGSize::new((x1 - x0).max(0.0), (y1 - y0).max(0.0)));
        intersect(px, image_bounds)
    }

    /// A window frame in ScreenCaptureKit global points → pixel rect inside
    /// the captured display's image. `display_frame_sckit` is the captured
    /// display's frame in the same SCKit space; the result is clamped to the
    /// image bounds.
    pub fn window_frame_to_pixel_rect(
        window_frame: CGRect,
        display_frame_sckit: CGRect,
        scale: f64,
    ) -> CGRect {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let x0 = (window_frame.origin.x - display_frame_sckit.origin.x) * scale;
        let y0 = (window_frame.origin.y - display_frame_sckit.origin.y) * scale;
        let w = window_frame.size.width * scale;
        let h = window_frame.size.height * scale;
        let image_bounds = CGRect::new(
            CGPoint::ZERO,
            CGSize::new(
                display_frame_sckit.size.width * scale,
                display_frame_sckit.size.height * scale,
            ),
        );
        let px = CGRect::new(
            CGPoint::new(x0, y0),
            CGSize::new(w.max(0.0), h.max(0.0)),
        );
        intersect(px, image_bounds)
    }

    /// Display-local AppKit point → pixel point in the captured image.
    pub fn display_point_to_pixel(point: CGPoint, display_height_pt: f64, scale: f64) -> CGPoint {
        CGPoint::new(point.x * scale, (display_height_pt - point.y) * scale)
    }

    /// Index of the first (front-to-back) window frame containing `point`,
    /// or None — the click-to-window target. Zero-sized frames never hit.
    #[allow(dead_code)]
    pub fn topmost_window_at(frames: &[CGRect], point: CGPoint) -> Option<usize> {
        frames
            .iter()
            .position(|f| f.size.width > 0.0 && f.size.height > 0.0 && CGRectContainsPoint(*f, point))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
            CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
        }

        #[test]
        fn point_to_sckit_flips_y() {
            // Primary screen 900pt tall: bottom-left (0,0) is top-left (0,900).
            let p = point_to_sckit(CGPoint::new(100.0, 0.0), 900.0);
            assert_eq!((p.x, p.y), (100.0, 900.0));
            let p = point_to_sckit(CGPoint::new(100.0, 900.0), 900.0);
            assert_eq!((p.x, p.y), (100.0, 0.0));
        }

        #[test]
        fn intersect_clips_and_handles_disjoint() {
            let bounds = rect(0.0, 0.0, 100.0, 100.0);
            let r = intersect(rect(20.0, 30.0, 50.0, 40.0), bounds);
            assert_eq!((r.origin.x, r.origin.y), (20.0, 30.0));
            assert_eq!((r.size.width, r.size.height), (50.0, 40.0));
            // Extends past the right/bottom edges.
            let r = intersect(rect(80.0, 80.0, 60.0, 60.0), bounds);
            assert_eq!((r.size.width, r.size.height), (20.0, 20.0));
            // Disjoint.
            let r = intersect(rect(200.0, 0.0, 10.0, 10.0), bounds);
            assert!(r == CGRect::ZERO);
        }

        #[test]
        fn intersect_handles_negative_display_coords() {
            // A display left of the primary has negative x in both spaces.
            let display = rect(-1920.0, 0.0, 1920.0, 1080.0);
            let window = rect(-1800.0, 100.0, 3000.0, 500.0); // spills right
            let r = intersect(window, display);
            assert_eq!((r.origin.x, r.origin.y), (-1800.0, 100.0));
            assert_eq!((r.size.width, r.size.height), (1920.0 - 120.0, 500.0));
        }

        #[test]
        fn clamp_point_keeps_inside_bounds() {
            let bounds = rect(0.0, 0.0, 100.0, 50.0);
            let p = clamp_point(CGPoint::new(-5.0, 60.0), bounds);
            assert_eq!((p.x, p.y), (0.0, 50.0));
            let p = clamp_point(CGPoint::new(10.0, 20.0), bounds);
            assert_eq!((p.x, p.y), (10.0, 20.0));
        }

        #[test]
        fn selection_maps_1x_bottom_left_origin() {
            // Display 800x600pt at 1x, selection y=400..600 (top half in
            // AppKit space) → pixel rect y=0..200 from the top.
            let px = selection_to_pixel_rect(rect(100.0, 400.0, 300.0, 200.0), CGSize::new(800.0, 600.0), 1.0);
            assert_eq!((px.origin.x, px.origin.y), (100.0, 0.0));
            assert_eq!((px.size.width, px.size.height), (300.0, 200.0));
        }

        #[test]
        fn selection_maps_2x_and_rounds_edges() {
            // 2x: a 300.5 x 200.25pt selection covers ceil-rounded pixels.
            let px = selection_to_pixel_rect(
                rect(10.2, 400.0, 300.5, 200.25),
                CGSize::new(800.0, 600.0),
                2.0,
            );
            assert_eq!(px.origin.x, 20.0); // floor(10.2 * 2)
            assert_eq!(px.size.width, 602.0); // ceil(310.7 * 2) - floor(10.2 * 2) = 622 - 20
            assert_eq!(px.origin.y, 0.0); // selection spills past the top edge
            assert_eq!(px.size.height, (600.0 - 400.0) * 2.0);
        }

        #[test]
        fn selection_off_display_clamps_to_image() {
            // Selection partially outside the display: pixels clamp to bounds.
            let px = selection_to_pixel_rect(rect(-50.0, 500.0, 200.0, 200.0), CGSize::new(800.0, 600.0), 2.0);
            assert_eq!((px.origin.x, px.origin.y), (0.0, 0.0));
            assert_eq!((px.size.width, px.size.height), (300.0, 200.0));
        }

        #[test]
        fn window_frame_maps_to_display_pixels() {
            // Display at SCKit origin (0,0) 800x600pt; window at (100, 50)
            // 200x100pt → pixel rect (100,50) 200x100 at 1x.
            let px = window_frame_to_pixel_rect(
                rect(100.0, 50.0, 200.0, 100.0),
                rect(0.0, 0.0, 800.0, 600.0),
                1.0,
            );
            assert_eq!((px.origin.x, px.origin.y), (100.0, 50.0));
            assert_eq!((px.size.width, px.size.height), (200.0, 100.0));

            // Secondary display with negative SCKit origin and 2x scale.
            let px = window_frame_to_pixel_rect(
                rect(-900.0, 100.0, 200.0, 100.0),
                rect(-1000.0, 0.0, 1000.0, 800.0),
                2.0,
            );
            assert_eq!((px.origin.x, px.origin.y), (200.0, 200.0));
            assert_eq!((px.size.width, px.size.height), (400.0, 200.0));
        }

        #[test]
        fn window_frame_clamps_when_off_display() {
            // Window spills past the display's right edge: pixel rect clamps.
            let px = window_frame_to_pixel_rect(
                rect(700.0, 50.0, 400.0, 100.0),
                rect(0.0, 0.0, 800.0, 600.0),
                1.0,
            );
            assert_eq!((px.origin.x, px.origin.y), (700.0, 50.0));
            assert_eq!((px.size.width, px.size.height), (100.0, 100.0));
        }

        #[test]
        fn display_point_maps_to_pixel() {
            let p = display_point_to_pixel(CGPoint::new(100.0, 450.0), 600.0, 2.0);
            assert_eq!((p.x, p.y), (200.0, 300.0));
        }

        #[test]
        fn topmost_window_hits_front_to_back() {
            let frames = [
                rect(100.0, 100.0, 200.0, 100.0),
                rect(0.0, 0.0, 500.0, 500.0),
            ];
            // Where both frames contain the point, the front one wins.
            assert_eq!(topmost_window_at(&frames, CGPoint::new(120.0, 120.0)), Some(0));
            // Outside the front window but inside the one behind it.
            assert_eq!(topmost_window_at(&frames, CGPoint::new(20.0, 20.0)), Some(1));
            // Desktop: no window contains the point.
            assert_eq!(topmost_window_at(&frames, CGPoint::new(600.0, 600.0)), None);
        }

        #[test]
        fn topmost_window_skips_zero_sized_frames() {
            let frames = [
                rect(10.0, 10.0, 0.0, 0.0),
                rect(0.0, 0.0, 100.0, 100.0),
            ];
            assert_eq!(topmost_window_at(&frames, CGPoint::new(50.0, 50.0)), Some(1));
        }
    }
}
