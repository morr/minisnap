//! Screen capture at moment T0: a frozen frame per display, the display
//! under the cursor and the visible window list.

use std::sync::mpsc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextGetData, CGColorSpace, CGDisplayCopyDisplayMode,
    CGDisplayMode, CGImage, CGImageAlphaInfo, CGContext, CGRectContainsPoint,
};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
    SCCaptureResolutionType, SCWindow,
};

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

/// Regular visible windows, front-to-back: layer 0 only, an owning app
/// required. Minisnap's own on-screen windows take part like any other app's.
/// Used for click-to-window hit-testing at T0.
pub fn visible_windows(content: &SCShareableContent) -> Vec<T0Window> {
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
        if app.is_none() {
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

/// One captured display surface: the frozen frame of a single display plus
/// that display's frame in ScreenCaptureKit global points (top-left origin).
pub struct T0Surface {
    /// Full display frame in display pixels (top-left origin image space).
    pub image: Retained<CGImage>,
    /// The display's frame in ScreenCaptureKit global points.
    pub display_frame_sckit: CGRect,
}

/// Everything captured at the moment the mode starts (T0): a frozen frame
/// per display, the visible window list and the target display under the
/// cursor.
pub struct T0Capture {
    /// Index into `surfaces` of the display under the cursor.
    pub target: usize,
    /// Frozen frame per display, one surface each. Displays whose capture
    /// failed have no surface and stay unreachable for target switching.
    pub surfaces: Vec<T0Surface>,
    /// Visible regular windows, front-to-back.
    // Consumed by the overlay's click-to-window hit-testing.
    #[allow(dead_code)]
    pub windows: Vec<T0Window>,
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

/// Capture the whole T0 state: shareable content (display + window list)
/// and a screenshot of every display at its full pixel resolution, taken
/// one after another in the calling (background) thread. `target` indexes
/// the display under `mouse_sckit`. A failed capture on a non-target
/// display just drops that surface; a failed target fails the whole
/// capture, as before. Blocking; call from a background thread.
pub fn capture_t0(mouse_sckit: CGPoint) -> Result<T0Capture, String> {
    let content = shareable_content()?;
    let target_display =
        pick_display(&content, mouse_sckit).ok_or("capture: no display under cursor")?;
    let mut surfaces = Vec::new();
    let mut target = None;
    for display in unsafe { content.displays() }.iter() {
        let is_target = std::ptr::eq(&*display, &*target_display);
        match capture_display_image(&display) {
            Ok(image) => {
                if is_target {
                    target = Some(surfaces.len());
                }
                // SAFETY: plain property access on a live SCDisplay object.
                let display_frame_sckit = unsafe { display.frame() };
                surfaces.push(T0Surface {
                    image,
                    display_frame_sckit,
                });
            }
            Err(e) => {
                if is_target {
                    return Err(e);
                }
                eprintln!("capture: skipping a display without a T0 frame: {e}");
            }
        }
    }
    let target = target.expect("the target display was captured");
    let windows = visible_windows(&content);
    Ok(T0Capture {
        target,
        surfaces,
        windows,
    })
}

/// Pixel rect of the window the user actually sees under `point` in the
/// frozen frame `frozen` (whose display spans `display_frame` in SCKit
/// points), cropped from that frame.
///
/// The T0 window list is only candidates: ScreenCaptureKit picks up freshly
/// created windows with a lag (a game restarted by a dev loop is visible on
/// screen seconds before it shows up in the list), and the list order can
/// disagree with the compositor, so the first containing entry may be a
/// window that is actually behind or invisible. Each candidate is verified
/// by rendering it alone and comparing its pixels at the point with the
/// frozen frame; the first match is the answer. A total miss falls back to
/// the plain list-order hit-test, then to the whole frame.
pub fn resolve_click_target(
    t0_windows: &[T0Window],
    point: CGPoint,
    display_frame: CGRect,
    frozen: &CGImage,
) -> CGRect {
    let scale = image_scale(frozen, display_frame);
    let fallback = |frames: &[CGRect]| {
        match geometry::topmost_window_at(frames, point) {
            Some(i) => geometry::window_frame_to_pixel_rect(frames[i], display_frame, scale),
            None => CGRect::new(
                CGPoint::ZERO,
                CGSize::new(
                    CGImage::width(Some(frozen)) as f64,
                    CGImage::height(Some(frozen)) as f64,
                ),
            ),
        }
    };
    let t0_frames: Vec<CGRect> = t0_windows.iter().map(|w| w.frame).collect();
    // Fresh content covers windows created after the T0 snapshot and
    // provides the live window objects needed for rendering.
    let fresh = shareable_content().ok();
    let fresh_pairs: Vec<(u32, CGRect)> = fresh
        .as_ref()
        .map(|content| {
            // SAFETY: plain property access on live shareable content.
            unsafe { content.windows() }
                .iter()
                .map(|w| {
                    // SAFETY: plain property access on a live SCWindow.
                    unsafe { (w.windowID(), w.frame()) }
                })
                .collect()
        })
        .unwrap_or_default();
    let fresh_windows = fresh.as_ref().map(|content| unsafe { content.windows() });
    for (id, frame) in candidate_frames(t0_windows, &fresh_pairs, point) {
        let Some(list) = &fresh_windows else {
            continue;
        };
        let Some(window) = list.iter().find(|w| unsafe { w.windowID() } == id) else {
            continue;
        };
        if renders_as_seen(&window, frame, point, display_frame, frozen) {
            return geometry::window_frame_to_pixel_rect(frame, display_frame, scale);
        }
    }
    fallback(&t0_frames)
}

/// Frames of click candidates in probe order: containing T0 windows in
/// their list order first, then fresh windows the T0 list missed (matched
/// by id), preserving the fresh list order. Capped — each candidate costs a
/// window render.
fn candidate_frames(
    t0_windows: &[T0Window],
    fresh: &[(u32, CGRect)],
    point: CGPoint,
) -> Vec<(u32, CGRect)> {
    let contains = |frame: CGRect| {
        frame.size.width > 0.0 && frame.size.height > 0.0 && CGRectContainsPoint(frame, point)
    };
    let mut out: Vec<(u32, CGRect)> = t0_windows
        .iter()
        .filter(|w| contains(w.frame))
        .map(|w| (w.window_id, w.frame))
        .collect();
    for (id, frame) in fresh {
        if contains(*frame) && !out.iter().any(|(seen, _)| seen == id) {
            out.push((*id, *frame));
        }
    }
    out.truncate(5);
    out
}

/// Does the window's own content match the frozen frame at `point`? The
/// window is rendered alone (no shadow); a match means nothing else is
/// drawn over it there, so it is the window the user sees.
fn renders_as_seen(
    window: &SCWindow,
    frame: CGRect,
    point: CGPoint,
    display_frame: CGRect,
    frozen: &CGImage,
) -> bool {
    let Some(render) = render_window(window) else {
        return false;
    };
    let disp_scale = image_scale(frozen, display_frame);
    let win_scale = image_scale(&render, frame);
    let Some(a) = patch_at(frozen, point, display_frame, disp_scale) else {
        return false;
    };
    let Some(b) = patch_at(&render, point, frame, win_scale) else {
        return false;
    };
    patches_match(&a, &b)
}

/// Actual pixel-per-point scale of an image covering `frame`.
fn image_scale(image: &CGImage, frame: CGRect) -> f64 {
    if frame.size.width <= 0.0 {
        return 1.0;
    }
    CGImage::width(Some(image)) as f64 / frame.size.width
}

/// Render one window alone at its current on-screen content, without its
/// shadow. Blocking until the image arrives; the SCK completion arrives on
/// a private queue, so this must not be called from the queue it dispatches
/// on.
fn render_window(window: &SCWindow) -> Option<Retained<CGImage>> {
    // SAFETY: plain init on a freshly allocated filter; the desktop-
    // independent filter renders the window alone, ignoreShadows drops its
    // shadow so the image maps 1:1 onto the window frame.
    let filter = unsafe {
        SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), window)
    };
    // SAFETY: plain constructor.
    let config = unsafe { SCStreamConfiguration::new() };
    // SAFETY: plain property setters on a freshly created configuration.
    unsafe {
        config.setCaptureResolution(SCCaptureResolutionType::Best);
        config.setShowsCursor(false);
        config.setIgnoreShadowsSingleWindow(true);
    }
    let (tx, rx) = mpsc::channel::<Option<Retained<CGImage>>>();
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let result = if !error.is_null() || image.is_null() {
            None
        } else {
            // SAFETY: SCScreenshotManager passes a +1 retained image.
            Some(unsafe { Retained::retain(image) }.expect("non-null screenshot image"))
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
    rx.recv().ok().flatten()
}

/// RGBA8 bytes of a small patch of `image` centered on `point` (in `frame`'s
/// point space at `scale`); `None` when the patch would run off the image.
fn patch_at(image: &CGImage, point: CGPoint, frame: CGRect, scale: f64) -> Option<Vec<u8>> {
    const RADIUS: f64 = 3.0;
    let side = 2.0 * RADIUS + 1.0;
    let cx = (point.x - frame.origin.x) * scale;
    let cy = (point.y - frame.origin.y) * scale;
    let x0 = (cx - RADIUS).floor().max(0.0);
    let y0 = (cy - RADIUS).floor().max(0.0);
    if x0 + side > CGImage::width(Some(image)) as f64
        || y0 + side > CGImage::height(Some(image)) as f64
    {
        return None;
    }
    // SAFETY: the sub-rect lies inside the image.
    let cropped = CGImage::with_image_in_rect(
        Some(image),
        CGRect::new(CGPoint::new(x0, y0), CGSize::new(side, side)),
    )?;
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: plain CGBitmapContextCreate with a null data pointer; CG
    // allocates and owns the backing store, read through
    // CGBitmapContextGetData after drawing.
    let ctx = unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            side as usize,
            side as usize,
            8,
            side as usize * 4,
            Some(&*space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }?;
    CGContext::draw_image(
        Some(&ctx),
        CGRect::new(CGPoint::ZERO, CGSize::new(side, side)),
        Some(&cropped),
    );
    // SAFETY: the context owns a valid RGBA8 buffer of this size.
    let data = CGBitmapContextGetData(Some(&ctx)) as *const u8;
    if data.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, side as usize * side as usize * 4) }.to_vec())
}

/// Close-enough patch comparison: mean per-channel deviation under a small
/// tolerance. The same content through the same capture pipeline differs by
/// a few units at most; different windows differ by a lot. Fully
/// transparent pixels carry no content and are skipped.
fn patches_match(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let mut sum = 0u64;
    let mut used = 0u64;
    for (ca, cb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        if ca[3] == 0 || cb[3] == 0 {
            continue;
        }
        sum += (i32::from(ca[0]) - i32::from(cb[0])).unsigned_abs() as u64
            + (i32::from(ca[1]) - i32::from(cb[1])).unsigned_abs() as u64
            + (i32::from(ca[2]) - i32::from(cb[2])).unsigned_abs() as u64;
        used += 1;
    }
    used > 0 && (sum as f64) / (used as f64 * 3.0) < 16.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::{NSDictionary, NSString};

    #[test]
    fn click_candidates_t0_order_first_then_fresh_missed() {
        let point = CGPoint::new(100.0, 100.0);
        let inside = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(200.0, 200.0));
        let outside = CGRect::new(CGPoint::new(500.0, 500.0), CGSize::new(50.0, 50.0));
        let win = |id: u32, frame: CGRect| T0Window {
            frame,
            window_id: id,
            title: None,
        };
        let t0 = vec![win(1, inside), win(2, outside), win(3, inside)];
        let fresh = vec![(2, outside), (4, inside), (1, inside)];
        // T0 containing windows keep their list order; fresh adds only what
        // the T0 list missed (4); 2 is outside the point and 1 is a dupe.
        assert_eq!(
            candidate_frames(&t0, &fresh, point),
            vec![(1, inside), (3, inside), (4, inside)]
        );
    }

    #[test]
    fn click_candidates_capped_at_five() {
        let point = CGPoint::new(50.0, 50.0);
        let inside = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(200.0, 200.0));
        let t0: Vec<T0Window> = (0..8)
            .map(|i| T0Window {
                frame: inside,
                window_id: i,
                title: None,
            })
            .collect();
        let out = candidate_frames(&t0, &[], point);
        assert_eq!(out.len(), 5);
    }

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

    /// ScreenCaptureKit global frame (top-left origin) → AppKit global
    /// points (bottom-left origin): flip Y around the primary screen height.
    pub fn frame_sckit_to_appkit(frame_sckit: CGRect, primary_height_pt: f64) -> CGRect {
        CGRect::new(
            CGPoint::new(
                frame_sckit.origin.x,
                primary_height_pt - frame_sckit.origin.y - frame_sckit.size.height,
            ),
            frame_sckit.size,
        )
    }

    /// Tolerance (in points) when matching an AppKit screen frame to a
    /// converted ScreenCaptureKit display frame: the two frameworks report
    /// frames with slight rounding differences.
    pub const FRAME_MATCH_TOLERANCE_PT: f64 = 1.0;

    /// Whether an AppKit screen frame and a ScreenCaptureKit display frame
    /// describe the same display: the SCKit frame is flipped into AppKit
    /// space by the primary screen height and origin + size are compared
    /// within `tolerance_pt`. Sizes alone cannot tell two identical
    /// monitors apart, so the whole frame must match.
    pub fn screen_matches_display(
        screen_frame: CGRect,
        display_frame_sckit: CGRect,
        primary_height_pt: f64,
        tolerance_pt: f64,
    ) -> bool {
        let appkit = frame_sckit_to_appkit(display_frame_sckit, primary_height_pt);
        let close = |a: f64, b: f64| (a - b).abs() <= tolerance_pt;
        close(screen_frame.origin.x, appkit.origin.x)
            && close(screen_frame.origin.y, appkit.origin.y)
            && close(screen_frame.size.width, appkit.size.width)
            && close(screen_frame.size.height, appkit.size.height)
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
        fn sckit_frame_flips_to_appkit() {
            // External 1080p display above a 900pt primary: in the SCKit
            // space (1980pt tall) it occupies the top, in AppKit space it
            // sits at y=900.
            let f = frame_sckit_to_appkit(rect(0.0, 0.0, 1920.0, 1080.0), 1980.0);
            assert_eq!((f.origin.x, f.origin.y), (0.0, 900.0));
            assert_eq!((f.size.width, f.size.height), (1920.0, 1080.0));
            // The primary itself: SCKit y 1080..1980 → AppKit y 0.
            let f = frame_sckit_to_appkit(rect(0.0, 1080.0, 1440.0, 900.0), 1980.0);
            assert_eq!((f.origin.x, f.origin.y), (0.0, 0.0));
            // A display left of the primary keeps its negative x.
            let f = frame_sckit_to_appkit(rect(-1920.0, 100.0, 1920.0, 1080.0), 1980.0);
            assert_eq!((f.origin.x, f.origin.y), (-1920.0, 800.0));
        }

        #[test]
        fn screen_matches_display_by_full_frame() {
            // External above the primary: SCKit (0,0,1920,1080) ↔ AppKit
            // (0,900).
            assert!(screen_matches_display(
                rect(0.0, 900.0, 1920.0, 1080.0),
                rect(0.0, 0.0, 1920.0, 1080.0),
                1980.0,
                FRAME_MATCH_TOLERANCE_PT,
            ));
            // Same size at a different position: no match — two identical
            // monitors are told apart by origin.
            assert!(!screen_matches_display(
                rect(0.0, 0.0, 1920.0, 1080.0),
                rect(0.0, 0.0, 1920.0, 1080.0),
                1980.0,
                FRAME_MATCH_TOLERANCE_PT,
            ));
            // Rounding differences within the tolerance still match.
            assert!(screen_matches_display(
                rect(0.0, 899.5, 1920.0, 1080.5),
                rect(0.0, 0.0, 1920.0, 1080.0),
                1980.0,
                FRAME_MATCH_TOLERANCE_PT,
            ));
            // Differences beyond the tolerance do not.
            assert!(!screen_matches_display(
                rect(0.0, 900.0, 1920.0, 1085.0),
                rect(0.0, 0.0, 1920.0, 1080.0),
                1980.0,
                FRAME_MATCH_TOLERANCE_PT,
            ));
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
