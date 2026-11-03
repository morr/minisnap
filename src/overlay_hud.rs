//! HUD of the frozen-frame overlay: the chrome layers over the frozen
//! frame (dimming strips, loupe, axis guides, coordinate readout, the red
//! mode border) and the logical-pixel frame sampler they read. Extracted
//! from `overlay.rs`, which keeps the session-side orchestration (window,
//! surfaces, target switching, click resolution).

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSColor, NSFont};
use objc2_core_foundation::{CFRetained, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorSpace, CGImage,
    CGImageAlphaInfo, CGMutablePath, CGPath, CGContext,
};
use objc2_foundation::{NSNumber, NSString};
use objc2_quartz_core::{
    kCAAlignmentCenter, kCAFilterNearest, kCAMediaTimingFunctionEaseInEaseOut, CABasicAnimation,
    CACurrentMediaTime, CALayer, CAMediaTiming, CAMediaTimingFunction, CAShapeLayer, CATextLayer,
    CATransaction,
};

/// The frozen frame downsampled to logical pixels (one Retina pixel pair
/// becomes one unit): the loupe magnifies UI pixels instead of physical
/// ones, and the axis guides sample content brightness from `buf`.
pub(crate) struct FrameSampler {
    /// Kept alive alongside the image: the exported image references the
    /// context's buffer.
    #[allow(dead_code)]
    ctx: CFRetained<CGContext>,
    image: CFRetained<CGImage>,
    /// Backing bytes of `ctx`; the context does not own them, so the buffer
    /// must outlive the exported image. Nothing reads it in production any
    /// more (the content-adaptive guide coloring is gone) — only the tests.
    #[allow(dead_code)]
    buf: Vec<u8>,
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Physical pixels per logical pixel (backing scale).
    pub(crate) factor: usize,
}

impl FrameSampler {
    /// Downsample the frozen frame to logical pixels (one backing-scale
    /// pixel group per unit). Returns None when the bitmap context could not
    /// be created; the HUD then works without the loupe.
    pub(crate) fn new(w_px: usize, h_px: usize, scale: f64, image: &CGImage) -> Option<Self> {
        let factor = (scale.round() as usize).max(1);
        let lw = (w_px / factor).max(1);
        let lh = (h_px / factor).max(1);
        let space = CGColorSpace::new_device_rgb()?;
        let mut buf = vec![0u8; lw * lh * 4];
        // SAFETY: plain CGBitmapContextCreate; buf is stored in the sampler
        // alongside the context whose buffer it backs.
        let ctx = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                lw,
                lh,
                8,
                lw * 4,
                Some(&*space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }?;
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

    pub(crate) fn image(&self) -> &CGImage {
        &self.image
    }
}

/// Dimming opacity over the frozen frame outside the selection (the veil
/// windows over other displays share the value).
pub(crate) const DIM_ALPHA: f64 = 0.4;

/// Loupe magnification: on-screen points per logical (UI) pixel.
const LOUPE_ZOOM_PT: f64 = 7.0;
/// Loupe sample side, in logical pixels (the loupe is square). Odd on
/// purpose: the cursor pixel is then the exact middle cell, with the same
/// whole number of cells on every side of the crosshair. 21 × 7pt keeps
/// the on-screen cell at a whole 7pt (crisp grid at any backing scale).
const LOUPE_SAMPLE_PX: f64 = 21.0;
/// Guide line widths, in points: white outline underneath, black core on
/// top.
const AXIS_OUTLINE_W_PT: f64 = 1.5;
const AXIS_CORE_W_PT: f64 = 0.75;
/// Loupe side, in points.
const LOUPE_SIZE_PT: f64 = LOUPE_SAMPLE_PX * LOUPE_ZOOM_PT;
/// Loupe frame rings, in points: a white outline ring with a black core
/// ring on top — the same two-tone scheme as the axis guides — so the
/// loupe edge reads on light and dark content alike.
const LOUPE_FRAME_OUTLINE_W_PT: f64 = 3.0;
const LOUPE_FRAME_CORE_W_PT: f64 = 1.5;
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
/// Animation key of the idle pulse on the border layer.
const BORDER_PULSE_KEY: &str = "border-pulse";

/// All overlay chrome above the frozen frame: the dimming strips, the
/// loupe with its grid, crosshair and two-tone frame, the axis guides, the
/// coordinate readout, the red mode border and the logical-pixel frame
/// sampler they read. The overlay's root layer keeps exactly two children
/// — the frame layer and this HUD's container — so adding a chrome layer
/// never touches the presentation path again.
pub(crate) struct Hud {
    /// Carries every chrome layer; hangs into the root layer on `attach`.
    container: Retained<CALayer>,
    /// Four black strips around the selection rectangle (top, bottom, left,
    /// right) forming the dimming with a "hole" for the selection.
    dim_strips: RefCell<[Retained<CALayer>; 4]>,
    /// Magnifier: nearest-neighbor sample of the T0 bitmap by the cursor.
    loupe: Retained<CALayer>,
    /// Pixel grid over the loupe sample.
    loupe_grid: Retained<CAShapeLayer>,
    /// Crosshair through the loupe's center (cursor-pixel) cell.
    loupe_cross: Retained<CAShapeLayer>,
    /// Two-tone frame over the loupe edge: a white outline ring under a
    /// thin black core ring, so the boundary reads on any content.
    loupe_frame_light: Retained<CAShapeLayer>,
    loupe_frame_dark: Retained<CAShapeLayer>,
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
    /// Target screen size and backing scale, recorded by `present`; `dim`
    /// and `update` read them every frame.
    screen: Cell<CGSize>,
    scale: Cell<f64>,
}

impl Hud {
    /// Build and style every chrome layer. Layers stay hidden until the
    /// presentation shows them; sharpness (`contentsScale`) and the border
    /// geometry are applied by `attach`/`present`, which know the display.
    pub(crate) fn new() -> Hud {
        let container = CALayer::new();

        let dim_strips = [CALayer::new(), CALayer::new(), CALayer::new(), CALayer::new()];
        let dim_color = NSColor::blackColor().colorWithAlphaComponent(DIM_ALPHA);
        for strip in &dim_strips {
            strip.setBackgroundColor(Some(&dim_color.CGColor()));
        }

        let loupe = CALayer::new();
        loupe.setMasksToBounds(true);
        // Magnified source pixels must stay sharp squares.
        loupe.setMagnificationFilter(unsafe { kCAFilterNearest });
        loupe.setHidden(true);

        let grid = CAShapeLayer::new();
        grid.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(LOUPE_SIZE_PT, LOUPE_SIZE_PT)));
        let grid_color = NSColor::grayColor().colorWithAlphaComponent(0.35);
        grid.setStrokeColor(Some(&grid_color.CGColor()));
        grid.setLineWidth(0.5);
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

        // Crosshair through the center cell = the logical pixel under the
        // cursor (the contents image is drawn top-down; layer coords grow
        // up). With the odd sample side the cell center sits at exactly
        // half the loupe, so the crosshair is equidistant from all edges.
        let cross = CAShapeLayer::new();
        cross.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(LOUPE_SIZE_PT, LOUPE_SIZE_PT)));
        let cell_center = ((LOUPE_SAMPLE_PX / 2.0).floor() + 0.5) * LOUPE_ZOOM_PT;
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

        // Two-tone frame over the loupe edge, on top of the grid and the
        // crosshair: the white outline reads on dark content, the black
        // core on light. The band sits fully inside the clipped bounds.
        let frame_inset = LOUPE_FRAME_OUTLINE_W_PT / 2.0;
        let frame_path = CGMutablePath::new();
        // SAFETY: plain C path builders with a null transform.
        unsafe {
            CGMutablePath::move_to_point(
                Some(&frame_path),
                std::ptr::null(),
                frame_inset,
                frame_inset,
            );
            CGMutablePath::add_line_to_point(
                Some(&frame_path),
                std::ptr::null(),
                LOUPE_SIZE_PT - frame_inset,
                frame_inset,
            );
            CGMutablePath::add_line_to_point(
                Some(&frame_path),
                std::ptr::null(),
                LOUPE_SIZE_PT - frame_inset,
                LOUPE_SIZE_PT - frame_inset,
            );
            CGMutablePath::add_line_to_point(
                Some(&frame_path),
                std::ptr::null(),
                frame_inset,
                LOUPE_SIZE_PT - frame_inset,
            );
            CGMutablePath::close_subpath(Some(&frame_path));
        }
        let frame_ref: &CGPath = &frame_path;
        let white = NSColor::whiteColor().colorWithAlphaComponent(0.9).CGColor();
        let black = NSColor::blackColor().colorWithAlphaComponent(0.9).CGColor();
        let loupe_frame_light = CAShapeLayer::new();
        let loupe_frame_dark = CAShapeLayer::new();
        for (ring, color, width) in [
            (&loupe_frame_light, &white, LOUPE_FRAME_OUTLINE_W_PT),
            (&loupe_frame_dark, &black, LOUPE_FRAME_CORE_W_PT),
        ] {
            ring.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(LOUPE_SIZE_PT, LOUPE_SIZE_PT)));
            ring.setPath(Some(frame_ref));
            ring.setStrokeColor(Some(color));
            ring.setLineWidth(width);
            // A closed rect path would otherwise fill with the default
            // opaque black.
            ring.setFillColor(None);
        }

        // Guides: white outline first, black cores on top of it.
        let outline = NSColor::whiteColor().colorWithAlphaComponent(0.9).CGColor();
        let core = NSColor::blackColor().colorWithAlphaComponent(0.9).CGColor();
        let axis_v_light = CAShapeLayer::new();
        let axis_h_light = CAShapeLayer::new();
        let axis_v_dark = CAShapeLayer::new();
        let axis_h_dark = CAShapeLayer::new();
        let axes: [(&Retained<CAShapeLayer>, &Retained<CGColor>, f64); 4] = [
            (&axis_v_light, &outline, AXIS_OUTLINE_W_PT),
            (&axis_h_light, &outline, AXIS_OUTLINE_W_PT),
            (&axis_v_dark, &core, AXIS_CORE_W_PT),
            (&axis_h_dark, &core, AXIS_CORE_W_PT),
        ];
        for (axis, color, width) in axes {
            axis.setStrokeColor(Some(color));
            axis.setLineWidth(width);
            axis.setHidden(true);
        }

        let size_label = CATextLayer::new();
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(14.0, 0.4);
        // SAFETY: NSFont is an ObjC object at runtime; CATextLayer.font
        // expects it as CFType.
        let as_cf: &CFType = unsafe { &*(std::ptr::from_ref(&*font).cast::<CFType>()) };
        // SAFETY: plain property setters.
        unsafe { size_label.setFont(Some(as_cf)) };
        size_label.setFontSize(14.0);
        size_label.setForegroundColor(Some(&NSColor::whiteColor().CGColor()));
        let bg = NSColor::blackColor().colorWithAlphaComponent(0.65);
        size_label.setBackgroundColor(Some(&bg.CGColor()));
        size_label.setCornerRadius(3.0);
        size_label.setMasksToBounds(true);
        size_label.setAlignmentMode(unsafe { kCAAlignmentCenter });
        size_label.setHidden(true);

        // Red mode border along the display perimeter, Monosnap-style: the
        // stroke color and geometry style once, the path rebuilt per target
        // (it follows the display size). Invisible until present() fades it
        // in.
        let border = CAShapeLayer::new();
        border.setPath(None);
        border.setStrokeColor(Some(&NSColor::redColor().CGColor()));
        border.setFillColor(None);
        border.setLineWidth(BORDER_W_PT);
        border.setOpacity(0.0);

        Hud {
            container,
            dim_strips: RefCell::new(dim_strips),
            loupe,
            loupe_grid: grid,
            loupe_cross: cross,
            loupe_frame_light,
            loupe_frame_dark,
            axis_v_dark,
            axis_v_light,
            axis_h_dark,
            axis_h_light,
            size_label,
            border,
            sampler: RefCell::new(None),
            screen: Cell::new(CGSize::new(0.0, 0.0)),
            scale: Cell::new(1.0),
        }
    }

    /// Hang the HUD into the overlay's layer tree (once, on the first
    /// show). Children keep the chrome's z-order: dimming strips, axis
    /// guides, loupe (with its grid, crosshair and frame rings), readout,
    /// border on top.
    pub(crate) fn attach(&self, root: &CALayer, scale: f64) {
        for strip in self.dim_strips.borrow().iter() {
            self.container.addSublayer(strip);
        }
        for axis in [&self.axis_v_light, &self.axis_h_light, &self.axis_v_dark, &self.axis_h_dark] {
            self.container.addSublayer(axis);
        }
        // The loupe rides above the axis lines; its chrome hangs into it as
        // sublayers, bottom-up: the grid, the crosshair over it, then the
        // frame rings with the white outline under the black core.
        self.container.addSublayer(&self.loupe);
        for chrome in [&self.loupe_grid, &self.loupe_cross, &self.loupe_frame_light, &self.loupe_frame_dark] {
            self.loupe.addSublayer(chrome);
        }
        self.container.addSublayer(&self.size_label);
        self.container.addSublayer(&self.border);
        root.addSublayer(&self.container);
        self.apply_contents_scale(scale);
    }

    /// Sharpness of every chrome layer follows the target display's backing
    /// scale (displays differ in scale).
    fn apply_contents_scale(&self, scale: f64) {
        for strip in self.dim_strips.borrow().iter() {
            strip.setContentsScale(scale);
        }
        self.loupe.setContentsScale(scale);
        self.loupe_grid.setContentsScale(scale);
        self.loupe_cross.setContentsScale(scale);
        self.loupe_frame_light.setContentsScale(scale);
        self.loupe_frame_dark.setContentsScale(scale);
        self.axis_v_dark.setContentsScale(scale);
        self.axis_v_light.setContentsScale(scale);
        self.axis_h_dark.setContentsScale(scale);
        self.axis_h_light.setContentsScale(scale);
        self.size_label.setContentsScale(scale);
        self.border.setContentsScale(scale);
    }

    /// Point the whole HUD at the presented target: sharpness follows the
    /// display's backing scale, the border path and the container follow its
    /// size, the sampler is rebuilt from the new frame, the border fade-in
    /// and idle pulse restart. Called by the shared presentation path on
    /// show and on every target switch.
    pub(crate) fn present(&self, scale: f64, screen: CGSize, frame: Option<(&CGImage, usize, usize)>) {
        self.screen.set(screen);
        self.scale.set(scale);

        // Sharpness and geometry swap without implicit animations: Core
        // Animation would otherwise tween them behind the frame swap.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.container.setFrame(CGRect::new(CGPoint::ZERO, screen));
        self.apply_contents_scale(scale);
        self.rebuild_border_path();
        CATransaction::commit();

        // Downsample the new frame to logical pixels for the loupe.
        *self.sampler.borrow_mut() =
            frame.and_then(|(image, w_px, h_px)| FrameSampler::new(w_px, h_px, scale, image));

        // Border: reset invisible without animation, fade in, arm the idle
        // pulse (it starts only after the fade completes).
        let border = &self.border;
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
    }

    /// Drop the logical-pixel frame copy (end of the mode): the sampler
    /// holds tens of megabytes on a 5K display.
    pub(crate) fn drop_sampler(&self) {
        *self.sampler.borrow_mut() = None;
    }

    /// Hide the idle HUD: loupe, readout label, the four axis guides.
    /// Shared by the target presentation (idle reset) and finish().
    pub(crate) fn hide(&self) {
        self.loupe.setHidden(true);
        self.size_label.setHidden(true);
        self.axis_v_dark.setHidden(true);
        self.axis_v_light.setHidden(true);
        self.axis_h_dark.setHidden(true);
        self.axis_h_light.setHidden(true);
    }

    /// Remove the idle pulse from the border: dragging holds it at full
    /// red, and finish() tears the mode down without animations.
    pub(crate) fn stop_border_pulse(&self) {
        self.border
            .removeAnimationForKey(&NSString::from_str(BORDER_PULSE_KEY));
    }

    /// Place the four dimming strips around `hole` (display-local points);
    /// `None` hides them (no active selection yet).
    pub(crate) fn dim(&self, hole: Option<CGRect>) {
        // Strip frames must follow the cursor instantly, without Core
        // Animation's implicit transitions.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.dim_inner(hole);
        CATransaction::commit();
    }

    fn dim_inner(&self, hole: Option<CGRect>) {
        let strips = self.dim_strips.borrow();
        let screen = self.screen.get();
        let Some(rect) = hole else {
            for strip in strips.iter() {
                strip.setHidden(true);
            }
            return;
        };
        let rect = rect.standardize();
        let w = screen.width;
        let h = screen.height;
        let frames = [
            // top: from the hole's top edge up to the screen top
            CGRect::new(CGPoint::new(0.0, rect.max().y), CGSize::new(w, h - rect.max().y)),
            // bottom
            CGRect::new(CGPoint::ZERO, CGSize::new(w, rect.min().y)),
            // left
            CGRect::new(CGPoint::new(0.0, rect.min().y), CGSize::new(rect.min().x, rect.size.height)),
            // right
            CGRect::new(
                CGPoint::new(rect.max().x, rect.min().y),
                CGSize::new(w - rect.max().x, rect.size.height),
            ),
        ];
        for (strip, frame) in strips.iter().zip(frames) {
            strip.setHidden(false);
            strip.setFrame(frame);
        }
    }

    /// Reposition the HUD for the cursor. `selection_px` is the live pixel
    /// size of the selection during a drag; `None` means idle, where the
    /// guides show the cursor coordinates instead.
    pub(crate) fn update(&self, cursor: CGPoint, selection_px: Option<(f64, f64)>) {
        // The loupe and guides must track the cursor instantly: without this,
        // Core Animation tweens every frame and contents change over ~0.25s
        // and the loupe visibly lags behind.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.update_inner(cursor, selection_px);
        CATransaction::commit();
    }

    fn update_inner(&self, cursor: CGPoint, selection_px: Option<(f64, f64)>) {
        let screen = self.screen.get();
        let scale = self.scale.get();
        let sampler = self.sampler.borrow();
        let Some(sampler) = sampler.as_ref() else {
            return;
        };
        let dragging = selection_px.is_some();

        let layout = loupe_layout(cursor, screen, scale, sampler);
        let loupe = &self.loupe;
        if let Some(cropped) = CGImage::with_image_in_rect(Some(sampler.image()), layout.sample_rect) {
            // SAFETY: CGImage is an ObjC object at runtime; CALayer.contents
            // expects it as AnyObject.
            let as_any: &AnyObject = unsafe { &*(std::ptr::from_ref(&*cropped).cast::<AnyObject>()) };
            // SAFETY: contents setter with a valid image object.
            unsafe { loupe.setContents(Some(as_any)) };
        }
        loupe.setFrame(layout.loupe_frame);
        loupe.setHidden(false);

        // Monosnap-style readout pinned under the loupe: cursor coordinates
        // while idle, the selection size while dragging. Logical pixels.
        let text = match selection_px {
            Some((w, h)) => format!("{} × {}", w.round() as i64, h.round() as i64),
            None => format!(
                "{} × {}",
                layout.cursor_logical.x.round() as i64,
                layout.cursor_logical.y.round() as i64
            ),
        };
        let label = &self.size_label;
        set_label_text(label, &text);
        label.setFrame(layout.label_frame);
        label.setHidden(false);

        // Idle: full-screen guides through the cursor with coordinate labels.
        // Drag: guides off — the rectangle is feedback enough.
        if dragging {
            self.axis_v_dark.setHidden(true);
            self.axis_v_light.setHidden(true);
            self.axis_h_dark.setHidden(true);
            self.axis_h_light.setHidden(true);
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
            for layer in [&self.axis_v_dark, &self.axis_v_light] {
                layer.setPath(Some(v_ref));
                layer.setHidden(false);
            }
            for layer in [&self.axis_h_dark, &self.axis_h_light] {
                layer.setPath(Some(h_ref));
                layer.setHidden(false);
            }
        }
    }

    /// Rebuild the red border path along the current display's perimeter.
    /// The rect is inset by half the stroke so the line stays fully inside
    /// the window. Called by `present` (displays differ in size and scale).
    fn rebuild_border_path(&self) {
        let inset = BORDER_W_PT / 2.0;
        let size = self.screen.get();
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
        self.border.setPath(Some(border_ref));
        self.border.setContentsScale(self.scale.get());
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
}

/// Placement math for the loupe and its readout, factored out of the
/// per-frame update as a pure function (unit-tested on a synthetic
/// sampler, no live layers involved).
struct LoupeLayout {
    /// Loupe frame in display-local points; the origin is snapped to whole
    /// points so the magnified squares stay pinned to the screen grid while
    /// the crop steps in whole pixels — otherwise the picture visibly
    /// slides under the cursor in sub-pixel steps.
    loupe_frame: CGRect,
    /// Readout bar frame: pinned under the loupe, flipped above it near the
    /// top edge of the screen.
    label_frame: CGRect,
    /// Logical-pixel sample rect around the cursor, kept fully inside the
    /// sampler bitmap; the cursor pixel stays the center cell.
    sample_rect: CGRect,
    /// The cursor in logical pixels (the readout text speaks them).
    cursor_logical: CGPoint,
}

/// Compute the loupe placement for the cursor: the loupe sits in the
/// gap-sized corner away from the cursor, flipping horizontally at the
/// right edge and vertically at the bottom edge (AppKit points, bottom-left
/// origin).
fn loupe_layout(cursor: CGPoint, screen: CGSize, scale: f64, sampler: &FrameSampler) -> LoupeLayout {
    let factor = sampler.factor as f64;
    let lw = sampler.width as f64;
    let lh = sampler.height as f64;
    // Whole cells between the cursor cell and the window edge on each side
    // (the sample side is odd, so this is symmetric).
    let side = (LOUPE_SAMPLE_PX / 2.0).floor();

    let cursor_logical =
        crate::capture::geometry::display_point_to_logical_pixel(cursor, screen.height, scale, factor);
    let px = cursor_logical.x.round();
    let py = cursor_logical.y.round();
    let sample_rect = CGRect::new(
        CGPoint::new(
            (px - side).clamp(0.0, lw - LOUPE_SAMPLE_PX),
            (py - side).clamp(0.0, lh - LOUPE_SAMPLE_PX),
        ),
        CGSize::new(LOUPE_SAMPLE_PX, LOUPE_SAMPLE_PX),
    );

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
    let loupe_frame = CGRect::new(
        CGPoint::new(lx, ly),
        CGSize::new(LOUPE_SIZE_PT, LOUPE_SIZE_PT),
    );

    let ty = if ly >= HUD_LABEL_H_PT + 6.0 {
        ly - HUD_LABEL_H_PT - 6.0
    } else {
        ly + LOUPE_SIZE_PT + 6.0
    };
    let label_frame = CGRect::new(
        CGPoint::new(lx, ty),
        CGSize::new(LOUPE_SIZE_PT, HUD_LABEL_H_PT),
    );

    LoupeLayout {
        loupe_frame,
        label_frame,
        sample_rect,
        cursor_logical,
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
        let ctx = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                Some(&*space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .expect("bitmap context");
        CGBitmapContextCreateImage(Some(&ctx)).expect("image from context")
    }

    /// Mean per-channel brightness of one sampler column, read straight
    /// from the backing bytes.
    fn column_brightness(sampler: &FrameSampler, x: usize) -> u32 {
        let mut total: u64 = 0;
        for y in 0..sampler.height {
            let i = (y * sampler.width + x) * 4;
            total += (sampler.buf[i] as u64) + (sampler.buf[i + 1] as u64) + (sampler.buf[i + 2] as u64);
        }
        (total / (sampler.height as u64 * 3)) as u32
    }

    #[test]
    fn sampler_downsamples_to_logical_pixels() {
        let image = synthetic_frame(64, 64);
        let sampler = FrameSampler::new(64, 64, 2.0, &image).expect("sampler");
        assert_eq!((sampler.width, sampler.height, sampler.factor), (32, 32, 2));
        // Left half of the frame is black, right half white: a column in the
        // dark half reads dark, one in the light half reads light.
        let dark = column_brightness(&sampler, 4);
        let light = column_brightness(&sampler, 28);
        assert!(dark < 128, "dark col: {dark}");
        assert!(light > 128, "light col: {light}");
    }

    #[test]
    fn sampler_handles_scale_one() {
        let image = synthetic_frame(64, 64);
        let sampler = FrameSampler::new(64, 64, 1.0, &image).expect("sampler");
        assert_eq!((sampler.width, sampler.height, sampler.factor), (64, 64, 1));
    }

    /// A 64×64 sampler at scale 1 (64×64 logical pixels) for layout math;
    /// the pixel content is irrelevant here.
    fn layout_sampler() -> FrameSampler {
        let image = synthetic_frame(64, 64);
        FrameSampler::new(64, 64, 1.0, &image).expect("sampler")
    }

    #[test]
    fn loupe_flips_away_from_the_screen_edges() {
        let sampler = layout_sampler();
        let screen = CGSize::new(800.0, 600.0);
        // Near the right edge the loupe flips to the left of the cursor.
        let l = loupe_layout(CGPoint::new(790.0, 300.0), screen, 1.0, &sampler);
        assert_eq!(l.loupe_frame.origin.x, 790.0 - LOUPE_GAP_PT - LOUPE_SIZE_PT);
        // Near the bottom edge (AppKit y grows up) it flips above.
        let l = loupe_layout(CGPoint::new(100.0, 5.0), screen, 1.0, &sampler);
        assert_eq!(l.loupe_frame.origin.y, 5.0 + LOUPE_GAP_PT);
        // Room on both sides: the loupe sits right and below.
        let l = loupe_layout(CGPoint::new(100.0, 300.0), screen, 1.0, &sampler);
        assert_eq!(l.loupe_frame.origin.x, 100.0 + LOUPE_GAP_PT);
        assert_eq!(l.loupe_frame.origin.y, 300.0 - LOUPE_GAP_PT - LOUPE_SIZE_PT);
        assert_eq!(l.loupe_frame.size, CGSize::new(LOUPE_SIZE_PT, LOUPE_SIZE_PT));
    }

    #[test]
    fn loupe_origin_snaps_to_whole_points() {
        let sampler = layout_sampler();
        let screen = CGSize::new(800.0, 600.0);
        let l = loupe_layout(CGPoint::new(100.4, 200.6), screen, 1.0, &sampler);
        assert_eq!(l.loupe_frame.origin.x, 118.0);
        assert_eq!(l.loupe_frame.origin.y, 36.0);
    }

    /// The sample side is odd: unclamped, the cursor pixel is the exact
    /// middle cell of the window — the same whole number of pixel cells on
    /// every side of the crosshair.
    #[test]
    fn sample_rect_centers_the_cursor_cell() {
        let sampler = layout_sampler();
        let screen = CGSize::new(800.0, 600.0);
        // Cursor inside the 64×64 sampler bitmap (AppKit y grows up).
        let l = loupe_layout(CGPoint::new(32.0, 568.0), screen, 1.0, &sampler);
        assert_eq!(l.cursor_logical, CGPoint::new(32.0, 32.0));
        assert_eq!(l.sample_rect.size.width, LOUPE_SAMPLE_PX);
        assert_eq!(l.sample_rect.size.height, LOUPE_SAMPLE_PX);
        assert_eq!(
            l.sample_rect.mid().x,
            l.cursor_logical.x + 0.5,
            "cursor cell centered horizontally"
        );
        assert_eq!(
            l.sample_rect.mid().y,
            l.cursor_logical.y + 0.5,
            "cursor cell centered vertically"
        );
    }

    #[test]
    fn sample_rect_stays_inside_the_bitmap() {
        let sampler = layout_sampler();
        // A tiny screen so the cursor maps past the sampler bounds.
        let screen = CGSize::new(40.0, 30.0);
        // Logical (70, 3): x clamps to the right edge, y stays.
        let l = loupe_layout(CGPoint::new(70.0, 3.0), screen, 1.0, &sampler);
        assert_eq!(l.sample_rect.max().x, 64.0);
        assert!(l.sample_rect.min().x >= 0.0);
        // Logical (70, 29): y clamps to the bottom of the bitmap.
        let l = loupe_layout(CGPoint::new(70.0, 29.0), screen, 1.0, &sampler);
        assert_eq!(l.sample_rect.min().y, 0.0);
        assert!(l.sample_rect.max().y <= 64.0);
    }

    #[test]
    fn label_flips_to_the_other_side_of_the_loupe() {
        let sampler = layout_sampler();
        let screen = CGSize::new(800.0, 600.0);
        // Cursor at the very top: the loupe sits below the cursor, the
        // label flips under the loupe.
        let l = loupe_layout(CGPoint::new(100.0, 2.0), screen, 1.0, &sampler);
        let loupe_top = l.loupe_frame.origin.y;
        assert!(loupe_top < 6.0 + HUD_LABEL_H_PT);
        assert_eq!(
            l.label_frame.origin.y,
            loupe_top + LOUPE_SIZE_PT + 6.0
        );
        // Mid-screen: the label rides above the loupe.
        let l = loupe_layout(CGPoint::new(100.0, 200.0), screen, 1.0, &sampler);
        assert_eq!(
            l.label_frame.origin.y,
            l.loupe_frame.origin.y - HUD_LABEL_H_PT - 6.0
        );
        assert_eq!(l.label_frame.size, CGSize::new(LOUPE_SIZE_PT, HUD_LABEL_H_PT));
    }

    #[test]
    fn idle_readout_speaks_logical_pixels() {
        let sampler = layout_sampler();
        let screen = CGSize::new(800.0, 600.0);
        let l = loupe_layout(CGPoint::new(100.0, 200.0), screen, 1.0, &sampler);
        assert_eq!((l.cursor_logical.x, l.cursor_logical.y), (100.0, 400.0));
    }

    /// The loupe's chrome hangs into the layer tree as the loupe's own
    /// sublayers, bottom-up: grid, crosshair, white frame outline, black
    /// frame core. Regression against "built but never attached": the
    /// chrome was once styled in `new` while missing from the tree.
    #[test]
    fn loupe_chrome_hangs_into_the_loupe_in_z_order() {
        let hud = Hud::new();
        let root = CALayer::new();
        hud.attach(&root, 2.0);

        // SAFETY: plain layer-tree reads, no external invariants.
        let children = unsafe { hud.loupe.sublayers() }.expect("loupe carries its chrome");
        let count = children.count();
        assert_eq!(count, 4, "loupe carries exactly its four chrome layers");
        // Identity, not just class or color: exactly the HUD's four chrome
        // layers, in the exact z-order.
        let attached: Vec<*const CALayer> = (0..count)
            .map(|i| Retained::as_ptr(&children.objectAtIndexedSubscript(i)))
            .collect();
        let expected = [
            Retained::as_ptr(&hud.loupe_grid).cast::<CALayer>(),
            Retained::as_ptr(&hud.loupe_cross).cast::<CALayer>(),
            Retained::as_ptr(&hud.loupe_frame_light).cast::<CALayer>(),
            Retained::as_ptr(&hud.loupe_frame_dark).cast::<CALayer>(),
        ];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(attached[i], *want, "loupe sublayer {i} is not the expected chrome layer");
        }
    }
}
