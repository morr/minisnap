//! Composite rendering: the base frame plus annotation elements, drawn with
//! CoreGraphics. Selection markers and any other UI chrome never render
//! here — a composite is exactly what a saved file or the pasteboard gets.

use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreateImage, CGColor, CGColorSpace, CGGradient, CGGradientDrawingOptions,
    CGImage, CGImageAlphaInfo, CGInterpolationQuality, CGLineCap, CGLineJoin, CGContext,
};

use super::{Element, ElementKind, Rect};

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

/// A plain RGBA bitmap context of the given pixel size. The context owns its
/// buffer; keep the context alive while drawing into it and snapshot with
/// `CGBitmapContextCreateImage` (which copies).
pub(crate) fn bitmap_context(width: usize, height: usize) -> Option<CFRetained<CGContext>> {
    if width == 0 || height == 0 {
        return None;
    }
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: the context owns its buffer for as long as the returned
    // context is alive.
    let ctx_ptr = unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            width,
            height,
            8,
            width * 4,
            &space,
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    };
    // SAFETY: ctx_ptr comes from a Create-rule constructor; on success we
    // take ownership of the +1 reference.
    unsafe { std::ptr::NonNull::new(ctx_ptr).map(|p| CFRetained::from_raw(p)) }
}

/// Render the base frame with all elements into a standalone image at the
/// base's pixel size.
pub fn render_composite(base: &CGImage, elements: &[Element]) -> Option<CFRetained<CGImage>> {
    let (w, h) = (CGImage::width(Some(base)), CGImage::height(Some(base)));
    let ctx = bitmap_context(w, h)?;
    CGContext::draw_image(
        Some(&ctx),
        CGRect::new(CGPoint::ZERO, CGSize::new(w as f64, h as f64)),
        Some(base),
    );
    draw_elements(&ctx, base, h as f64, elements);
    CGBitmapContextCreateImage(Some(&ctx))
}

/// Draw the elements onto a context whose user space is the base image's
/// pixel space: 1 unit = 1 pixel, origin at the image's top-left, Y down.
/// `base` feeds the blur elements (they pixelate the frame beneath them);
/// `image_height` is the image height in user units (pixels). The context
/// is flipped here, so callers pass an untransformed context.
pub fn draw_elements(ctx: &CGContext, base: &CGImage, image_height: f64, elements: &[Element]) {
    CGContext::save_g_state(Some(ctx));
    CGContext::translate_ctm(Some(ctx), 0.0, image_height);
    CGContext::scale_ctm(Some(ctx), 1.0, -1.0);
    for element in elements {
        draw_element(ctx, base, element);
    }
    CGContext::restore_g_state(Some(ctx));
}

fn draw_element(ctx: &CGContext, base: &CGImage, element: &Element) {
    match &element.kind {
        ElementKind::Arrow { a, b, color, width } => draw_arrow(ctx, *a, *b, *color, *width),
        ElementKind::Rect { rect, color, width } => {
            // Negative drag sizes (drawing up/left) normalize to a proper
            // rect; committed elements are already standard.
            let rect = rect.standardize();
            CGContext::set_rgb_stroke_color(Some(ctx), color[0], color[1], color[2], color[3]);
            CGContext::stroke_rect_with_width(
                Some(ctx),
                CGRect::new(
                    CGPoint::new(rect.x, rect.y),
                    CGSize::new(rect.width, rect.height),
                ),
                *width,
            );
        }
        ElementKind::Line { a, b, color, width } => {
            stroke_polyline(ctx, &[*a, *b], *color, *width);
        }
        ElementKind::Pen { points, color, width } => {
            stroke_polyline(ctx, points, *color, *width);
        }
        // Text and callouts render via Core Text; blur lands with the
        // pixelization task.
        ElementKind::Text { origin, text, color, size } => {
            super::text::draw_text(ctx, *origin, text, *color, *size);
        }
        ElementKind::Callout { tip, anchor, text, color, size } => {
            // A small wedge pointer from the text block to the target.
            draw_arrow(ctx, *anchor, *tip, *color, *size / 3.0);
            super::text::draw_text(ctx, *anchor, text, *color, *size);
        }
        ElementKind::Blur { rect } => {
            if let Some(blurred) = pixelate_region(base, *rect) {
                draw_image_upright(
                    ctx,
                    &blurred,
                    super::Point::new(rect.x, rect.y),
                    super::Size::new(rect.width, rect.height),
                );
            }
        }
    }
}

/// Draw `image` with its top-left corner at `top_left`, scaled to `size`,
/// inside the Y-down element space. `draw_image` orients against the Y-up
/// axis, so a local un-flip keeps the picture from mirroring.
fn draw_image_upright(
    ctx: &CGContext,
    image: &CGImage,
    top_left: super::Point,
    size: super::Size,
) {
    if size.width <= 0.0 || size.height <= 0.0 {
        return;
    }
    CGContext::save_g_state(Some(ctx));
    CGContext::translate_ctm(Some(ctx), top_left.x, top_left.y + size.height);
    CGContext::scale_ctm(Some(ctx), 1.0, -1.0);
    CGContext::draw_image(
        Some(ctx),
        CGRect::new(CGPoint::ZERO, CGSize::new(size.width, size.height)),
        Some(image),
    );
    CGContext::restore_g_state(Some(ctx));
}

/// Side of one pixelation block, in image pixels.
const BLUR_BLOCK_PX: usize = 10;

/// Pixelate `region` of `base`: downscale to block-sized cells, then redraw
/// stretched back with nearest-neighbor interpolation. Recomputed from the
/// base on every render, so moving the region reveals the original beneath.
pub fn pixelate_region(base: &CGImage, region: Rect) -> Option<CFRetained<CGImage>> {
    let (bw, bh) = (CGImage::width(Some(base)), CGImage::height(Some(base)));
    let bounds = Rect::new(0.0, 0.0, bw as f64, bh as f64);
    let region = clamp_rect(region.standardize(), bounds);
    if region.is_empty() {
        return None;
    }
    let w = region.width.round().max(1.0) as usize;
    let h = region.height.round().max(1.0) as usize;
    let cropped = crate::output::crop(base, cg_rect(region))?;

    // Downscale: one pixel per block.
    let cells_w = (w / BLUR_BLOCK_PX).clamp(1, w);
    let cells_h = (h / BLUR_BLOCK_PX).clamp(1, h);
    let small = bitmap_context(cells_w, cells_h)?;
    CGContext::draw_image(
        Some(&small),
        CGRect::new(
            CGPoint::ZERO,
            CGSize::new(cells_w as f64, cells_h as f64),
        ),
        Some(&cropped),
    );
    let small_image = CGBitmapContextCreateImage(Some(&small))?;

    // Upscale with nearest neighbors: hard mosaic blocks.
    let big = bitmap_context(w, h)?;
    CGContext::set_interpolation_quality(Some(&big), CGInterpolationQuality::None);
    CGContext::draw_image(
        Some(&big),
        CGRect::new(CGPoint::ZERO, CGSize::new(w as f64, h as f64)),
        Some(&small_image),
    );
    CGBitmapContextCreateImage(Some(&big))
}

/// Intersect `rect` with `bounds` (both standardized); an empty result reads
/// as an empty rect.
fn clamp_rect(rect: Rect, bounds: Rect) -> Rect {
    let x0 = rect.x.max(bounds.x);
    let y0 = rect.y.max(bounds.y);
    let x1 = rect.max_x().min(bounds.max_x());
    let y1 = rect.max_y().min(bounds.max_y());
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// How much darker than the picked color the arrow tail starts (the gradient
/// runs from this shade at the tail to the full color at the tip).
const ARROW_TAIL_SHADE: f64 = 0.55;

/// Soft drop shadow under an arrow: offset in element space (+y is
/// screen-down here — note the render context is Y-flipped, so the offset
/// needs a negative y to land below), blur radius, and opacity.
const ARROW_SHADOW_OFFSET_X: f64 = 1.0;
const ARROW_SHADOW_OFFSET_Y: f64 = -2.0;
const ARROW_SHADOW_BLUR: f64 = 3.0;
const ARROW_SHADOW_ALPHA: f64 = 0.35;

/// The arrow silhouette, Monosnap-style: the body is a wedge that starts
/// needle-thin with a rounded cap at `a` and widens along the whole length,
/// and the head is a barbed arrowhead with a concave notch at its base on
/// each side. One closed ring (the last point connects back to the first);
/// empty for a degenerate zero-length arrow.
pub(crate) fn arrow_outline(a: super::Point, b: super::Point, width: f64) -> Vec<super::Point> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < f64::EPSILON {
        return Vec::new();
    }
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    // Proportions of the stroke width; the head clamps on short arrows so
    // it never eats the whole length.
    let head_len = (width * 4.6).min(len * 0.5);
    let half_head = width * 2.3;
    let half_body = width * 1.1;
    let tail_r = width * 0.32;
    // The rounded tail cap is a semicircle of radius `tail_r` behind its
    // center; the body widens linearly from `tail_r` to `half_body` up to
    // where it dips into the head.
    let cap = super::Point::new(a.x + ux * tail_r, a.y + uy * tail_r);
    let body_end = super::Point::new(
        b.x - ux * head_len * 0.78,
        b.y - uy * head_len * 0.78,
    );
    let step = |p: super::Point, along: f64, off: f64| {
        super::Point::new(p.x + ux * along + nx * off, p.y + uy * along + ny * off)
    };
    let mut pts = Vec::with_capacity(16);
    // Tail cap: from the bottom edge around the back to the top edge.
    const CAP_STEPS: usize = 6;
    for k in 0..=CAP_STEPS {
        let t =
            -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * k as f64 / CAP_STEPS as f64;
        pts.push(super::Point::new(
            cap.x + tail_r * (nx * t.sin() - ux * t.cos()),
            cap.y + tail_r * (ny * t.sin() - uy * t.cos()),
        ));
    }
    // Top edge of the widening body, the concave notch into the top barb,
    // the tip, and the mirrored way back.
    pts.push(step(body_end, 0.0, half_body));
    pts.push(step(b, -head_len, half_head));
    pts.push(b);
    pts.push(step(b, -head_len, -half_head));
    pts.push(step(body_end, 0.0, -half_body));
    pts
}

/// The arrow's fill: a linear gradient from a darkened shade of the picked
/// color to the picked color itself.
fn arrow_gradient(color: super::Rgba) -> Option<CFRetained<CGGradient>> {
    let space = CGColorSpace::new_device_rgb()?;
    let components = [
        color[0] * ARROW_TAIL_SHADE,
        color[1] * ARROW_TAIL_SHADE,
        color[2] * ARROW_TAIL_SHADE,
        color[3],
        color[0],
        color[1],
        color[2],
        color[3],
    ];
    let locations = [0.0f64, 1.0];
    // SAFETY: both slices hold exactly the two stops the constructor reads.
    unsafe {
        CGGradient::with_color_components(
            Some(&space),
            components.as_ptr(),
            locations.as_ptr(),
            locations.len(),
        )
    }
}

/// Arrow from `a` to `b`, Monosnap-style (see `arrow_outline`): filled with
/// a gradient from a darker tail to the picked color at the tip, over a soft
/// drop shadow.
pub(crate) fn draw_arrow(
    ctx: &CGContext,
    a: super::Point,
    b: super::Point,
    color: super::Rgba,
    width: f64,
) {
    let outline = arrow_outline(a, b, width);
    if outline.is_empty() {
        return;
    }
    let add_path = |ctx: &CGContext| {
        CGContext::begin_path(Some(ctx));
        let mut iter = outline.iter();
        if let Some(first) = iter.next() {
            CGContext::move_to_point(Some(ctx), first.x, first.y);
            for p in iter {
                CGContext::add_line_to_point(Some(ctx), p.x, p.y);
            }
        }
        CGContext::close_path(Some(ctx));
    };

    // Silhouette pass: a solid fill casts the shadow. The gradient pass
    // repaints the whole surface opaquely, so this fill only survives as
    // the shadow it cast (and as the arrow itself when the gradient cannot
    // be built).
    CGContext::save_g_state(Some(ctx));
    let shadow = CGColor::new_srgb(0.0, 0.0, 0.0, ARROW_SHADOW_ALPHA);
    CGContext::set_shadow_with_color(
        Some(ctx),
        CGSize::new(ARROW_SHADOW_OFFSET_X, ARROW_SHADOW_OFFSET_Y),
        ARROW_SHADOW_BLUR,
        Some(&shadow),
    );
    CGContext::set_rgb_fill_color(Some(ctx), color[0], color[1], color[2], color[3]);
    add_path(ctx);
    CGContext::fill_path(Some(ctx));
    CGContext::restore_g_state(Some(ctx));

    if let Some(gradient) = arrow_gradient(color) {
        CGContext::save_g_state(Some(ctx));
        add_path(ctx);
        CGContext::clip(Some(ctx));
        CGContext::draw_linear_gradient(
            Some(ctx),
            Some(&gradient),
            CGPoint::new(a.x, a.y),
            CGPoint::new(b.x, b.y),
            CGGradientDrawingOptions::DrawsBeforeStartLocation
                | CGGradientDrawingOptions::DrawsAfterEndLocation,
        );
        CGContext::restore_g_state(Some(ctx));
    }
}

fn stroke_polyline(ctx: &CGContext, points: &[super::Point], color: super::Rgba, width: f64) {
    if points.len() < 2 {
        return;
    }
    let points: Vec<CGPoint> = points
        .iter()
        .map(|p| CGPoint::new(p.x, p.y))
        .collect();
    CGContext::set_rgb_stroke_color(Some(ctx), color[0], color[1], color[2], color[3]);
    CGContext::set_line_cap(Some(ctx), CGLineCap::Round);
    CGContext::set_line_join(Some(ctx), CGLineJoin::Round);
    CGContext::set_line_width(Some(ctx), width);
    CGContext::begin_path(Some(ctx));
    // SAFETY: points outlives the call.
    unsafe { CGContext::add_lines(Some(ctx), points.as_ptr(), points.len()) };
    CGContext::stroke_path(Some(ctx));
}

/// Element-space rect → CGRect.
pub(crate) fn cg_rect(rect: Rect) -> CGRect {
    CGRect::new(
        CGPoint::new(rect.x, rect.y),
        CGSize::new(rect.width, rect.height),
    )
}

#[cfg(test)]
mod tests {
    use super::super::tests::{noisy_frame, pixel_bytes, solid_frame};
    use super::super::{Element, ElementKind, Point, Rgba};
    use super::*;

    const MAGENTA: Rgba = [1.0, 0.0, 1.0, 1.0];
    const RED: Rgba = [1.0, 0.0, 0.0, 1.0];

    /// The pixel at image coordinates (x, y) of an RGBA byte buffer.
    fn pixel(bytes: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let i = (y * w + x) * 4;
        [bytes[i], bytes[i + 1], bytes[i + 2]]
    }

    #[test]
    fn composite_renders_at_base_size() {
        let base = solid_frame(64, 64);
        let elements = vec![
            Element {
                id: 1,
                kind: ElementKind::Arrow {
                    a: Point::new(10.0, 10.0),
                    b: Point::new(50.0, 10.0),
                    color: MAGENTA,
                    width: 6.0,
                },
            },
            Element {
                id: 2,
                kind: ElementKind::Rect {
                    rect: Rect::new(20.0, 30.0, 24.0, 20.0),
                    color: RED,
                    width: 4.0,
                },
            },
            Element {
                id: 3,
                kind: ElementKind::Line {
                    a: Point::new(30.0, 50.0),
                    b: Point::new(40.0, 60.0),
                    color: MAGENTA,
                    width: 3.0,
                },
            },
            Element {
                id: 4,
                kind: ElementKind::Pen {
                    points: vec![
                        Point::new(5.0, 5.0),
                        Point::new(15.0, 8.0),
                        Point::new(25.0, 6.0),
                    ],
                    color: MAGENTA,
                    width: 3.0,
                },
            },
        ];
        let composite = render_composite(&base, &elements).expect("composite");
        assert_eq!(
            (CGImage::width(Some(&composite)), CGImage::height(Some(&composite))),
            (64, 64)
        );

        // The wedge arrow covers the axis midpoint, in image coordinates:
        // (30, 10) must be magenta (darkened by the tail-side gradient),
        // not the gray base.
        let bytes = pixel_bytes(&composite);
        let arrow_px = pixel(&bytes, 64, 30, 10);
        assert!(arrow_px[0] > 180 && arrow_px[2] > 180, "arrow pixel {arrow_px:?}");

        // The rect stroke covers (20, 40) (the left edge band 18..22).
        let rect_px = pixel(&bytes, 64, 20, 40);
        assert!(rect_px[0] > 200 && rect_px[2] < 100, "rect pixel {rect_px:?}");
    }

    #[test]
    fn arrow_matches_the_monosnap_shape() {
        let base = solid_frame(64, 64);
        let element = Element {
            id: 1,
            kind: ElementKind::Arrow {
                a: Point::new(8.0, 32.0),
                b: Point::new(56.0, 32.0),
                color: MAGENTA,
                width: 6.0,
            },
        };
        let composite = render_composite(&base, &[element]).expect("composite");
        let bytes = pixel_bytes(&composite);
        // width 6 over len 48: tail cap radius 1.92 at x≈10, body widening
        // to half 6.6 at x=37.3, head base at x=32 with half 13.8, tip at b.
        let is_arrow = |x: usize, y: usize| {
            let p = pixel(&bytes, 64, x, y);
            p[0] > 60 && p[2] > 60 && p[1] < 100 && (p[0] as i32 - p[2] as i32).abs() < 40
        };
        let is_gray = |x: usize, y: usize| {
            let p = pixel(&bytes, 64, x, y);
            (p[0] as i32 - 128).abs() < 40 && (p[2] as i32 - 128).abs() < 40
        };
        // The tail starts needle-thin: on-axis colored, a few pixels off it
        // already clear (the old flat flare occupied that space).
        assert!(is_arrow(12, 31), "tail on the axis");
        assert!(is_gray(12, 28), "nothing beside the thin tail");
        assert!(is_gray(10, 29), "rounded narrow start");
        // The body is a widening wedge, not a constant-width shaft.
        assert!(is_arrow(25, 32), "body mid-way");
        assert!(is_arrow(30, 37), "body widened toward the head");
        assert!(is_gray(14, 36), "body still thin near the tail");
        assert!(is_gray(25, 26), "nothing above the thin mid-body");
        // The head is a barbed arrowhead: wide at the base, colored apex at
        // b, and a concave notch void between the barbs and the body. The
        // body's soft shadow darkens the void, but no magenta fill lands
        // there: the pixel stays neutral gray (green tracks red), unlike
        // the wing beside it.
        assert!(is_arrow(38, 24), "head above the axis");
        assert!(is_arrow(38, 40), "head below the axis");
        assert!(is_arrow(53, 32), "head apex");
        assert!(is_gray(38, 18), "nothing beyond the head");
        let void = pixel(&bytes, 64, 35, 40);
        assert!(
            void[1] as i32 > void[0] as i32 / 2,
            "concave notch void beside the body end: {void:?}"
        );
        assert!(is_arrow(35, 42), "barb wing over the notch");
        // The fill is a gradient: darker at the tail, the picked color at
        // the tip.
        let tail = pixel(&bytes, 64, 12, 31);
        let tip = pixel(&bytes, 64, 50, 31);
        assert!(tail[0] < 170 && tail[2] < 170, "tail shaded: {tail:?}");
        assert!(tip[0] > 220 && tip[2] > 220, "tip at full color: {tip:?}");
        // A soft shadow darkens the base just below the body silhouette
        // (the silhouette itself ends at y≈35.6 here).
        let below = pixel(&bytes, 64, 20, 37);
        assert!(below[0] < 120 && below[2] < 120, "shadow below: {below:?}");
    }

    #[test]
    fn pixelate_produces_flat_blocks() {
        let base = noisy_frame(64, 64);
        let blurred = pixelate_region(&base, Rect::new(0.0, 0.0, 64.0, 64.0)).expect("blur");
        assert_eq!(
            (CGImage::width(Some(&blurred)), CGImage::height(Some(&blurred))),
            (64, 64)
        );
        let bytes = pixel_bytes(&blurred);
        // Pixels inside one mosaic block are identical…
        assert_eq!(pixel(&bytes, 64, 0, 0), pixel(&bytes, 64, 5, 5));
        // …and the region differs from the original noise somewhere.
        let orig = pixel_bytes(&base);
        let mut diffs = 0;
        for y in 0..64usize {
            for x in 0..64usize {
                if pixel(&bytes, 64, x, y) != pixel(&orig, 64, x, y) {
                    diffs += 1;
                }
            }
        }
        assert!(diffs > 0, "pixelation left the region unchanged");
    }

    #[test]
    fn composite_blurs_only_the_region() {
        let base = noisy_frame(64, 64);
        let element = Element {
            id: 1,
            kind: ElementKind::Blur {
                rect: Rect::new(0.0, 0.0, 32.0, 64.0),
            },
        };
        let composite = render_composite(&base, &[element]).expect("composite");
        let orig = pixel_bytes(&base);
        let bytes = pixel_bytes(&composite);
        // The half outside the region is untouched.
        for y in 0..64usize {
            for x in 32..64usize {
                assert_eq!(
                    pixel(&bytes, 64, x, y),
                    pixel(&orig, 64, x, y),
                    "pixel ({x},{y}) outside the blur region changed"
                );
            }
        }
        // The region itself is covered by the mosaic: flat blocks where the
        // original had noise.
        assert_ne!(pixel(&bytes, 64, 0, 0), pixel(&bytes, 64, 15, 15));
        assert_eq!(pixel(&bytes, 64, 0, 0), pixel(&bytes, 64, 5, 5));
    }
}
