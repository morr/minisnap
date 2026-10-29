//! Frozen-frame overlay: a full-screen window over the captured display
//! showing the T0 frame, with selection UI layers on top. Created once at
//! app start and reused for every capture.

use std::cell::{Cell, RefCell};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{define_class, extern_methods, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::CGImage;
use objc2_quartz_core::CALayer;

use crate::capture::T0Capture;

// The shielding window level sits above the menu bar and everything else.
extern "C" {
    fn CGShieldingWindowLevel() -> i64;
}

/// What the user did inside the overlay mode.
#[derive(Debug, Clone, Copy)]
pub enum OverlayOutcome {
    /// A final selection, as a pixel rect of the T0 image.
    #[allow(dead_code)] // produced once the selection state machine lands
    Selected { pixel_rect: CGRect },
    /// The mode was cancelled (Esc / other-display click / screen change).
    Cancelled,
}

define_class!(
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    struct OverlayWindow;

    unsafe impl NSObjectProtocol for OverlayWindow {}

    impl OverlayWindow {
        // Borderless windows cannot become key by default; without this the
        // overlay would never receive keyboard events (Esc).
        #[unsafe(method(canBecomeKeyWindow))]
        fn __can_become_key_window(&self) -> bool {
            true
        }
    }
);

impl OverlayWindow {
    extern_methods!(
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool;

        #[unsafe(method(initWithContentRect:styleMask:backing:defer:))]
        unsafe fn init_with_content_rect(
            this: Allocated<Self>,
            content_rect: CGRect,
            style_mask: NSWindowStyleMask,
            backing: NSBackingStoreType,
            defer: bool,
        ) -> Retained<Self>;
    );
}

/// Interaction state inside the overlay mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayState {
    /// Moving the cursor, no button held.
    Idle,
    /// Left button held: selection rectangle is being drawn.
    Drag,
}

/// Movement tolerance (in points) within which down+up counts as a click.
const CLICK_TOLERANCE_PT: f64 = 3.0;

struct OverlayIvars {
    window: RefCell<Option<Retained<OverlayWindow>>>,
    veils: RefCell<Vec<Retained<NSWindow>>>,
    active: Cell<bool>,
    finishing: Cell<bool>,
    on_done: RefCell<Option<Box<dyn FnOnce(OverlayOutcome)>>>,
    image: RefCell<Option<Retained<CGImage>>>,
    /// Full-image pixel size.
    image_size_px: Cell<(usize, usize)>,
    /// Captured display frame in ScreenCaptureKit global points.
    #[allow(dead_code)]
    display_frame_sckit: Cell<CGRect>,
    /// Overlay window frame = captured screen frame (AppKit points).
    screen_frame: Cell<CGRect>,
    scale: Cell<f64>,
    frame_layer: Retained<CALayer>,
    state: Cell<OverlayState>,
    /// Selection start point and current cursor, display-local (AppKit,
    /// bottom-left origin, points).
    drag_start: Cell<CGPoint>,
    mouse: Cell<CGPoint>,
    /// Layer tree root; attached to the view's layer on the first show.
    root_layer: Retained<CALayer>,
    layers_attached: Cell<bool>,
    /// Four black strips around the selection rectangle (top, bottom, left,
    /// right) forming the dimming with a "hole" for the selection.
    dim_strips: RefCell<[Retained<CALayer>; 4]>,
}

/// Dimming opacity over the frozen frame outside the selection.
const DIM_ALPHA: f64 = 0.4;

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = OverlayIvars]
    struct OverlayView;

    unsafe impl NSObjectProtocol for OverlayView {}

    impl OverlayView {
        #[unsafe(method_id(initWithFrame:))]
        unsafe fn __init_with_frame(
            this: Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self> {
            let this = this.set_ivars(OverlayIvars {
                window: RefCell::new(None),
                veils: RefCell::new(Vec::new()),
                active: Cell::new(false),
                finishing: Cell::new(false),
                on_done: RefCell::new(None),
                image: RefCell::new(None),
                image_size_px: Cell::new((0, 0)),
                display_frame_sckit: Cell::new(CGRect::ZERO),
                screen_frame: Cell::new(CGRect::ZERO),
                scale: Cell::new(1.0),
                frame_layer: CALayer::new(),
                state: Cell::new(OverlayState::Idle),
                drag_start: Cell::new(CGPoint::ZERO),
                mouse: Cell::new(CGPoint::ZERO),
                root_layer: CALayer::new(),
                layers_attached: Cell::new(false),
                dim_strips: RefCell::new([CALayer::new(), CALayer::new(), CALayer::new(), CALayer::new()]),
            });
            // SAFETY: NSView's designated initializer with plain ivars set.
            unsafe { msg_send![super(this), initWithFrame: frame_rect] }
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn __accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn __key_down(&self, event: &NSEvent) {
            if event.keyCode() == 53 {
                self.finish(OverlayOutcome::Cancelled);
            }
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
    }
);

impl OverlayView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: overridden designated initializer, ivars fully set.
        unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::ZERO) }
    }

    /// Style a dimming strip once.
    fn setup_strip(strip: &CALayer) {
        let dim_color = NSColor::blackColor().colorWithAlphaComponent(DIM_ALPHA);
        strip.setBackgroundColor(Some(&dim_color.CGColor()));
    }

    /// Place the four dimming strips around `hole` (display-local points);
    /// `None` hides them (no active selection yet).
    fn update_dim(&self, hole: Option<CGRect>) {
        let ivars = self.ivars();
        let strips = ivars.dim_strips.borrow();
        let screen_size = ivars.screen_frame.get().size;
        let Some(rect) = hole else {
            for strip in strips.iter() {
                strip.setHidden(true);
            }
            return;
        };
        let rect = rect.standardize();
        let w = screen_size.width;
        let h = screen_size.height;
        let frames = [
            // top: from the hole's top edge up to the screen top
            CGRect::new(CGPoint::new(0.0, rect.max().y), objc2_core_foundation::CGSize::new(w, h - rect.max().y)),
            // bottom
            CGRect::new(CGPoint::ZERO, objc2_core_foundation::CGSize::new(w, rect.min().y)),
            // left
            CGRect::new(CGPoint::new(0.0, rect.min().y), objc2_core_foundation::CGSize::new(rect.min().x, rect.size.height)),
            // right
            CGRect::new(
                CGPoint::new(rect.max().x, rect.min().y),
                objc2_core_foundation::CGSize::new(w - rect.max().x, rect.size.height),
            ),
        ];
        for (strip, frame) in strips.iter().zip(frames) {
            strip.setHidden(false);
            strip.setFrame(frame);
        }
    }

    /// Point the frame layer at the T0 image and show the overlay on the
    /// captured screen. Main thread only.
    fn show(
        &self,
        t0: T0Capture,
        screen_frame: CGRect,
        scale: f64,
        veils: Vec<Retained<NSWindow>>,
        on_done: Box<dyn FnOnce(OverlayOutcome)>,
    ) {
        let ivars = self.ivars();
        if ivars.active.get() {
            eprintln!("overlay: capture request ignored, mode already active");
            return;
        }

        let w_px = CGImage::width(Some(&t0.image));
        let h_px = CGImage::height(Some(&t0.image));
        *ivars.image.borrow_mut() = Some(t0.image);
        ivars.image_size_px.set((w_px, h_px));
        ivars.display_frame_sckit.set(t0.display_frame);
        ivars.screen_frame.set(screen_frame);
        ivars.scale.set(scale);
        *ivars.on_done.borrow_mut() = Some(on_done);
        ivars.finishing.set(false);
        ivars.active.set(true);

        // Attach the layer tree to the view once.
        if !ivars.layers_attached.get() {
            ivars.layers_attached.set(true);
            self.setWantsLayer(true);
            if let Some(view_layer) = self.layer() {
                let root = &ivars.root_layer;
                root.setFrame(CGRect::new(CGPoint::ZERO, screen_frame.size));
                // z-order: frozen frame, then dimming strips.
                root.addSublayer(&ivars.frame_layer);
                for strip in ivars.dim_strips.borrow().iter() {
                    Self::setup_strip(strip);
                    root.addSublayer(strip);
                }
                view_layer.addSublayer(root);
            }
        }
        ivars.root_layer.setFrame(CGRect::new(CGPoint::ZERO, screen_frame.size));

        // Frame layer: the frozen T0 bitmap, 1:1 with the display pixels.
        let frame_layer = &ivars.frame_layer;
        frame_layer.setContentsScale(scale);
        frame_layer.setFrame(CGRect::new(CGPoint::ZERO, screen_frame.size));
        if let Some(image) = ivars.image.borrow().as_ref() {
            // SAFETY: CGImage is an ObjC object at runtime; CALayer.contents
            // expects it as AnyObject.
            let as_any: &AnyObject = unsafe { &*(std::ptr::from_ref(image).cast::<AnyObject>()) };
            // SAFETY: contents setter with a valid image object.
            unsafe { frame_layer.setContents(Some(as_any)) };
        }

        // Veils for other displays (created by the caller per show()).
        let mut old_veils = ivars.veils.borrow_mut();
        for veil in old_veils.drain(..) {
            veil.orderOut(None);
        }
        *old_veils = veils;

        if let Some(window) = ivars.window.borrow().as_ref() {
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            window.setFrame_display(screen_frame, false);
            window.makeKeyAndOrderFront(None);
        }
        eprintln!(
            "overlay: frozen {}x{} px on screen ({:.0},{:.0} {}x{})",
            w_px,
            h_px,
            screen_frame.origin.x,
            screen_frame.origin.y,
            screen_frame.size.width,
            screen_frame.size.height
        );
    }

    /// Cursor position in display-local points (bottom-left origin).
    fn local_point(&self, event: &NSEvent) -> CGPoint {
        let location = event.locationInWindow();
        // The borderless content view covers the whole window, so window
        // coordinates are display-local coordinates.
        CGPoint::new(location.x, location.y)
    }

    fn handle_mouse_down(&self, event: &NSEvent) {
        let ivars = self.ivars();
        if !ivars.active.get() {
            return;
        }
        let point = self.local_point(event);
        ivars.drag_start.set(point);
        ivars.mouse.set(point);
        ivars.state.set(OverlayState::Drag);
        // Zero-size hole: full dimming until the drag opens a rectangle.
        self.update_dim(Some(CGRect::new(point, objc2_core_foundation::CGSize::ZERO)));
    }

    fn handle_mouse_dragged(&self, event: &NSEvent) {
        let ivars = self.ivars();
        if ivars.state.get() != OverlayState::Drag {
            return;
        }
        // Cursor leaving the display clamps the rectangle to its bounds; the
        // drag loop keeps receiving events, so the rect follows the cursor.
        let point = crate::capture::geometry::clamp_point(
            self.local_point(event),
            CGRect::new(CGPoint::ZERO, ivars.screen_frame.get().size),
        );
        ivars.mouse.set(point);
        let start = ivars.drag_start.get();
        let rect = CGRect::new(
            CGPoint::new(start.x.min(point.x), start.y.min(point.y)),
            objc2_core_foundation::CGSize::new((point.x - start.x).abs(), (point.y - start.y).abs()),
        );
        self.update_dim(Some(rect));
    }

    fn handle_mouse_up(&self, event: &NSEvent) {
        let ivars = self.ivars();
        if ivars.state.get() != OverlayState::Drag {
            return;
        }
        let up = self.local_point(event);
        ivars.mouse.set(up);
        ivars.state.set(OverlayState::Idle);
        self.update_dim(None);

        let start = ivars.drag_start.get();
        let moved = (up.x - start.x).abs() > CLICK_TOLERANCE_PT
            || (up.y - start.y).abs() > CLICK_TOLERANCE_PT;
        if !moved {
            // Click: whole display for now; click-to-window lands in 4.6.
            let (w_px, h_px) = ivars.image_size_px.get();
            let full = CGRect::new(
                CGPoint::ZERO,
                objc2_core_foundation::CGSize::new(w_px as f64, h_px as f64),
            );
            self.finish(OverlayOutcome::Selected { pixel_rect: full });
            return;
        }

        // Drag: normalize the selection and crop in pixel space.
        let screen_frame = ivars.screen_frame.get();
        let scale = ivars.scale.get();
        let rect = crate::capture::geometry::intersect(
            CGRect::new(
                CGPoint::new(start.x.min(up.x), start.y.min(up.y)),
                objc2_core_foundation::CGSize::new((up.x - start.x).abs(), (up.y - start.y).abs()),
            ),
            CGRect::new(CGPoint::ZERO, screen_frame.size),
        );
        let pixel_rect = crate::capture::geometry::selection_to_pixel_rect(
            rect,
            screen_frame.size,
            scale,
        );
        self.finish(OverlayOutcome::Selected { pixel_rect });
    }

    /// Complete the mode: hide everything and hand the outcome to the owner.
    fn finish(&self, outcome: OverlayOutcome) {
        let ivars = self.ivars();
        if !ivars.active.get() || ivars.finishing.get() {
            return;
        }
        ivars.finishing.set(true);
        ivars.active.set(false);

        for veil in ivars.veils.borrow().iter() {
            veil.orderOut(None);
        }
        ivars.veils.borrow_mut().clear();
        *ivars.image.borrow_mut() = None;
        if let Some(window) = ivars.window.borrow().as_ref() {
            window.orderOut(None);
        }

        if let Some(on_done) = ivars.on_done.borrow_mut().take() {
            on_done(outcome);
        }
    }
}

/// The reusable overlay mode: one window + view + per-show veil windows.
pub struct Overlay {
    // Kept alive together with the view; both live for the process.
    #[allow(dead_code)]
    window: Retained<OverlayWindow>,
    view: Retained<OverlayView>,
}

impl Overlay {
    /// Create the overlay machinery once at startup. Cheap to keep around.
    pub fn new(mtm: MainThreadMarker) -> Self {
        let window = unsafe {
            OverlayWindow::init_with_content_rect(
                OverlayWindow::alloc(mtm),
                CGRect::ZERO,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: CGShieldingWindowLevel is a plain C function.
        let level = unsafe { CGShieldingWindowLevel() };
        window.setLevel(level as _);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setIgnoresMouseEvents(false);
        window.setAcceptsMouseMovedEvents(true);
        window.setHidesOnDeactivate(false);
        let view = OverlayView::new(mtm);
        // Cross-link the window and its content view.
        view.ivars().window.replace(Some(window.clone()));
        window.setContentView(Some(&view));
        window.setInitialFirstResponder(Some(&view));
        Overlay { window, view }
    }

    pub fn is_active(&self) -> bool {
        self.view.ivars().active.get()
    }

    /// Enter the frozen-frame mode on the captured screen. Main thread only.
    pub fn show(
        &self,
        t0: T0Capture,
        screen_frame: CGRect,
        scale: f64,
        veils: Vec<Retained<NSWindow>>,
        on_done: Box<dyn FnOnce(OverlayOutcome)>,
    ) {
        self.view.show(t0, screen_frame, scale, veils, on_done);
    }

    /// Cancel the active mode without a result (e.g. display config change).
    pub fn cancel(&self) {
        self.view.finish(OverlayOutcome::Cancelled);
    }
}
