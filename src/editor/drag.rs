//! The editor's drag vocabulary: the in-progress mouse interaction, the
//! crop/resize frame state and the pending-callout phase, with the pure
//! math the canvas and the window handlers work in. No AppKit types —
//! everything here lives in image-pixel space, the space of `Point`/`Rect`.

use crate::editor::{ElementKind, Endpoint, Point, Rect, Rgba};

/// A drag shorter than this (in image pixels) is a plain click, not a
/// drawn element.
pub(crate) const MIN_DRAG_PX: f64 = 4.0;

/// Gap between a new callout's text block and the arrow base it sits beside,
/// in image pixels.
const CALLOUT_TEXT_OFFSET_PX: f64 = 16.0;

/// The in-progress mouse interaction; committed as one command on mouse-up.
pub(crate) enum Drag {
    /// Drawing a new element; live-previewed until mouse-up.
    Draw(ElementKind),
    /// Moving an existing element: `last` trails the cursor, the preview
    /// shifts the element by `last - start`.
    Move {
        id: u64,
        start: Point,
        last: Point,
        moved: bool,
    },
    /// Dragging one endpoint of an existing element to `last`.
    End {
        id: u64,
        endpoint: Endpoint,
        start: Point,
        last: Point,
    },
    /// Dragging a spot of the crop/resize frame; nothing is committed on
    /// mouse-up — the frame persists until OK/Enter applies or Esc cancels.
    Frame {
        spot: FrameSpot,
        orig: Rect,
        start: Point,
        last: Point,
    },
}

/// Which grabbable spot of the crop/resize frame is under the cursor.
#[derive(Clone, Copy)]
pub(crate) enum FrameSpot {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
    Center,
}

/// The active crop/resize frame, in image pixel space (always standardized).
pub(crate) struct FrameEdit {
    /// True for the resize tool (resample to the frame size), false for
    /// crop (cut the frame out).
    pub(crate) resize: bool,
    pub(crate) rect: Rect,
}

/// A callout whose arrow is drawn but whose text is still being entered:
/// the arrow previews on the canvas and the element is committed (or
/// dropped) together with the text.
pub(crate) struct PendingCallout {
    /// The arrow's start (the text block goes beside it).
    pub(crate) base: Point,
    pub(crate) tip: Point,
    pub(crate) color: Rgba,
    pub(crate) size: f64,
    /// The stroke the arrow was drawn with (the width slider's value).
    pub(crate) width: f64,
}

/// Distance between two points.
pub(crate) fn point_dist(a: Point, b: Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Whether a finished drag drew something real (as opposed to a plain
/// click, which must not produce zero-size elements).
pub(crate) fn draft_is_real(kind: &ElementKind) -> bool {
    match kind {
        ElementKind::Arrow { a, b, .. } | ElementKind::Line { a, b, .. } => {
            point_dist(*a, *b) >= MIN_DRAG_PX
        }
        ElementKind::Rect { rect, .. } | ElementKind::Blur { rect } => {
            let rect = rect.standardize();
            rect.width >= MIN_DRAG_PX && rect.height >= MIN_DRAG_PX
        }
        ElementKind::Pen { points, .. } => match (points.first(), points.last()) {
            (Some(first), Some(last)) => point_dist(*first, *last) >= MIN_DRAG_PX,
            _ => false,
        },
        ElementKind::Callout { anchor, tip, .. } => {
            point_dist(*anchor, *tip) >= MIN_DRAG_PX
        }
        _ => false,
    }
}

/// The crop/resize frame rect while dragging `spot` from the original rect
/// `orig`, with the drag start at `start` and the cursor at `cur`.
pub(crate) fn frame_rect(orig: Rect, spot: FrameSpot, start: Point, cur: Point) -> Rect {
    let r = orig.standardize();
    if matches!(spot, FrameSpot::Center) {
        return Rect::new(
            r.x + cur.x - start.x,
            r.y + cur.y - start.y,
            r.width,
            r.height,
        );
    }
    let (mut x0, mut y0, mut x1, mut y1) = (r.x, r.y, r.max_x(), r.max_y());
    match spot {
        FrameSpot::Nw => {
            x0 = cur.x;
            y0 = cur.y;
        }
        FrameSpot::N => y0 = cur.y,
        FrameSpot::Ne => {
            x1 = cur.x;
            y0 = cur.y;
        }
        FrameSpot::E => x1 = cur.x,
        FrameSpot::Se => {
            x1 = cur.x;
            y1 = cur.y;
        }
        FrameSpot::S => y1 = cur.y,
        FrameSpot::Sw => {
            x0 = cur.x;
            y1 = cur.y;
        }
        FrameSpot::W => x0 = cur.x,
        FrameSpot::Center => {}
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0).standardize()
}

/// Top-left of a new callout's text block (in image pixels): beside the
/// arrow's base, on the side away from the tip, so the growing text never
/// runs over the pointer. Along the dominant axis the block clears the base
/// by `CALLOUT_TEXT_OFFSET_PX`; across it, the block is centered on the
/// base. The whole block is clamped inside the frame (4 px margin) — near an
/// edge the block shifts inward, even if that brings it over the arrow.
pub(crate) fn callout_anchor(base: Point, tip: Point, block_w: f64, block_h: f64, frame: Rect) -> Point {
    let margin = 4.0;
    let gap = CALLOUT_TEXT_OFFSET_PX;
    let max_x = (frame.max_x() - block_w - margin).max(margin);
    let max_y = (frame.max_y() - block_h - margin).max(margin);
    let (dx, dy) = (tip.x - base.x, tip.y - base.y);
    let (x, y) = if dx.abs() >= dy.abs() {
        // A mostly horizontal arrow: the block goes left or right of the
        // base, vertically centered on it.
        let x = if dx >= 0.0 {
            base.x - gap - block_w
        } else {
            base.x + gap
        };
        (x, base.y - block_h / 2.0)
    } else {
        // A mostly vertical arrow: the block goes above or below the base,
        // horizontally centered on it.
        let y = if dy >= 0.0 {
            base.y - gap - block_h
        } else {
            base.y + gap
        };
        (base.x - block_w / 2.0, y)
    };
    Point::new(x.clamp(margin, max_x), y.clamp(margin, max_y))
}

/// The eight frame handles of a rect (image pixels), corners first for hit
/// priority. There is no center handle: for crop, drags that miss the
/// handles move the whole frame (`FrameSpot::Center`); resize has no move
/// at all.
pub(crate) fn spot_points(r: Rect) -> Vec<(FrameSpot, Point)> {
    let (cx, cy) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
    vec![
        (FrameSpot::Nw, Point::new(r.x, r.y)),
        (FrameSpot::Ne, Point::new(r.max_x(), r.y)),
        (FrameSpot::Se, Point::new(r.max_x(), r.max_y())),
        (FrameSpot::Sw, Point::new(r.x, r.max_y())),
        (FrameSpot::N, Point::new(cx, r.y)),
        (FrameSpot::E, Point::new(r.max_x(), cy)),
        (FrameSpot::S, Point::new(cx, r.max_y())),
        (FrameSpot::W, Point::new(r.x, cy)),
    ]
}

/// The frame spot of a rect under an image-space cursor within the per-axis
/// pixel tolerance, if any.
pub(crate) fn spot_at(r: Rect, p: Point, tol_x: f64, tol_y: f64) -> Option<(FrameSpot, Point)> {
    spot_points(r)
        .into_iter()
        .find(|(_, c)| (p.x - c.x).abs() <= tol_x && (p.y - c.y).abs() <= tol_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callout_anchor_sits_beside_the_base_away_from_the_tip() {
        let frame = Rect::new(0.0, 0.0, 400.0, 300.0);
        let base = Point::new(200.0, 150.0);
        let (w, h) = (100.0, 40.0);

        // A right-pointing arrow: the block goes left of the base,
        // vertically centered on it.
        let anchor = callout_anchor(base, Point::new(260.0, 150.0), w, h, frame);
        assert_eq!(anchor, Point::new(200.0 - 16.0 - w, 150.0 - h / 2.0));

        // Left-pointing: the block goes right of the base.
        let anchor = callout_anchor(base, Point::new(140.0, 150.0), w, h, frame);
        assert_eq!(anchor, Point::new(200.0 + 16.0, 150.0 - h / 2.0));

        // Down-pointing: the block goes above the base, horizontally
        // centered on it.
        let anchor = callout_anchor(base, Point::new(200.0, 220.0), w, h, frame);
        assert_eq!(anchor, Point::new(200.0 - w / 2.0, 150.0 - 16.0 - h));

        // Up-pointing: below the base.
        let anchor = callout_anchor(base, Point::new(200.0, 80.0), w, h, frame);
        assert_eq!(anchor, Point::new(200.0 - w / 2.0, 150.0 + 16.0));
    }

    #[test]
    fn callout_anchor_shifts_inward_at_frame_edges() {
        let frame = Rect::new(0.0, 0.0, 120.0, 200.0);
        // A left-pointing arrow near the left edge would push the block
        // past the right edge; the clamp shifts it fully inside.
        let anchor = callout_anchor(
            Point::new(30.0, 100.0),
            Point::new(5.0, 100.0),
            100.0,
            40.0,
            frame,
        );
        assert_eq!(anchor, Point::new(16.0, 80.0));

        // An up-pointing arrow near the bottom edge: the block below the
        // base does not fit and is clamped inside.
        let anchor = callout_anchor(
            Point::new(60.0, 180.0),
            Point::new(60.0, 120.0),
            100.0,
            40.0,
            frame,
        );
        assert_eq!(anchor, Point::new(10.0, 156.0));
    }
}
