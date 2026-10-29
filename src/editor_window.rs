//! The editor window: one reusable `NSWindow` showing the open shot — the
//! base frame plus annotations, drawn with CoreGraphics into a custom
//! `NSView`. Selection markers are drawn on top here and never become part
//! of a saved composite.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{define_class, extern_methods, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSEvent, NSGraphicsContext, NSScreen, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGImage, CGContext};
use objc2_foundation::NSString;

use crate::editor::{render, ElementKind, OpenShot, Point, Rect};

/// The dark backdrop the frame sits on.
const BACKDROP: [f64; 4] = [0.12, 0.12, 0.14, 1.0];

/// Selection marker radius, in view points (constant on screen).
const MARKER_RADIUS_PT: f64 = 4.5;

/// Initial content size before the first shot arrives.
const INITIAL_SIZE: CGSize = CGSize::new(480.0, 320.0);

pub struct EditorWindow {
    pub window: Retained<NSWindow>,
    view: Retained<EditorView>,
}

struct EditorIvars {
    shot: RefCell<Option<OpenShot>>,
    /// Selected element id, if any.
    selection: Cell<Option<u64>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = EditorIvars]
    struct EditorView;

    unsafe impl NSObjectProtocol for EditorView {}

    impl EditorView {
        #[unsafe(method_id(initWithFrame:))]
        unsafe fn __init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self> {
            let this = this.set_ivars(EditorIvars {
                shot: RefCell::new(None),
                selection: Cell::new(None),
            });
            // SAFETY: NSView's designated initializer with plain ivars set.
            unsafe { msg_send![super(this), initWithFrame: frame_rect] }
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn __accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn __draw_rect(&self, _dirty_rect: CGRect) {
            self.draw();
        }

        #[unsafe(method(mouseDown:))]
        fn __mouse_down(&self, event: &NSEvent) {
            self.handle_mouse_down(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn __mouse_dragged(&self, event: &NSEvent) {
            self.handle_mouse_dragged(event);
        }

        #[unsafe(method(mouseUp:))]
        fn __mouse_up(&self, event: &NSEvent) {
            self.handle_mouse_up(event);
        }

        #[unsafe(method(keyDown:))]
        fn __key_down(&self, event: &NSEvent) {
            if event.keyCode() == 53 {
                // Esc: close the editor.
                if let Some(window) = self.window() {
                    window.close();
                }
            }
        }
    }
);

impl EditorView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    fn new(mtm: MainThreadMarker) -> Retained<EditorView> {
        // SAFETY: designated initializer with plain ivars.
        unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::new(CGPoint::ZERO, INITIAL_SIZE)) }
    }

    /// The image pixel size of the open shot.
    fn image_size(&self) -> (usize, usize) {
        self.ivars()
            .shot
            .borrow()
            .as_ref()
            .map(|shot| {
                let base = shot.doc.base();
                (
                    CGImage::width(Some(base)) as usize,
                    CGImage::height(Some(base)) as usize,
                )
            })
            .unwrap_or((0, 0))
    }

    /// Cursor position in image pixel space (top-left origin): view points
    /// scale by the current frame-to-pixel ratio and flip vertically.
    fn pixel_point(&self, event: &NSEvent) -> Point {
        let location = event.locationInWindow();
        let local = self.convertPoint_fromView(location, None);
        let bounds = self.bounds();
        let (iw, ih) = self.image_size();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 || iw == 0 || ih == 0 {
            return Point::new(0.0, 0.0);
        }
        Point::new(
            local.x * iw as f64 / bounds.size.width,
            (bounds.size.height - local.y) * ih as f64 / bounds.size.height,
        )
    }

    /// Redraw everything: backdrop, composite (base + elements), markers.
    fn draw(&self) {
        let Some(context) = NSGraphicsContext::currentContext() else {
            return;
        };
        let cg = context.CGContext();
        let ivars = self.ivars();
        let shot_guard = ivars.shot.borrow();
        let Some(shot) = shot_guard.as_ref() else {
            return;
        };
        let base = shot.doc.base();
        let (iw, ih) = self.image_size();
        if iw == 0 || ih == 0 {
            return;
        }
        let bounds = self.bounds();
        let (bw, bh) = (bounds.size.width, bounds.size.height);
        if bw <= 0.0 || bh <= 0.0 {
            return;
        }
        let (sx, sy) = (bw / iw as f64, bh / ih as f64);

        // Backdrop behind the frame.
        CGContext::set_rgb_fill_color(
            Some(&cg),
            BACKDROP[0],
            BACKDROP[1],
            BACKDROP[2],
            BACKDROP[3],
        );
        CGContext::fill_rect(Some(&cg), bounds);

        // The composite content: base frame + elements, drawn in image
        // pixel units (`draw_elements` applies its own Y flip).
        CGContext::save_g_state(Some(&cg));
        CGContext::scale_ctm(Some(&cg), sx, sy);
        CGContext::draw_image(
            Some(&cg),
            CGRect::new(CGPoint::ZERO, CGSize::new(iw as f64, ih as f64)),
            Some(base),
        );
        render::draw_elements(&cg, base, ih as f64, shot.doc.elements());
        CGContext::restore_g_state(Some(&cg));

        // Selection markers on top, in screen points so they stay constant
        // sized; they never reach a saved composite.
        if let Some(id) = ivars.selection.get() {
            self.draw_markers(&cg, shot, id, &bounds, sx, sy);
        }
    }

    /// Handles of the selected element: endpoint dots and bounding-box
    /// corners, white fill with the element's color ring.
    fn draw_markers(
        &self,
        cg: &CGContext,
        shot: &OpenShot,
        id: u64,
        bounds: &CGRect,
        sx: f64,
        sy: f64,
    ) {
        let Some(element) = shot.doc.element(id) else {
            return;
        };
        let (iw, ih) = self.image_size();
        let to_point = |p: Point| -> CGPoint {
            CGPoint::new(p.x * sx, bounds.size.height - p.y * sy)
        };
        let _ = (iw, ih);
        let mut points: Vec<CGPoint> = Vec::new();
        let mut ring: [f64; 4] = [1.0, 0.0, 1.0, 1.0];
        match &element.kind {
            ElementKind::Arrow { a, b, color, .. } | ElementKind::Line { a, b, color, .. } => {
                points = vec![to_point(*a), to_point(*b)];
                ring = *color;
            }
            ElementKind::Callout { tip, anchor, color, .. } => {
                points = vec![to_point(*tip), to_point(*anchor)];
                ring = *color;
            }
            ElementKind::Rect { rect, color, .. } => {
                points = rect_corners(*rect).map(to_point).to_vec();
                ring = *color;
            }
            ElementKind::Blur { rect } => {
                points = rect_corners(*rect).map(to_point).to_vec();
            }
            ElementKind::Text { origin, text, color, size } => {
                if let Some(bbox) = crate::editor::hit_test::text_bounds(*origin, text, *size) {
                    points = rect_corners(bbox).map(to_point).to_vec();
                }
                ring = *color;
            }
            ElementKind::Pen { points: trace, color, .. } => {
                // Endpoint dots only; the trace itself is the shape.
                if let (Some(first), Some(last)) = (trace.first(), trace.last()) {
                    points = vec![to_point(*first), to_point(*last)];
                }
                ring = *color;
            }
        }
        for p in points {
            let r = MARKER_RADIUS_PT;
            let marker_rect = CGRect::new(CGPoint::new(p.x - r, p.y - r), CGSize::new(2.0 * r, 2.0 * r));
            CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
            CGContext::fill_ellipse_in_rect(Some(cg), marker_rect);
            CGContext::set_rgb_stroke_color(Some(cg), ring[0], ring[1], ring[2], ring[3]);
            CGContext::set_line_width(Some(cg), 1.5);
            CGContext::stroke_ellipse_in_rect(Some(cg), marker_rect);
        }
    }

    fn handle_mouse_down(&self, event: &NSEvent) {
        let point = self.pixel_point(event);
        eprintln!("editor: mouse down at {point:?}");
    }

    fn handle_mouse_dragged(&self, event: &NSEvent) {
        let point = self.pixel_point(event);
        eprintln!("editor: mouse dragged to {point:?}");
    }

    fn handle_mouse_up(&self, event: &NSEvent) {
        let point = self.pixel_point(event);
        eprintln!("editor: mouse up at {point:?}");
    }
}

/// The four corners of a rect (top-left, top-right, bottom-right,
/// bottom-left), standardized.
fn rect_corners(rect: Rect) -> [Point; 4] {
    let r = rect.standardize();
    [
        Point::new(r.x, r.y),
        Point::new(r.max_x(), r.y),
        Point::new(r.max_x(), r.max_y()),
        Point::new(r.x, r.max_y()),
    ]
}

impl EditorWindow {
    pub fn new(mtm: MainThreadMarker) -> EditorWindow {
        let view = EditorView::new(mtm);
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                CGRect::new(CGPoint::ZERO, INITIAL_SIZE),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str("minisnap"));
        // The window lives in the app controller for the whole process;
        // closing only hides it, the next shot reuses it (no second window).
        // SAFETY: plain setter on the freshly created window.
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setContentView(Some(&view));
        window.center();
        EditorWindow { window, view }
    }

    /// Show (or replace) the shot in this window: title, size limited by the
    /// screen's visible frame, full redraw.
    pub fn show_shot(&self, shot: OpenShot) {
        let title = shot.file_name();
        let (iw, ih) = {
            let base = shot.doc.base();
            (
                CGImage::width(Some(base)) as usize,
                CGImage::height(Some(base)) as usize,
            )
        };
        let view = &self.view;
        *view.ivars().shot.borrow_mut() = Some(shot);
        view.ivars().selection.set(None);
        self.window.setTitle(&NSString::from_str(&title));

        // Content size = frame size in points, uniformly scaled down when
        // the frame exceeds the screen's visible frame.
        let mtm = MainThreadMarker::new().expect("editor window on main thread");
        let content = NSScreen::mainScreen(mtm)
            .map(|screen| screen.visibleFrame().size)
            .map(|visible| fit_size(iw as f64, ih as f64, visible.width, visible.height))
            .unwrap_or_else(|| CGSize::new(iw as f64, ih as f64));
        self.window.setContentSize(content);
        self.window.center();
        view.setNeedsDisplayInRect(view.bounds());
        self.window.makeKeyAndOrderFront(None);
    }

    /// The selected element id, if any.
    pub fn selection(&self) -> Option<u64> {
        self.view.ivars().selection.get()
    }

    pub fn set_selection(&self, id: Option<u64>) {
        let view = &self.view;
        view.ivars().selection.set(id);
        view.setNeedsDisplayInRect(view.bounds());
    }
}

/// Uniformly fit `(w, h)` inside `(max_w, max_h)` without upscaling.
fn fit_size(w: f64, h: f64, max_w: f64, max_h: f64) -> CGSize {
    if w <= 0.0 || h <= 0.0 {
        return CGSize::new(w, h);
    }
    let scale = 1.0_f64.min(max_w / w).min(max_h / h);
    CGSize::new(w * scale, h * scale)
}
