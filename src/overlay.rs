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
}

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
