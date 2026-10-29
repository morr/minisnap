//! Hit-testing with a tolerance: which element (and which of its ends) sits
//! under a position in base-frame pixel space.

use super::text;
use super::{Element, ElementKind, Endpoint, Point, Rect};

/// Extra pixels around the geometry that still count as a hit.
const TOLERANCE: f64 = 6.0;

/// Radius of the end zones (arrow/line/callout ends), in pixels.
const END_ZONE: f64 = 10.0;

/// What a click landed on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// The element body: drag to move, a second click edits its text.
    Body(u64),
    /// One end of an arrow/line/callout (for a callout: A = tip, B = anchor).
    End(u64, Endpoint),
}

/// Topmost (last in paint order) element hit at `point`, or None.
pub fn top_hit(elements: &[Element], point: Point) -> Option<Hit> {
    elements
        .iter()
        .rev()
        .find_map(|el| hit_element(el, point))
}

/// What this element yields at `point`, if anything.
pub fn hit_element(element: &Element, point: Point) -> Option<Hit> {
    match &element.kind {
        ElementKind::Arrow { a, b, width, .. } => {
            // Ends first: a click near the head or tail grabs the end, not
            // the body.
            if let Some(end) = hit_end(*a, *b, point) {
                return Some(Hit::End(element.id, end));
            }
            near_segment(point, *a, *b, *width / 2.0 + TOLERANCE)
                .then_some(Hit::Body(element.id))
        }
        ElementKind::Line { a, b, width, .. } => {
            if let Some(end) = hit_end(*a, *b, point) {
                return Some(Hit::End(element.id, end));
            }
            near_segment(point, *a, *b, *width / 2.0 + TOLERANCE)
                .then_some(Hit::Body(element.id))
        }
        ElementKind::Rect { rect, width, .. } => {
            let reach = *width / 2.0 + TOLERANCE;
            (dist_to_rect_outline(point, *rect) <= reach).then_some(Hit::Body(element.id))
        }
        ElementKind::Pen { points, width, .. } => {
            let reach = *width / 2.0 + TOLERANCE;
            let near = points
                .windows(2)
                .any(|pair| near_segment(point, pair[0], pair[1], reach));
            near.then_some(Hit::Body(element.id))
        }
        ElementKind::Text { origin, text, size, .. } => {
            let bounds = text_bounds(*origin, text, *size)?;
            bounds.contains(point).then_some(Hit::Body(element.id))
        }
        ElementKind::Callout { tip, anchor, text, size, .. } => {
            // Clicking the text block selects it (and later edits it), even
            // where it overlaps the anchor's end zone.
            let bounds = text_bounds(*anchor, text, *size)?;
            if bounds.contains(point) {
                return Some(Hit::Body(element.id));
            }
            if let Some(end) = hit_end(*tip, *anchor, point) {
                return Some(Hit::End(element.id, end));
            }
            near_segment(point, *anchor, *tip, TOLERANCE).then_some(Hit::Body(element.id))
        }
        ElementKind::Blur { rect } => {
            rect.standardize().contains(point).then_some(Hit::Body(element.id))
        }
    }
}

/// The end (if any) whose zone contains `point`; ties go to the nearer end.
fn hit_end(a: Point, b: Point, point: Point) -> Option<Endpoint> {
    let da = dist(point, a);
    let db = dist(point, b);
    if da <= END_ZONE && da <= db {
        Some(Endpoint::A)
    } else if db <= END_ZONE {
        Some(Endpoint::B)
    } else {
        None
    }
}

/// The bounding box of a text block drawn at `origin` (top-left corner).
pub fn text_bounds(origin: Point, text: &str, size: f64) -> Option<Rect> {
    let m = text::measure(text, size)?;
    Some(Rect::new(origin.x, origin.y, m.width, m.height()))
}

/// Distance from `point` to the closest point of segment `a`–`b`.
fn dist_to_segment(point: Point, a: Point, b: Point) -> f64 {
    let (vx, vy) = (b.x - a.x, b.y - a.y);
    let len2 = vx * vx + vy * vy;
    if len2 < f64::EPSILON {
        return dist(point, a);
    }
    let t = ((point.x - a.x) * vx + (point.y - a.y) * vy) / len2;
    let t = t.clamp(0.0, 1.0);
    let closest = Point::new(a.x + t * vx, a.y + t * vy);
    dist(point, closest)
}

fn near_segment(point: Point, a: Point, b: Point, reach: f64) -> bool {
    dist_to_segment(point, a, b) <= reach
}

fn dist(a: Point, b: Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Distance from `point` to the outline of `rect` (nearest edge when
/// inside, usual point-to-rect distance when outside).
fn dist_to_rect_outline(point: Point, rect: Rect) -> f64 {
    let r = rect.standardize();
    if r.contains(point) {
        p_min(
            p_min(point.x - r.x, r.max_x() - point.x),
            p_min(point.y - r.y, r.max_y() - point.y),
        )
    } else {
        let dx = if point.x < r.x {
            r.x - point.x
        } else if point.x > r.max_x() {
            point.x - r.max_x()
        } else {
            0.0
        };
        let dy = if point.y < r.y {
            r.y - point.y
        } else if point.y > r.max_y() {
            point.y - r.max_y()
        } else {
            0.0
        };
        (dx * dx + dy * dy).sqrt()
    }
}

fn p_min(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn arrow() -> Element {
        Element {
            id: 1,
            kind: ElementKind::Arrow {
                a: pt(10.0, 10.0),
                b: pt(60.0, 10.0),
                color: [1.0, 0.0, 1.0, 1.0],
                width: 4.0,
            },
        }
    }

    fn rect() -> Element {
        Element {
            id: 2,
            kind: ElementKind::Rect {
                rect: Rect::new(20.0, 40.0, 40.0, 30.0),
                color: [1.0, 0.0, 0.0, 1.0],
                width: 4.0,
            },
        }
    }

    fn text() -> Element {
        Element {
            id: 3,
            kind: ElementKind::Text {
                origin: pt(5.0, 80.0),
                text: "WW".into(),
                color: [1.0, 0.0, 1.0, 1.0],
                size: 32.0,
            },
        }
    }

    #[test]
    fn arrow_body_hits_on_axis_and_misses_off_axis() {
        let el = arrow();
        assert_eq!(hit_element(&el, pt(35.0, 12.0)), Some(Hit::Body(1)));
        assert_eq!(hit_element(&el, pt(35.0, 30.0)), None);
    }

    #[test]
    fn arrow_end_zone_beats_the_body() {
        let el = arrow();
        // Near the head: End(B), not Body.
        assert_eq!(hit_element(&el, pt(58.0, 13.0)), Some(Hit::End(1, Endpoint::B)));
        // Near the tail.
        assert_eq!(hit_element(&el, pt(12.0, 8.0)), Some(Hit::End(1, Endpoint::A)));
    }

    #[test]
    fn rect_outline_hits_but_hollow_middle_misses() {
        let el = rect();
        assert_eq!(hit_element(&el, pt(21.0, 55.0)), Some(Hit::Body(2)));
        assert_eq!(hit_element(&el, pt(40.0, 55.0)), None, "hollow middle");
    }

    #[test]
    fn pen_hits_along_the_trace() {
        let el = Element {
            id: 4,
            kind: ElementKind::Pen {
                points: vec![pt(0.0, 0.0), pt(30.0, 30.0)],
                color: [1.0, 0.0, 1.0, 1.0],
                width: 4.0,
            },
        };
        assert_eq!(hit_element(&el, pt(15.0, 16.0)), Some(Hit::Body(4)));
        assert_eq!(hit_element(&el, pt(30.0, 10.0)), None);
    }

    #[test]
    fn text_hits_inside_its_bbox_only() {
        let el = text();
        let bounds = text_bounds(pt(5.0, 80.0), "WW", 32.0).expect("bounds");
        assert!(bounds.width > 0.0 && bounds.height > 0.0);
        let inside = pt(5.0 + bounds.width / 2.0, 80.0 + bounds.height / 2.0);
        assert_eq!(hit_element(&el, inside), Some(Hit::Body(3)));
        let outside = pt(5.0 + bounds.width + 30.0, 80.0 + bounds.height / 2.0);
        assert_eq!(hit_element(&el, outside), None);
    }

    #[test]
    fn callout_separates_text_tip_and_anchor() {
        let el = Element {
            id: 5,
            kind: ElementKind::Callout {
                tip: pt(100.0, 20.0),
                anchor: pt(10.0, 20.0),
                text: "WW".into(),
                color: [1.0, 0.0, 1.0, 1.0],
                size: 32.0,
            },
        };
        // On the text block: body, even right next to the anchor.
        let bounds = text_bounds(pt(10.0, 20.0), "WW", 32.0).expect("bounds");
        let on_text = pt(10.0 + bounds.width / 2.0, 20.0 + bounds.height / 2.0);
        assert_eq!(hit_element(&el, on_text), Some(Hit::Body(5)));
        // Near the tip and near the anchor (left of the text block): ends.
        assert_eq!(hit_element(&el, pt(98.0, 22.0)), Some(Hit::End(5, Endpoint::A)));
        assert_eq!(hit_element(&el, pt(6.0, 24.0)), Some(Hit::End(5, Endpoint::B)));
        // On the pointer line between them: body.
        assert_eq!(hit_element(&el, pt(55.0, 20.0)), Some(Hit::Body(5)));
    }

    #[test]
    fn blur_hits_anywhere_inside() {
        let el = Element {
            id: 6,
            kind: ElementKind::Blur {
                rect: Rect::new(0.0, 0.0, 50.0, 50.0),
            },
        };
        assert_eq!(hit_element(&el, pt(25.0, 25.0)), Some(Hit::Body(6)));
        assert_eq!(hit_element(&el, pt(60.0, 60.0)), None);
    }

    #[test]
    fn top_hit_prefers_the_later_element() {
        // The top arrow overlaps the rect's outline band.
        let top_arrow = Element {
            id: 7,
            kind: ElementKind::Arrow {
                a: pt(20.0, 55.0),
                b: pt(60.0, 55.0),
                color: [0.0, 0.0, 1.0, 1.0],
                width: 4.0,
            },
        };
        let elements = [arrow(), rect(), top_arrow];
        // The point hits the top arrow's body (far from its ends) — the
        // later (topmost) element wins.
        assert_eq!(top_hit(&elements, pt(40.0, 57.0)), Some(Hit::Body(7)));
        // Away from the top arrow, the rect answers.
        assert_eq!(top_hit(&elements, pt(58.0, 42.0)), Some(Hit::Body(2)));
        assert_eq!(top_hit(&elements, pt(90.0, 90.0)), None);
    }
}
