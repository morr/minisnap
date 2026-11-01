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
        ElementKind::Callout { tip, anchor, text, color, size, width } => {
            // The pointer emerges from the text block's edge nearest the
            // target and keeps the stroke width it was drawn with. While the
            // text is still empty (the two-phase preview), the tail stays at
            // the drawn base — the arrow renders exactly like a plain arrow.
            let tail = callout_tail(*tip, *anchor, text, *size);
            draw_arrow(ctx, tail, *tip, *color, *width);
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

/// The knobs of the arrow construction — head proportions, notch and wing
/// curves, fill, shadow — in one `Copy` struct. `ArrowParams::DEFAULT` is
/// the shipped look; the debug tuner (`arrow_tuner_panel` in
/// `editor_window.rs`, shown under `MINISNAP_ARROW_DEBUG`) rewrites the
/// live copy in `ARROW_PARAMS` on every slider tick, and each render
/// re-reads it, so the arrows respond instantly.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ArrowParams {
    /// Head length and half-width at the base (the barbs), in stroke
    /// widths; the head length clamps to half the arrow length so short
    /// arrows keep a body.
    pub head_len_w: f64,
    /// Must stay above the body half-width (1.1 stroke widths), or the
    /// wing inverts into the body.
    pub half_head_w: f64,
    /// Where the notch shoulder sits along the head, as a fraction of the
    /// head length from the tip (in front of the barb line, leaving a
    /// notch pocket between the head and the body).
    pub notch_at: f64,
    /// How far the notch control point bows out between the body edge and
    /// the barb, 0..1. 0.5 leaves the notch edge tangent to the
    /// perpendicular at the barb — the wing tips open wide (~60°) — while
    /// the pocket stays as deep as the straight edge's.
    pub notch_bow: f64,
    /// How far the outer wing edge (barb → tip) bows out of its chord, as
    /// a fraction of the head half-width — the convex sweep of the head's
    /// silhouette, the outer counterpart of the notch bow. 0 is straight.
    pub outer_bow: f64,
    /// Corner-rounding radius of the notch ends — the wing tips and the
    /// pocket apexes — in stroke widths. The wing tips read like the
    /// rounded tail cap, a touch narrower.
    pub notch_round_w: f64,
    /// The cross-wise gradient fill: how much the light end blends toward
    /// white and the dark end scales down the picked color (the axis runs
    /// from the upper notch of the head to the lower one — light from
    /// above, whatever the arrow's direction).
    pub light_toward_white: f64,
    pub dark_shade: f64,
    /// Soft drop shadow under an arrow: offset in element space (+y is
    /// screen-down here — note the render context is Y-flipped, so the
    /// offset needs a negative y to land below), blur radius, and opacity.
    pub shadow_dx: f64,
    pub shadow_dy: f64,
    pub shadow_blur: f64,
    pub shadow_alpha: f64,
}

impl ArrowParams {
    /// The shipped Monosnap-style look.
    pub(crate) const DEFAULT: ArrowParams = ArrowParams {
        head_len_w: 5.14,
        half_head_w: 2.41,
        notch_at: 0.71,
        notch_bow: 0.40,
        outer_bow: 0.08,
        notch_round_w: 0.74,
        light_toward_white: 0.26,
        dark_shade: 0.67,
        shadow_dx: 0.5,
        shadow_dy: -1.0,
        shadow_blur: 2.0,
        shadow_alpha: 0.33,
    };
}

/// The live arrow knobs — `ArrowParams::DEFAULT` until the debug tuner
/// rewrites them. A poisoned lock is taken over rather than panicked on:
/// the params are plain copyable data, never structurally broken.
static ARROW_PARAMS: std::sync::RwLock<ArrowParams> =
    std::sync::RwLock::new(ArrowParams::DEFAULT);

/// A copy of the live arrow knobs.
pub(crate) fn arrow_params() -> ArrowParams {
    *ARROW_PARAMS.read().unwrap_or_else(|e| e.into_inner())
}

/// Replace the live arrow knobs (the debug tuner calls this on every
/// slider tick).
pub(crate) fn set_arrow_params(params: ArrowParams) {
    *ARROW_PARAMS.write().unwrap_or_else(|e| e.into_inner()) = params;
}

/// Direction and head proportions of an arrow.
struct ArrowFrame {
    /// Unit vector a → b (along the arrow).
    u: super::Point,
    /// Unit normal of u (across the arrow).
    n: super::Point,
    /// Head length from the tip, in pixels.
    head_len: f64,
    /// Head half-width at the base, in pixels.
    half_head: f64,
}

impl ArrowFrame {
    fn new(
        a: super::Point,
        b: super::Point,
        width: f64,
        params: ArrowParams,
    ) -> Option<ArrowFrame> {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = (dx * dx + dy * dy).sqrt();
        if len < f64::EPSILON {
            return None;
        }
        Some(ArrowFrame {
            u: super::Point::new(dx / len, dy / len),
            n: super::Point::new(-dy / len, dx / len),
            head_len: (width * params.head_len_w).min(len * 0.5),
            half_head: width * params.half_head_w,
        })
    }

    /// The point `along` the a→b axis from `p`, offset `off` across it.
    fn step(&self, p: super::Point, along: f64, off: f64) -> super::Point {
        super::Point::new(
            p.x + self.u.x * along + self.n.x * off,
            p.y + self.u.y * along + self.n.y * off,
        )
    }
}

/// Where a callout's pointer starts: the point of the text block's rect
/// nearest the tip, so the arrow always emerges from the block's edge facing
/// its target. An empty block (the two-phase preview, whose anchor still is
/// the drawn base) leaves the tail at the anchor.
pub(crate) fn callout_tail(
    tip: super::Point,
    anchor: super::Point,
    text: &str,
    size: f64,
) -> super::Point {
    let Some(m) = super::text::measure_block(text, size) else {
        return anchor;
    };
    super::Point::new(
        tip.x.clamp(anchor.x, anchor.x + m.width),
        tip.y.clamp(anchor.y, anchor.y + m.height),
    )
}

/// The arrow silhouette, Monosnap-style: the body is a wedge that starts
/// needle-thin with a rounded cap at `a` and widens along the whole length;
/// the head is a barbed arrowhead whose wings sweep out on convex outer
/// edges and end in wide rounded tips, with notch pockets between the
/// wings and the body. One closed ring (the last point connects back to
/// the first); empty for a degenerate zero-length arrow.
pub(crate) fn arrow_outline(a: super::Point, b: super::Point, width: f64) -> Vec<super::Point> {
    arrow_outline_with(arrow_params(), a, b, width)
}

/// `arrow_outline` against explicit params — tests reshape the arrow
/// without touching the live global.
pub(crate) fn arrow_outline_with(
    params: ArrowParams,
    a: super::Point,
    b: super::Point,
    width: f64,
) -> Vec<super::Point> {
    let Some(frame) = ArrowFrame::new(a, b, width, params) else {
        return Vec::new();
    };
    let half_body = width * 1.1;
    let tail_r = width * 0.32;
    let round_r = width * params.notch_round_w;
    // The rounded tail cap is a semicircle of radius `tail_r` behind its
    // center; the body widens linearly from `tail_r` to `half_body` up to
    // the notch shoulder.
    let cap = frame.step(a, tail_r, 0.0);
    // One side's notch: a concave quadratic from the shoulder out to the
    // barb, bowed so it leaves the barb perpendicular to the axis (wide
    // wing tips) and flattens into the pocket toward the shoulder.
    let wing = |sign: f64| {
        let shoulder = frame.step(b, -frame.head_len * params.notch_at, half_body * sign);
        let barb = frame.step(b, -frame.head_len, frame.half_head * sign);
        let out = half_body + params.notch_bow * (frame.half_head - half_body);
        let elbow = frame.step(b, -frame.head_len, out * sign);
        vec![
            (shoulder, Some(round_r)),
            (quad(shoulder, elbow, barb, 0.35), None),
            (quad(shoulder, elbow, barb, 0.55), None),
            (barb, Some(round_r)),
        ]
    };
    // The outer wing edge: a convex quadratic from the barb to the tip,
    // bowed out of the chord by a fraction of the head half-width — the
    // outer counterpart of the notch curve, so the wing sweeps into the
    // tip instead of running straight. The tip itself stays sharp.
    let outer = |sign: f64| {
        let barb = frame.step(b, -frame.head_len, frame.half_head * sign);
        let bulge = params.outer_bow * frame.half_head;
        let ctrl = super::Point::new(
            (barb.x + b.x) / 2.0 + frame.n.x * sign * bulge,
            (barb.y + b.y) / 2.0 + frame.n.y * sign * bulge,
        );
        [0.3, 0.5, 0.7].map(|t| (quad(barb, ctrl, b, t), None))
    };
    // (point, corner radius) pairs: the cap arc points and the tip stay
    // sharp, the notch ends round.
    let mut ring: Vec<(super::Point, Option<f64>)> = Vec::with_capacity(24);
    const CAP_STEPS: usize = 6;
    for k in 0..=CAP_STEPS {
        let t =
            -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * k as f64 / CAP_STEPS as f64;
        ring.push((
            super::Point::new(
                cap.x + tail_r * (frame.n.x * t.sin() - frame.u.x * t.cos()),
                cap.y + tail_r * (frame.n.y * t.sin() - frame.u.y * t.cos()),
            ),
            None,
        ));
    }
    // One side's notch and wing, the tip, then the mirrored side walked
    // back from the head toward the tail.
    ring.extend(wing(1.0));
    ring.extend(outer(1.0));
    ring.push((b, None));
    let lower_outer = outer(-1.0);
    ring.extend(lower_outer.into_iter().rev());
    let lower = wing(-1.0);
    ring.extend(lower.into_iter().rev());
    round_corners(&ring)
}

/// A point on the quadratic Bézier (`a`, `ctrl`, `b`) at `t`.
fn quad(a: super::Point, ctrl: super::Point, b: super::Point, t: f64) -> super::Point {
    let (sa, sc, sb) = ((1.0 - t).powi(2), 2.0 * t * (1.0 - t), t * t);
    super::Point::new(
        sa * a.x + sc * ctrl.x + sb * b.x,
        sa * a.y + sc * ctrl.y + sb * b.y,
    )
}

/// Round every corner carrying a radius: the vertex is replaced by a short
/// quadratic arc tangent to both adjacent edges, clamped so an arc never
/// reaches past 70% of either of its edges. Radius-carrying vertices must
/// be separated by at least one plain vertex — then two arcs never meet on
/// a shared edge. Sharp vertices (None) pass through.
fn round_corners(ring: &[(super::Point, Option<f64>)]) -> Vec<super::Point> {
    const ARC_STEPS: usize = 4;
    let n = ring.len();
    let mut out = Vec::with_capacity(n * (ARC_STEPS + 1));
    for i in 0..n {
        let (p, radius) = ring[i];
        let (prev, next) = (ring[(i + n - 1) % n].0, ring[(i + 1) % n].0);
        let Some(radius) = radius else {
            out.push(p);
            continue;
        };
        let (d_prev, d_next) = (pt_dist(p, prev), pt_dist(p, next));
        let r = radius.min(d_prev * 0.7).min(d_next * 0.7);
        if r <= f64::EPSILON {
            out.push(p);
            continue;
        }
        let toward = |q: super::Point, d: f64| {
            super::Point::new(p.x + (q.x - p.x) / d * r, p.y + (q.y - p.y) / d * r)
        };
        let (q1, q2) = (toward(prev, d_prev), toward(next, d_next));
        out.push(q1);
        for k in 1..ARC_STEPS {
            let t = k as f64 / ARC_STEPS as f64;
            let (a, b, c) = ((1.0 - t).powi(2), 2.0 * t * (1.0 - t), t * t);
            out.push(super::Point::new(
                a * q1.x + b * p.x + c * q2.x,
                a * q1.y + b * p.y + c * q2.y,
            ));
        }
        out.push(q2);
    }
    out
}

fn pt_dist(a: super::Point, b: super::Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// The arrow's fill: a linear gradient from a lighter shade of the picked
/// color (blended toward white) to a darker one.
fn arrow_gradient(
    color: super::Rgba,
    params: ArrowParams,
) -> Option<CFRetained<CGGradient>> {
    let space = CGColorSpace::new_device_rgb()?;
    let light = |c: f64| c + (1.0 - c) * params.light_toward_white;
    let components = [
        light(color[0]),
        light(color[1]),
        light(color[2]),
        color[3],
        color[0] * params.dark_shade,
        color[1] * params.dark_shade,
        color[2] * params.dark_shade,
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
/// a cross-wise gradient — lighter above, darker below — over a soft drop
/// shadow.
pub(crate) fn draw_arrow(
    ctx: &CGContext,
    a: super::Point,
    b: super::Point,
    color: super::Rgba,
    width: f64,
) {
    let params = arrow_params();
    let Some(frame) = ArrowFrame::new(a, b, width, params) else {
        return;
    };
    let outline = arrow_outline_with(params, a, b, width);
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
    let shadow = CGColor::new_srgb(0.0, 0.0, 0.0, params.shadow_alpha);
    CGContext::set_shadow_with_color(
        Some(ctx),
        CGSize::new(params.shadow_dx, params.shadow_dy),
        params.shadow_blur,
        Some(&shadow),
    );
    CGContext::set_rgb_fill_color(Some(ctx), color[0], color[1], color[2], color[3]);
    add_path(ctx);
    CGContext::fill_path(Some(ctx));
    CGContext::restore_g_state(Some(ctx));

    // Gradient pass: across the arrow, from one notch of the head to the
    // other, the light end on whichever barb sits higher on screen.
    if let Some(gradient) = arrow_gradient(color, params) {
        let barb_a = frame.step(b, -frame.head_len, frame.half_head);
        let barb_b = frame.step(b, -frame.head_len, -frame.half_head);
        let (top, bottom) = if (barb_a.y, barb_a.x) <= (barb_b.y, barb_b.x) {
            (barb_a, barb_b)
        } else {
            (barb_b, barb_a)
        };
        CGContext::save_g_state(Some(ctx));
        add_path(ctx);
        CGContext::clip(Some(ctx));
        CGContext::draw_linear_gradient(
            Some(ctx),
            Some(&gradient),
            CGPoint::new(top.x, top.y),
            CGPoint::new(bottom.x, bottom.y),
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
    fn callout_tail_tracks_the_text_block_edge() {
        // Empty text (the two-phase preview): the tail stays at the anchor,
        // which then is the arrow's drawn base.
        assert_eq!(
            callout_tail(Point::new(100.0, 50.0), Point::new(20.0, 30.0), "", 32.0),
            Point::new(20.0, 30.0)
        );

        // With text, the tail is the text block's point nearest the tip: a
        // tip right of the block lands on its right edge.
        let m = super::super::text::measure_block("WW", 32.0).expect("measured");
        assert_eq!(
            callout_tail(Point::new(100.0, 30.0), Point::new(10.0, 20.0), "WW", 32.0),
            Point::new(10.0 + m.width, 30.0)
        );
        // A tip below-right clamps onto the block's bottom-right corner.
        assert_eq!(
            callout_tail(Point::new(100.0, 80.0), Point::new(10.0, 20.0), "WW", 32.0),
            Point::new(10.0 + m.width, 20.0 + m.height)
        );
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
        // to half 6.6 at the shoulder x≈34.1, barbs at x≈25.2 with half
        // 14.5, tip at b.
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
        assert!(is_arrow(30, 36), "body widened toward the head");
        assert!(is_gray(14, 36), "body still thin near the tail");
        assert!(is_gray(25, 26), "nothing above the thin mid-body");
        // The head is a barbed arrowhead: wide at the barbs, colored apex at
        // b, and a curved notch pocket between each wing and the body (only
        // the soft shadow darkens the pocket, so the pixel stays neutral
        // gray, green tracking red).
        assert!(is_arrow(38, 24), "head above the axis");
        assert!(is_arrow(38, 40), "head below the axis");
        assert!(is_arrow(53, 32), "head apex");
        assert!(is_gray(38, 16), "nothing beyond the head");
        let pocket = pixel(&bytes, 64, 32, 38);
        assert!(
            pocket[1] as i32 > pocket[0] as i32 / 2,
            "curved notch pocket beside the body: {pocket:?}"
        );
        assert!(is_arrow(35, 43), "wide wing over the pocket");
        // The fill is a cross-wise gradient — from one notch of the head to
        // the other — lighter above, darker below.
        let top = pixel(&bytes, 64, 38, 23);
        let bottom = pixel(&bytes, 64, 38, 41);
        assert!(top[0] > 230 && top[2] > 230, "lighter above: {top:?}");
        assert!(top[1] > 30, "light end blends toward white: {top:?}");
        assert!(
            bottom[0] < 215 && top[0] - bottom[0] > 25,
            "darker below: {bottom:?} vs {top:?}"
        );
        // A soft shadow darkens the base just below the body silhouette
        // (the silhouette itself ends at y≈35.5 here; the shadow peaks a
        // hair past it).
        let below = pixel(&bytes, 64, 20, 36);
        assert!(below[0] < 120 && below[2] < 120, "shadow below: {below:?}");
    }

    #[test]
    fn arrow_outer_edge_bows_out() {
        // The outer wing edge is a convex sweep, not a straight run into
        // the tip: the outline point nearest the barb→tip chord's midpoint
        // sits farther off the axis than the chord itself (by roughly the
        // curve's mid deviation, outer_bow · half_head / 2).
        let params = ArrowParams::DEFAULT;
        let (a, b) = (Point::new(8.0, 32.0), Point::new(56.0, 32.0));
        let outline = arrow_outline_with(params, a, b, 6.0);
        let frame = ArrowFrame::new(a, b, 6.0, params).expect("frame");
        let barb = frame.step(b, -frame.head_len, frame.half_head);
        let chord_mid = Point::new((barb.x + b.x) / 2.0, (barb.y + b.y) / 2.0);
        let edge_mid = outline
            .iter()
            .copied()
            .min_by(|p, q| {
                pt_dist(*p, chord_mid)
                    .partial_cmp(&pt_dist(*q, chord_mid))
                    .expect("finite distances")
            })
            .expect("non-empty outline");
        let bulge = params.outer_bow * frame.half_head / 2.0;
        assert!(
            (edge_mid.y - 32.0).abs() > (chord_mid.y - 32.0).abs() + bulge * 0.5,
            "edge mid {edge_mid:?} barely clears the chord mid {chord_mid:?}"
        );
    }

    #[test]
    fn arrow_params_reshape_the_outline() {
        // The knobs are live geometry, not just paint: a wider head
        // spreads the whole silhouette.
        let (a, b) = (Point::new(0.0, 0.0), Point::new(60.0, 0.0));
        let mut params = ArrowParams::DEFAULT;
        params.half_head_w = 4.0;
        let wide = arrow_outline_with(params, a, b, 6.0);
        params.half_head_w = 1.2;
        let narrow = arrow_outline_with(params, a, b, 6.0);
        let spread = |outline: &[Point]| outline.iter().map(|p| p.y.abs()).fold(0.0_f64, f64::max);
        assert!(
            spread(&wide) > spread(&narrow) + 5.0,
            "wide {} vs narrow {}",
            spread(&wide),
            spread(&narrow)
        );
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
