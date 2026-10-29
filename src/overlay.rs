//! Frozen-frame overlay: a full-screen window over the captured display
//! showing the T0 frame, with selection UI layers on top. Created once at
//! app start and reused for every capture.

use std::cell::{Cell, RefCell};

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{
    define_class, extern_methods, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSEvent, NSFont, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CFType, CGPoint, CGRect};
use objc2_core_graphics::{
    CGBitmapContextCreateImage, CGColor, CGColorSpace, CGImage, CGImageAlphaInfo, CGMutablePath,
    CGPath, CGContext,
};
use objc2_foundation::{NSNumber, NSString};
use objc2_quartz_core::{
    kCAAlignmentCenter, kCAFilterNearest, kCAMediaTimingFunctionEaseInEaseOut, CABasicAnimation,
    CACurrentMediaTime, CALayer, CAMediaTiming, CAMediaTimingFunction, CAShapeLayer, CATextLayer,
    CATransaction,
};

use crate::capture::T0Capture;

// The shielding window level sits above the menu bar and everything else.
extern "C" {
    fn CGShieldingWindowLevel() -> i64;
}

// The plain CoreGraphics bitmap-context constructor (objc2-core-graphics
// only wraps the block-based adaptive variant).
extern "C-unwind" {
    fn CGBitmapContextCreate(
        data: *mut std::ffi::c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: &CGColorSpace,
        bitmap_info: u32,
    ) -> *mut CGContext;
}

/// The frozen frame downsampled to logical pixels (one Retina pixel pair
/// becomes one unit): the loupe magnifies UI pixels instead of physical
/// ones, and the axis guides sample content brightness from `buf`.
struct FrameSampler {
    /// Kept alive alongside the image: the exported image references the
    /// context's buffer.
    #[allow(dead_code)]
    ctx: CFRetained<CGContext>,
    image: CFRetained<CGImage>,
    /// Backing bytes of `ctx`; the context does not own them.
    buf: Vec<u8>,
    width: usize,
    height: usize,
    /// Physical pixels per logical pixel (backing scale).
    factor: usize,
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
    /// Full-screen guide lines through the cursor (idle only): a white
    /// outline layer under a thin black core layer, Monosnap-style. The pair
    /// reads on any background and never flickers, unlike content-sampled
    /// coloring.
    axis_v_dark: Retained<CAShapeLayer>,
    axis_v_light: Retained<CAShapeLayer>,
    axis_h_dark: Retained<CAShapeLayer>,
    axis_h_light: Retained<CAShapeLayer>,
    /// Readout pinned under the loupe: cursor coordinates while idle, the
    /// selection size while dragging (logical pixels).
    size_label: Retained<CATextLayer>,
    /// Red mode border along the display perimeter.
    border: Retained<CAShapeLayer>,
    /// The logical-pixel copy of the frozen frame, alive while the mode is
    /// active (the context and its backing bytes must live together).
    sampler: RefCell<Option<FrameSampler>>,
}

/// Dimming opacity over the frozen frame outside the selection.
const DIM_ALPHA: f64 = 0.4;

/// Loupe magnification: on-screen points per logical (UI) pixel.
const LOUPE_ZOOM_PT: f64 = 10.0;
/// Loupe sample side, in logical pixels (the loupe is square).
const LOUPE_SAMPLE_PX: f64 = 14.0;
/// Brightness (0..255 per channel) above which content counts as light.
/// Used by the sampler tests; production guides are constant two-tone.
#[allow(dead_code)]
const LIGHT_CONTENT_THRESHOLD: u32 = 128;
/// Guide line widths, in points: white outline underneath, black core on
/// top.
const AXIS_OUTLINE_W_PT: f64 = 1.5;
const AXIS_CORE_W_PT: f64 = 0.75;
/// Loupe side, in points.
const LOUPE_SIZE_PT: f64 = LOUPE_SAMPLE_PX * LOUPE_ZOOM_PT;
/// Gap between the cursor and the loupe corner.
const LOUPE_GAP_PT: f64 = 18.0;
/// Height of the HUD readout bar under the loupe, in points. The text layer
/// draws from the top; 18pt for a 14pt font leaves near-equal gaps above
/// and below the digits.
const HUD_LABEL_H_PT: f64 = 18.0;
/// Red mode border along the display perimeter, in points.
const BORDER_W_PT: f64 = 4.0;
/// Opacity floor of the idle pulse.
const BORDER_MIN_OPACITY: f64 = 0.55;
const BORDER_FADE_IN_DUR: f64 = 0.25;
const BORDER_PULSE_DUR: f64 = 0.8;
const BORDER_FADE_OUT_DUR: f64 = 0.2;
/// Animation key of the idle pulse on the border layer.
const BORDER_PULSE_KEY: &str = "border-pulse";

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
                axis_v_dark: CAShapeLayer::new(),
                axis_v_light: CAShapeLayer::new(),
                axis_h_dark: CAShapeLayer::new(),
                axis_h_light: CAShapeLayer::new(),
                size_label: CATextLayer::new(),
                border: CAShapeLayer::new(),
                sampler: RefCell::new(None),
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
        // Crosshair through the center cell = the logical pixel under the
        // cursor (the contents image is drawn top-down; layer coords grow up).
        let cell_center = (LOUPE_SAMPLE_PX / 2.0 + 0.5) * LOUPE_ZOOM_PT;
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

        // Guides: white outline first, black cores on top of it.
        let outline = NSColor::whiteColor().colorWithAlphaComponent(0.9).CGColor();
        let core = NSColor::blackColor().colorWithAlphaComponent(0.9).CGColor();
        let axes: [(&Retained<CAShapeLayer>, &CGColor, f64); 4] = [
            (&ivars.axis_v_light, &outline, AXIS_OUTLINE_W_PT),
            (&ivars.axis_h_light, &outline, AXIS_OUTLINE_W_PT),
            (&ivars.axis_v_dark, &core, AXIS_CORE_W_PT),
            (&ivars.axis_h_dark, &core, AXIS_CORE_W_PT),
        ];
        for (axis, color, width) in axes {
            axis.setStrokeColor(Some(color));
            axis.setLineWidth(width);
            axis.setContentsScale(ivars.scale.get());
            axis.setHidden(true);
            ivars.root_layer.addSublayer(axis);
        }
        // The loupe rides above the axis lines, its grid and crosshair on top.
        ivars.root_layer.addSublayer(loupe);

        let font = NSFont::monospacedDigitSystemFontOfSize_weight(14.0, 0.4);
        // SAFETY: NSFont is an ObjC object at runtime; CATextLayer.font
        // expects it as CFType.
        let as_cf: &CFType = unsafe { &*(std::ptr::from_ref(&*font).cast::<CFType>()) };
        for label in [&ivars.size_label] {
            // SAFETY: plain property setters.
            unsafe { label.setFont(Some(as_cf)) };
            label.setFontSize(14.0);
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

        // Red mode border along the display perimeter, Monosnap-style. The
        // rect is inset by half the stroke so the line stays fully inside
        // the window. Invisible until show() fades it in.
        let border = &ivars.border;
        let inset = BORDER_W_PT / 2.0;
        let size = ivars.screen_frame.get().size;
        let border_path = CGMutablePath::new();
        // SAFETY: plain C path builders with a null transform.
        unsafe {
            CGMutablePath::move_to_point(Some(&border_path), std::ptr::null(), inset, inset);
            CGMutablePath::add_line_to_point(
                Some(&border_path),
                std::ptr::null(),
                size.width - inset,
                inset,
            );
            CGMutablePath::add_line_to_point(
                Some(&border_path),
                std::ptr::null(),
                size.width - inset,
                size.height - inset,
            );
            CGMutablePath::add_line_to_point(
                Some(&border_path),
                std::ptr::null(),
                inset,
                size.height - inset,
            );
            CGMutablePath::add_line_to_point(Some(&border_path), std::ptr::null(), inset, inset);
        }
        let border_ref: &CGPath = &border_path;
        border.setPath(Some(border_ref));
        border.setStrokeColor(Some(&NSColor::redColor().CGColor()));
        border.setFillColor(None);
        border.setLineWidth(BORDER_W_PT);
        border.setContentsScale(ivars.scale.get());
        border.setOpacity(0.0);
        // On top of every other HUD element.
        ivars.root_layer.addSublayer(border);
    }

    /// Arm the idle pulse on the border. The model opacity stays 1.0, so
    /// removing the pulse key drops the border back to full red while
    /// dragging.
    fn start_border_pulse(border: &CAShapeLayer) {
        let pulse = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
        let from = NSNumber::numberWithDouble(1.0);
        let to = NSNumber::numberWithDouble(BORDER_MIN_OPACITY);
        // SAFETY: opacity animations encode from/to values as NSNumber.
        unsafe {
            pulse.setFromValue(Some(&*(std::ptr::from_ref(&*from).cast::<AnyObject>())));
            pulse.setToValue(Some(&*(std::ptr::from_ref(&*to).cast::<AnyObject>())));
        }
        pulse.setDuration(BORDER_PULSE_DUR);
        pulse.setAutoreverses(true);
        pulse.setRepeatCount(f32::INFINITY);
        pulse.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(unsafe {
            kCAMediaTimingFunctionEaseInEaseOut
        })));
        // No overlap with the fade-in: the pulse starts only after the fade
        // completes.
        pulse.setBeginTime(CACurrentMediaTime() + BORDER_FADE_IN_DUR);
        border.addAnimation_forKey(&pulse, Some(&NSString::from_str(BORDER_PULSE_KEY)));
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

    /// Downsample the frozen frame to logical pixels (one backing-scale
    /// pixel group per unit). Returns None when the bitmap context could not
    /// be created; the HUD then works without the loupe.
    fn make_sampler(w_px: usize, h_px: usize, scale: f64, image: &CGImage) -> Option<FrameSampler> {
        let factor = (scale.round() as usize).max(1);
        let lw = (w_px / factor).max(1);
        let lh = (h_px / factor).max(1);
        let space = CGColorSpace::new_device_rgb()?;
        let mut buf = vec![0u8; lw * lh * 4];
        // SAFETY: plain CGBitmapContextCreate; buf is stored in the sampler
        // alongside the context whose buffer it backs.
        let ctx_ptr = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                lw,
                lh,
                8,
                lw * 4,
                &space,
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        };
        // SAFETY: ctx_ptr comes from a Create-rule constructor; on success we
        // take ownership of the +1 reference.
        let ctx = unsafe {
            std::ptr::NonNull::new(ctx_ptr).map(|p| CFRetained::from_raw(p))?
        };
        CGContext::draw_image(
            Some(&ctx),
            CGRect::new(
                CGPoint::ZERO,
                objc2_core_foundation::CGSize::new(lw as f64, lh as f64),
            ),
            Some(image),
        );
        let down = CGBitmapContextCreateImage(Some(&ctx))?;
        Some(FrameSampler {
            ctx,
            image: down,
            buf,
            width: lw,
            height: lh,
            factor,
        })
    }

    /// Mean per-channel brightness (0..255) of the content along a
    /// logical-pixel line through `at`, over the range `from..=to` along the
    /// line (rows for a vertical line, columns for a horizontal one).
    /// Production HUD uses constant two-tone guides; this stays for the
    /// sampler tests and possible future content-aware styling.
    #[allow(dead_code)]
    fn line_brightness_in(
        sampler: &FrameSampler,
        vertical: bool,
        at: usize,
        from: usize,
        to: usize,
    ) -> u32 {
        let (w, h) = (sampler.width, sampler.height);
        let buf = &sampler.buf;
        let dim = if vertical { h } else { w };
        let lo = from.min(dim.saturating_sub(1));
        let hi = to.min(dim.saturating_sub(1));
        let (lo, hi) = (lo.min(hi), hi.max(lo));
        let mut total: u64 = 0;
        let mut n: u64 = 0;
        if vertical {
            for v in lo..=hi {
                let i = (v * w + at) * 4;
                total += (buf[i] as u64) + (buf[i + 1] as u64) + (buf[i + 2] as u64);
                n += 1;
            }
        } else {
            for v in lo..=hi {
                let i = (at * w + v) * 4;
                total += (buf[i] as u64) + (buf[i + 1] as u64) + (buf[i + 2] as u64);
                n += 1;
            }
        }
        if n == 0 {
            return 0;
        }
        (total / (n * 3)) as u32
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
        let sampler = ivars.sampler.borrow();
        let Some(sampler) = sampler.as_ref() else {
            return;
        };
        let factor = sampler.factor as f64;
        let lw = sampler.width as f64;
        let lh = sampler.height as f64;
        let dragging = selection_px.is_some();

        // Logical-pixel sample around the cursor, kept fully inside the
        // bitmap; the cursor pixel stays the center cell.
        let half = LOUPE_SAMPLE_PX / 2.0;
        let px = (cursor.x * scale / factor).round().clamp(half, lw - half);
        let py = ((screen.height - cursor.y) * scale / factor)
            .round()
            .clamp(half, lh - half);
        let sample = CGRect::new(
            CGPoint::new(px - half, py - half),
            objc2_core_foundation::CGSize::new(LOUPE_SAMPLE_PX, LOUPE_SAMPLE_PX),
        );
        let loupe = &ivars.loupe;
        if let Some(cropped) = CGImage::with_image_in_rect(Some(&sampler.image), sample) {
            // SAFETY: CGImage is an ObjC object at runtime; CALayer.contents
            // expects it as AnyObject.
            let as_any: &AnyObject =
                unsafe { &*(std::ptr::from_ref(&*cropped).cast::<AnyObject>()) };
            // SAFETY: contents setter with a valid image object.
            unsafe { loupe.setContents(Some(as_any)) };
        }

        // Place the loupe by the cursor, flipping at the screen edges. The
        // origin snaps to whole points so the squares inside stay pinned to
        // the screen grid while the crop steps in whole pixels — otherwise
        // the picture visibly slides under the cursor in sub-pixel steps.
        let lx = if cursor.x + LOUPE_GAP_PT + LOUPE_SIZE_PT <= screen.width {
            cursor.x + LOUPE_GAP_PT
        } else {
            cursor.x - LOUPE_GAP_PT - LOUPE_SIZE_PT
        }
        .round();
        let ly = if cursor.y - LOUPE_GAP_PT - LOUPE_SIZE_PT >= 0.0 {
            cursor.y - LOUPE_GAP_PT - LOUPE_SIZE_PT
        } else {
            cursor.y + LOUPE_GAP_PT
        }
        .round();
        loupe.setFrame(CGRect::new(
            CGPoint::new(lx, ly),
            objc2_core_foundation::CGSize::new(LOUPE_SIZE_PT, LOUPE_SIZE_PT),
        ));
        loupe.setHidden(false);

        // Monosnap-style readout pinned under the loupe: cursor coordinates
        // while idle, the selection size while dragging. Logical pixels.
        let text = match selection_px {
            Some((w, h)) => format!("{} × {}", w.round() as i64, h.round() as i64),
            None => format!(
                "{} × {}",
                (cursor.x * scale / factor).round() as i64,
                ((screen.height - cursor.y) * scale / factor).round() as i64
            ),
        };
        let label = &ivars.size_label;
        Self::set_label_text(label, &text);
        let ty = if ly >= HUD_LABEL_H_PT + 6.0 {
            ly - HUD_LABEL_H_PT - 6.0
        } else {
            ly + LOUPE_SIZE_PT + 6.0
        };
        label.setFrame(CGRect::new(
            CGPoint::new(lx, ty),
            objc2_core_foundation::CGSize::new(LOUPE_SIZE_PT, HUD_LABEL_H_PT),
        ));
        label.setHidden(false);

        // Idle: full-screen guides through the cursor with coordinate labels.
        // Drag: guides off — the rectangle is feedback enough. The line
        // color adapts to the content underneath, so the old thin hairlines
        // stay readable on light and dark backgrounds alike.
        if dragging {
            ivars.axis_v_dark.setHidden(true);
            ivars.axis_v_light.setHidden(true);
            ivars.axis_h_dark.setHidden(true);
            ivars.axis_h_light.setHidden(true);
        } else {
            // One path per direction, shared by the outline and the core.
            let v_path = CGMutablePath::new();
            let h_path = CGMutablePath::new();
            // SAFETY: plain C path builders with a null transform.
            unsafe {
                CGMutablePath::move_to_point(Some(&v_path), std::ptr::null(), cursor.x, 0.0);
                CGMutablePath::add_line_to_point(Some(&v_path), std::ptr::null(), cursor.x, screen.height);
                CGMutablePath::move_to_point(Some(&h_path), std::ptr::null(), 0.0, cursor.y);
                CGMutablePath::add_line_to_point(Some(&h_path), std::ptr::null(), screen.width, cursor.y);
            }
            let v_ref: &CGPath = &v_path;
            let h_ref: &CGPath = &h_path;
            for layer in [&ivars.axis_v_dark, &ivars.axis_v_light] {
                layer.setPath(Some(v_ref));
                layer.setHidden(false);
            }
            for layer in [&ivars.axis_h_dark, &ivars.axis_h_light] {
                layer.setPath(Some(h_ref));
                layer.setHidden(false);
            }
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

        // Downsample the frame to logical pixels for the loupe and for the
        // axis-guide brightness sampling.
        let source = ivars.image.borrow();
        *ivars.sampler.borrow_mut() = source
            .as_ref()
            .and_then(|image| Self::make_sampler(w_px, h_px, scale, image));

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
            // This accessory app is not active when launched from a shell or
            // when another app is frontmost: NSApp.activate() alone is
            // silently denied without user-interaction attribution, the
            // window never becomes key, and Esc goes nowhere until the
            // first click activates the app. The deprecated
            // activateIgnoringOtherApps is still honored for this exact
            // case; the extra makeKeyAndOrderFront on the next runloop tick
            // locks the key status in after the activation lands.
            let app =
                NSApplication::sharedApplication(MainThreadMarker::new().expect("overlay on main thread"));
            // SAFETY: deprecated NSObject method with a BOOL argument.
            let _: () = unsafe { msg_send![&app, activateIgnoringOtherApps: true] };
            app.activate();
            window.makeKeyAndOrderFront(None);
            // SAFETY: NSObject delayed-perform scheduling; the selector is a
            // valid window method, the object argument is nil.
            let _: () = unsafe {
                msg_send![window,
                    performSelector: objc2::sel!(makeKeyAndOrderFront:),
                    withObject: None::<&AnyObject>,
                    afterDelay: 0.05
                ]
            };
        }

        // Border: reset invisible without animation, fade in, arm the idle
        // pulse (it starts only after the fade completes).
        let border = &ivars.border;
        border.removeAllAnimations();
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        border.setHidden(false);
        border.setOpacity(0.0);
        CATransaction::commit();
        CATransaction::begin();
        CATransaction::setAnimationDuration(BORDER_FADE_IN_DUR);
        border.setOpacity(1.0);
        CATransaction::commit();
        Self::start_border_pulse(border);

        let app = NSApplication::sharedApplication(MainThreadMarker::new().expect("overlay on main thread"));
        let key_state = ivars
            .window
            .borrow()
            .as_ref()
            .map(|w| w.isKeyWindow())
            .unwrap_or(false);
        eprintln!(
            "overlay: frozen {}x{} px on screen ({:.0},{:.0} {}x{}), app active: {}, key window: {}",
            w_px,
            h_px,
            screen_frame.origin.x,
            screen_frame.origin.y,
            screen_frame.size.width,
            screen_frame.size.height,
            app.isActive(),
            key_state
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
        // Drag: the border holds full red, no pulse.
        ivars
            .border
            .removeAnimationForKey(&NSString::from_str(BORDER_PULSE_KEY));
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
        // The HUD speaks logical pixels, like the loupe.
        let factor = ivars.scale.get().round().max(1.0);
        self.update_hud(
            point,
            Some((pixel_rect.size.width / factor, pixel_rect.size.height / factor)),
        );
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
        *ivars.sampler.borrow_mut() = None;
        ivars.loupe.setHidden(true);
        ivars.size_label.setHidden(true);
        ivars.axis_v_dark.setHidden(true);
        ivars.axis_v_light.setHidden(true);
        ivars.axis_h_dark.setHidden(true);
        ivars.axis_h_light.setHidden(true);

        // The border fades out; the window goes down in the fade's
        // completion block, so the frozen frame and the dimming linger for
        // the fade (a short freeze-frame flash). The guard keeps the window
        // up if the mode was re-entered during the fade.
        let border = &ivars.border;
        border.removeAnimationForKey(&NSString::from_str(BORDER_PULSE_KEY));
        let window = ivars.window.borrow().as_ref().map(|w| w.clone());
        let view = self.retain();
        CATransaction::begin();
        CATransaction::setAnimationDuration(BORDER_FADE_OUT_DUR);
        if let Some(window) = window {
            let on_fade = RcBlock::new(move || {
                if !view.ivars().active.get() {
                    window.orderOut(None);
                }
            });
            // SAFETY: the completion block runs on the main thread and only
            // touches retained ObjC handles owned by the closure.
            unsafe { CATransaction::setCompletionBlock(Some(&on_fade)) };
        }
        border.setOpacity(0.0);
        CATransaction::commit();

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A small synthetic frame via the same CG bitmap pipeline.
    fn synthetic_frame(w: usize, h: usize) -> CFRetained<CGImage> {
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                // Left half black, right half white.
                let v = if x < w / 2 { 0 } else { 255 };
                buf[i] = v;
                buf[i + 1] = v;
                buf[i + 2] = v;
                buf[i + 3] = 255;
            }
        }
        let space = CGColorSpace::new_device_rgb().expect("rgb colorspace");
        // SAFETY: buf outlives the context within this function.
        let ctx_ptr = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                &space,
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        };
        let ctx = unsafe {
            std::ptr::NonNull::new(ctx_ptr).map(|p| CFRetained::from_raw(p))
        }
        .expect("bitmap context");
        CGBitmapContextCreateImage(Some(&ctx)).expect("image from context")
    }

    #[test]
    fn sampler_downsamples_to_logical_pixels() {
        let image = synthetic_frame(64, 64);
        let sampler = OverlayView::make_sampler(64, 64, 2.0, &image).expect("sampler");
        assert_eq!((sampler.width, sampler.height, sampler.factor), (32, 32, 2));
        // Left half of the frame is black, right half white: a column in the
        // dark half reads dark, one in the light half reads light.
        let dark = OverlayView::line_brightness_in(&sampler, true, 4, 0, 31);
        let light = OverlayView::line_brightness_in(&sampler, true, 28, 0, 31);
        assert!(dark < LIGHT_CONTENT_THRESHOLD, "dark col: {dark}");
        assert!(light > LIGHT_CONTENT_THRESHOLD, "light col: {light}");
    }

    #[test]
    fn sampler_handles_scale_one() {
        let image = synthetic_frame(64, 64);
        let sampler = OverlayView::make_sampler(64, 64, 1.0, &image).expect("sampler");
        assert_eq!((sampler.width, sampler.height, sampler.factor), (64, 64, 1));
    }
}
