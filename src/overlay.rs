//! Frozen-frame overlay: a full-screen window over the captured display
//! showing the T0 frame, with selection UI layers on top. Created once at
//! app start and reused for every capture.

use std::cell::{Cell, RefCell};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{define_class, extern_methods, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSFont, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CFType, CGPoint, CGRect};
use objc2_core_graphics::{CGImage, CGMutablePath, CGPath};
use objc2_foundation::NSString;
use objc2_quartz_core::{
    kCAAlignmentCenter, kCAFilterNearest, CALayer, CAShapeLayer, CATextLayer, CATransaction,
};

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
    /// Magnifier: nearest-neighbor sample of the T0 bitmap by the cursor.
    loupe: Retained<CALayer>,
    /// Pixel grid over the loupe sample.
    loupe_grid: Retained<CAShapeLayer>,
    /// Crosshair through the loupe's center (cursor-pixel) cell.
    loupe_cross: Retained<CAShapeLayer>,
    /// Full-screen guide lines crossing at the cursor (idle only): a dark
    /// hairline underneath, a white one on top — readable on any background.
    axis_lines: Retained<CAShapeLayer>,
    axis_under: Retained<CAShapeLayer>,
    /// Cursor pixel coordinate at the top end of the vertical guide.
    x_label: Retained<CATextLayer>,
    /// Cursor pixel coordinate at the right end of the horizontal guide.
    y_label: Retained<CATextLayer>,
    /// Live "W × H px" of the selection while dragging.
    size_label: Retained<CATextLayer>,
}

/// Dimming opacity over the frozen frame outside the selection.
const DIM_ALPHA: f64 = 0.4;

/// Loupe magnification: on-screen points per source pixel.
const LOUPE_ZOOM_PT: f64 = 8.0;
/// Loupe sample side, in source pixels (the loupe is square).
const LOUPE_SAMPLE_PX: f64 = 16.0;
/// Loupe side, in points.
const LOUPE_SIZE_PT: f64 = LOUPE_SAMPLE_PX * LOUPE_ZOOM_PT;
/// Gap between the cursor and the loupe corner.
const LOUPE_GAP_PT: f64 = 18.0;
/// Height of the HUD labels (axis coordinates, "W × H px"), in points.
const HUD_LABEL_H_PT: f64 = 20.0;

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
                loupe: CALayer::new(),
                loupe_grid: CAShapeLayer::new(),
                loupe_cross: CAShapeLayer::new(),
                axis_lines: CAShapeLayer::new(),
                axis_under: CAShapeLayer::new(),
                x_label: CATextLayer::new(),
                y_label: CATextLayer::new(),
                size_label: CATextLayer::new(),
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

        #[unsafe(method(mouseMoved:))]
        fn __mouse_moved(&self, event: &NSEvent) {
            self.handle_mouse_moved(event);
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
        // Strip frames must follow the cursor instantly, without Core
        // Animation's implicit transitions.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.update_dim_inner(hole);
        CATransaction::commit();
    }

    fn update_dim_inner(&self, hole: Option<CGRect>) {
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

    /// Style the HUD layers and hang them into the layer tree (called once,
    /// on the first attach).
    fn setup_hud(&self) {
        let ivars = self.ivars();

        let loupe = &ivars.loupe;
        loupe.setMasksToBounds(true);
        loupe.setBorderWidth(1.0);
        let border = NSColor::whiteColor().colorWithAlphaComponent(0.9);
        loupe.setBorderColor(Some(&border.CGColor()));
        loupe.setContentsScale(ivars.scale.get());
        // Magnified source pixels must stay sharp squares.
        loupe.setMagnificationFilter(unsafe { kCAFilterNearest });
        loupe.setHidden(true);

        let grid = &ivars.loupe_grid;
        grid.setFrame(CGRect::new(
            CGPoint::ZERO,
            objc2_core_foundation::CGSize::new(LOUPE_SIZE_PT, LOUPE_SIZE_PT),
        ));
        let grid_color = NSColor::grayColor().colorWithAlphaComponent(0.35);
        grid.setStrokeColor(Some(&grid_color.CGColor()));
        grid.setLineWidth(0.5);
        grid.setContentsScale(ivars.scale.get());
        let grid_path = CGMutablePath::new();
        let cells = (LOUPE_SIZE_PT / LOUPE_ZOOM_PT) as i64;
        for i in 1..cells {
            let p = i as f64 * LOUPE_ZOOM_PT;
            // SAFETY: plain C path builders with a null transform.
            unsafe {
                CGMutablePath::move_to_point(Some(&grid_path), std::ptr::null(), p, 0.0);
                CGMutablePath::add_line_to_point(Some(&grid_path), std::ptr::null(), p, LOUPE_SIZE_PT);
                CGMutablePath::move_to_point(Some(&grid_path), std::ptr::null(), 0.0, p);
                CGMutablePath::add_line_to_point(Some(&grid_path), std::ptr::null(), LOUPE_SIZE_PT, p);
            }
        }
        let grid_ref: &CGPath = &grid_path;
        grid.setPath(Some(grid_ref));
        loupe.addSublayer(grid);

        // Crosshair through the center cell = the pixel under the cursor.
        let cross = &ivars.loupe_cross;
        cross.setFrame(CGRect::new(
            CGPoint::ZERO,
            objc2_core_foundation::CGSize::new(LOUPE_SIZE_PT, LOUPE_SIZE_PT),
        ));
        let cell_center = LOUPE_SAMPLE_PX / 2.0 * LOUPE_ZOOM_PT + LOUPE_ZOOM_PT / 2.0;
        // The contents image is drawn top-down; layer coordinates grow up.
        let cross_y = LOUPE_SIZE_PT - cell_center;
        let cross_path = CGMutablePath::new();
        // SAFETY: plain C path builders with a null transform.
        unsafe {
            CGMutablePath::move_to_point(Some(&cross_path), std::ptr::null(), 0.0, cross_y);
            CGMutablePath::add_line_to_point(Some(&cross_path), std::ptr::null(), LOUPE_SIZE_PT, cross_y);
            CGMutablePath::move_to_point(Some(&cross_path), std::ptr::null(), cell_center, 0.0);
            CGMutablePath::add_line_to_point(Some(&cross_path), std::ptr::null(), cell_center, LOUPE_SIZE_PT);
        }
        let cross_ref: &CGPath = &cross_path;
        cross.setPath(Some(cross_ref));
        cross.setStrokeColor(Some(&NSColor::redColor().CGColor()));
        cross.setLineWidth(1.0);
        cross.setContentsScale(ivars.scale.get());
        loupe.addSublayer(cross);

        let axis = &ivars.axis_lines;
        let axis_color = NSColor::whiteColor().colorWithAlphaComponent(0.9);
        axis.setStrokeColor(Some(&axis_color.CGColor()));
        axis.setLineWidth(0.75);
        axis.setContentsScale(ivars.scale.get());
        axis.setHidden(true);
        ivars.root_layer.addSublayer(axis);
        // Dark hairline under the white one: visible on any background.
        let under = &ivars.axis_under;
        let under_color = NSColor::blackColor().colorWithAlphaComponent(0.6);
        under.setStrokeColor(Some(&under_color.CGColor()));
        under.setLineWidth(1.5);
        under.setContentsScale(ivars.scale.get());
        under.setHidden(true);
        ivars.root_layer.addSublayer(under);
        // The loupe rides above the axis lines, its grid and crosshair on top.
        ivars.root_layer.addSublayer(loupe);

        let font = NSFont::monospacedDigitSystemFontOfSize_weight(11.0, 0.4);
        // SAFETY: NSFont is an ObjC object at runtime; CATextLayer.font
        // expects it as CFType.
        let as_cf: &CFType = unsafe { &*(std::ptr::from_ref(&*font).cast::<CFType>()) };
        for label in [&ivars.x_label, &ivars.y_label, &ivars.size_label] {
            // SAFETY: plain property setters.
            unsafe { label.setFont(Some(as_cf)) };
            label.setFontSize(11.0);
            label.setForegroundColor(Some(&NSColor::whiteColor().CGColor()));
            let bg = NSColor::blackColor().colorWithAlphaComponent(0.65);
            label.setBackgroundColor(Some(&bg.CGColor()));
            label.setCornerRadius(3.0);
            label.setMasksToBounds(true);
            label.setAlignmentMode(unsafe { kCAAlignmentCenter });
            label.setContentsScale(ivars.scale.get());
            label.setHidden(true);
            ivars.root_layer.addSublayer(label);
        }
    }

    /// Replace the text of a CATextLayer-backed HUD label.
    fn set_label_text(layer: &CATextLayer, text: &str) {
        let s = NSString::from_str(text);
        // SAFETY: NSString is an ObjC object at runtime; CATextLayer.string
        // expects it as AnyObject.
        let as_any: &AnyObject = unsafe { &*(std::ptr::from_ref(&*s).cast::<AnyObject>()) };
        // SAFETY: plain string setter.
        unsafe { layer.setString(Some(as_any)) };
    }

    /// Label width that fits an 11pt monospaced-digit text.
    fn label_width(text: &str) -> f64 {
        text.chars().count() as f64 * 7.2 + 14.0
    }

    /// Reposition the HUD for the cursor. `selection_px` is the live pixel
    /// size of the selection during a drag; `None` means idle, where the
    /// guides show the cursor coordinates instead.
    fn update_hud(&self, cursor: CGPoint, selection_px: Option<(f64, f64)>) {
        // The loupe and guides must track the cursor instantly: without this,
        // Core Animation tweens every frame and contents change over ~0.25s
        // and the loupe visibly lags behind.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.update_hud_inner(cursor, selection_px);
        CATransaction::commit();
    }

    fn update_hud_inner(&self, cursor: CGPoint, selection_px: Option<(f64, f64)>) {
        let ivars = self.ivars();
        let screen = ivars.screen_frame.get().size;
        let scale = ivars.scale.get();
        let (img_w, img_h) = ivars.image_size_px.get();
        if img_w == 0 || img_h == 0 {
            return;
        }
        let (img_w, img_h) = (img_w as f64, img_h as f64);
        let dragging = selection_px.is_some();

        // Sample of the T0 bitmap around the cursor in pixel space, kept
        // fully inside the image; the cursor pixel stays the center cell.
        let half = LOUPE_SAMPLE_PX / 2.0;
        let px = (cursor.x * scale).round().clamp(half, img_w - half);
        let py = ((screen.height - cursor.y) * scale).round().clamp(half, img_h - half);
        let sample = CGRect::new(
            CGPoint::new(px - half, py - half),
            objc2_core_foundation::CGSize::new(LOUPE_SAMPLE_PX, LOUPE_SAMPLE_PX),
        );
        let loupe = &ivars.loupe;
        let image = ivars.image.borrow();
        if let Some(image) = image.as_ref() {
            if let Some(cropped) = CGImage::with_image_in_rect(Some(image), sample) {
                // SAFETY: CGImage is an ObjC object at runtime; CALayer.contents
                // expects it as AnyObject.
                let as_any: &AnyObject =
                    unsafe { &*(std::ptr::from_ref(&*cropped).cast::<AnyObject>()) };
                // SAFETY: contents setter with a valid image object.
                unsafe { loupe.setContents(Some(as_any)) };
            }
        }
        drop(image);

        // Place the loupe by the cursor, flipping at the screen edges.
        let lx = if cursor.x + LOUPE_GAP_PT + LOUPE_SIZE_PT <= screen.width {
            cursor.x + LOUPE_GAP_PT
        } else {
            cursor.x - LOUPE_GAP_PT - LOUPE_SIZE_PT
        };
        let ly = if cursor.y - LOUPE_GAP_PT - LOUPE_SIZE_PT >= 0.0 {
            cursor.y - LOUPE_GAP_PT - LOUPE_SIZE_PT
        } else {
            cursor.y + LOUPE_GAP_PT
        };
        loupe.setFrame(CGRect::new(
            CGPoint::new(lx, ly),
            objc2_core_foundation::CGSize::new(LOUPE_SIZE_PT, LOUPE_SIZE_PT),
        ));
        loupe.setHidden(false);

        // "W × H px" next to the loupe while dragging.
        let size_label = &ivars.size_label;
        if let Some((w, h)) = selection_px {
            let text = format!("{} × {} px", w.round() as i64, h.round() as i64);
            Self::set_label_text(size_label, &text);
            let ty = if ly >= HUD_LABEL_H_PT + 6.0 {
                ly - HUD_LABEL_H_PT - 6.0
            } else {
                ly + LOUPE_SIZE_PT + 6.0
            };
            size_label.setFrame(CGRect::new(
                CGPoint::new(lx, ty),
                objc2_core_foundation::CGSize::new(Self::label_width(&text), HUD_LABEL_H_PT),
            ));
            size_label.setHidden(false);
        } else {
            size_label.setHidden(true);
        }

        // Idle: full-screen guides through the cursor with coordinate labels.
        // Drag: guides off — the rectangle is feedback enough.
        if dragging {
            ivars.axis_under.setHidden(true);
            ivars.axis_lines.setHidden(true);
            ivars.x_label.setHidden(true);
            ivars.y_label.setHidden(true);
        } else {
            let path = CGMutablePath::new();
            // SAFETY: plain C path builders with a null transform.
            unsafe {
                CGMutablePath::move_to_point(Some(&path), std::ptr::null(), cursor.x, 0.0);
                CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), cursor.x, screen.height);
                CGMutablePath::move_to_point(Some(&path), std::ptr::null(), 0.0, cursor.y);
                CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), screen.width, cursor.y);
            }
            let path_ref: &CGPath = &path;
            ivars.axis_under.setPath(Some(path_ref));
            ivars.axis_under.setHidden(false);
            ivars.axis_lines.setPath(Some(path_ref));
            ivars.axis_lines.setHidden(false);

            // The label coordinates are the real cursor pixel (not the
            // sample-clamped ones).
            let x_text = format!("{}", (cursor.x * scale).round() as i64);
            Self::set_label_text(&ivars.x_label, &x_text);
            let xw = Self::label_width(&x_text);
            let xx = if cursor.x + 6.0 + xw <= screen.width {
                cursor.x + 6.0
            } else {
                cursor.x - xw - 6.0
            };
            ivars.x_label.setFrame(CGRect::new(
                CGPoint::new(xx, screen.height - HUD_LABEL_H_PT - 6.0),
                objc2_core_foundation::CGSize::new(xw, HUD_LABEL_H_PT),
            ));
            ivars.x_label.setHidden(false);

            let y_text = format!("{}", ((screen.height - cursor.y) * scale).round() as i64);
            Self::set_label_text(&ivars.y_label, &y_text);
            let yw = Self::label_width(&y_text);
            let yy = if cursor.y + 6.0 + HUD_LABEL_H_PT <= screen.height {
                cursor.y + 6.0
            } else {
                cursor.y - HUD_LABEL_H_PT - 6.0
            };
            ivars.y_label.setFrame(CGRect::new(
                CGPoint::new(screen.width - yw - 6.0, yy),
                objc2_core_foundation::CGSize::new(yw, HUD_LABEL_H_PT),
            ));
            ivars.y_label.setHidden(false);
        }
    }

    /// Cursor moved without a button: guides + loupe in idle mode.
    fn handle_mouse_moved(&self, event: &NSEvent) {
        let ivars = self.ivars();
        if !ivars.active.get() || ivars.state.get() != OverlayState::Idle {
            return;
        }
        let point = crate::capture::geometry::clamp_point(
            self.local_point(event),
            CGRect::new(CGPoint::ZERO, ivars.screen_frame.get().size),
        );
        ivars.mouse.set(point);
        self.update_hud(point, None);
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
                self.setup_hud();
                view_layer.addSublayer(root);
            }
        }
        ivars.root_layer.setFrame(CGRect::new(CGPoint::ZERO, screen_frame.size));

        // Frame layer: the frozen T0 bitmap, 1:1 with the display pixels.
        let frame_layer = &ivars.frame_layer;
        frame_layer.setContentsScale(scale);
        frame_layer.setFrame(CGRect::new(CGPoint::ZERO, screen_frame.size));
        if let Some(image) = ivars.image.borrow().as_ref() {
            let image: &CGImage = image;
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
        self.update_hud(point, Some((0.0, 0.0)));
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
        let pixel_rect = crate::capture::geometry::selection_to_pixel_rect(
            rect,
            ivars.screen_frame.get().size,
            ivars.scale.get(),
        );
        self.update_hud(point, Some((pixel_rect.size.width, pixel_rect.size.height)));
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
        ivars.loupe.setHidden(true);
        ivars.size_label.setHidden(true);
        ivars.axis_under.setHidden(true);
        ivars.axis_lines.setHidden(true);
        ivars.x_label.setHidden(true);
        ivars.y_label.setHidden(true);
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
