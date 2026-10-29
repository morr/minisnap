//! Composite rendering: the base frame plus annotation elements, drawn with
//! CoreGraphics. Selection markers and any other UI chrome never render
//! here — a composite is exactly what a saved file or the pasteboard gets.

use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreateImage, CGColorSpace, CGImage, CGImageAlphaInfo, CGLineCap, CGLineJoin,
    CGContext,
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
    draw_elements(&ctx, h as f64, elements);
    CGBitmapContextCreateImage(Some(&ctx))
}

/// Draw the elements onto a context whose user space is the base image's
/// pixel space: 1 unit = 1 pixel, origin at the image's top-left, Y down.
/// `image_height` is the image height in user units (pixels); the context
/// is flipped here, so callers pass an untransformed context.
pub fn draw_elements(ctx: &CGContext, image_height: f64, elements: &[Element]) {
    CGContext::save_g_state(Some(ctx));
    CGContext::translate_ctm(Some(ctx), 0.0, image_height);
    CGContext::scale_ctm(Some(ctx), 1.0, -1.0);
    for element in elements {
        draw_element(ctx, element);
    }
    CGContext::restore_g_state(Some(ctx));
}

fn draw_element(ctx: &CGContext, element: &Element) {
    match &element.kind {
        ElementKind::Arrow { a, b, color, width } => draw_arrow(ctx, *a, *b, *color, *width),
        ElementKind::Rect { rect, color, width } => {
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
        ElementKind::Blur { .. } => {}
    }
}

/// Wedge arrow from `a` to `b`: perpendiculars to the axis, tail ~25% of
/// the thickness, widening toward the head and ending in a point at `b`.
fn draw_arrow(ctx: &CGContext, a: super::Point, b: super::Point, color: super::Rgba, width: f64) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < f64::EPSILON {
        return;
    }
    let (nx, ny) = (-dy / len, dx / len);
    let head = width / 2.0;
    let tail = width / 8.0;
    let corners = [
        CGPoint::new(a.x + nx * tail, a.y + ny * tail),
        CGPoint::new(b.x + nx * head, b.y + ny * head),
        CGPoint::new(b.x, b.y),
        CGPoint::new(b.x - nx * head, b.y - ny * head),
        CGPoint::new(a.x - nx * tail, a.y - ny * tail),
    ];
    CGContext::set_rgb_fill_color(Some(ctx), color[0], color[1], color[2], color[3]);
    CGContext::begin_path(Some(ctx));
    CGContext::move_to_point(Some(ctx), corners[0].x, corners[0].y);
    // SAFETY: corners outlives the call.
    unsafe { CGContext::add_lines(Some(ctx), corners[1..].as_ptr(), corners.len() - 1) };
    CGContext::close_path(Some(ctx));
    CGContext::fill_path(Some(ctx));
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
    use super::super::tests::{pixel_bytes, solid_frame};
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
        // (30, 10) must be magenta, not the gray base.
        let bytes = pixel_bytes(&composite);
        let arrow_px = pixel(&bytes, 64, 30, 10);
        assert!(arrow_px[0] > 200 && arrow_px[2] > 200, "arrow pixel {arrow_px:?}");

        // The rect stroke covers (20, 40) (the left edge band 18..22).
        let rect_px = pixel(&bytes, 64, 20, 40);
        assert!(rect_px[0] > 200 && rect_px[2] < 100, "rect pixel {rect_px:?}");
    }
}
