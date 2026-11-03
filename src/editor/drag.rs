//! The editor's drag vocabulary: the in-progress mouse interaction, the
//! crop/resize frame state and the pending-callout phase, with the pure
//! math the canvas and the window handlers work in. No AppKit types —
//! everything here lives in image-pixel space, the space of `Point`/`Rect`.

use crate::editor::text;
use crate::editor::{
    clamp_crop_rect, set_endpoint, shift_element, Command, Element, ElementKind, Endpoint, Point,
    Rect, Rgba, Size, Tool,
};

/// A drag shorter than this (in image pixels) is a plain click, not a
/// drawn element.
pub(crate) const MIN_DRAG_PX: f64 = 4.0;

/// Gap between a new callout's text block and the arrow base it sits beside,
/// in image pixels.
const CALLOUT_TEXT_OFFSET_PX: f64 = 16.0;

/// Inward margin a text block keeps from the frame edges whenever it is
/// clamped (shared by `callout_anchor` and `clamp_block`), in image pixels.
const EDGE_MARGIN_PX: f64 = 4.0;

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
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// The open text entry: a brand-new text/callout element or the live
/// re-edit of an existing one. The machine owns the live string, so the
/// canvas draft and the committed element come from the same builder —
/// the commit freezes what is on screen instead of re-deriving it.
pub(crate) struct TextDraft {
    pub(crate) target: TextTarget,
    /// The active input's font size in image pixels (the slider's text
    /// mode may move it mid-edit).
    pub(crate) size: f64,
    pub(crate) text: String,
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
pub(crate) fn callout_anchor(
    base: Point,
    tip: Point,
    block_w: f64,
    block_h: f64,
    frame: Rect,
) -> Point {
    let margin = EDGE_MARGIN_PX;
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

/// Minimal shift of a block (`w` × `h`) whose top-left sits at `origin` so
/// it fits inside `frame` with the shared edge margin. A block wider or
/// taller than the frame parks at the margin on that axis, like
/// `callout_anchor`'s clamp. The axes shift independently, so a block
/// clamped at an edge by a previous, larger size does not spring back when
/// the size shrinks.
pub(crate) fn clamp_block(origin: Point, w: f64, h: f64, frame: Rect) -> Point {
    let margin = EDGE_MARGIN_PX;
    let min_x = frame.x + margin;
    let min_y = frame.y + margin;
    let max_x = (frame.max_x() - w - margin).max(min_x);
    let max_y = (frame.max_y() - h - margin).max(min_y);
    Point::new(origin.x.clamp(min_x, max_x), origin.y.clamp(min_y, max_y))
}

/// Measured block of `text` at `size`; the fallback (empty or
/// whitespace-only input, which measures no width) keeps the draft's
/// anchor math running before the first glyph lands.
pub(crate) fn block_size(text: &str, size: f64) -> (f64, f64) {
    text::measure_block(text, size)
        .map(|m| (m.width, m.height))
        .unwrap_or((size * 4.0, text::line_height(size)))
}

/// The draft — and the committed form — of a plain text element: the
/// measured block clamped into the frame. One builder serves the preview
/// and the commit, so Enter freezes what is on screen.
fn text_element_kind(
    origin: Point,
    text: &str,
    color: Rgba,
    size: f64,
    image: Size,
) -> ElementKind {
    let (w, h) = block_size(text, size);
    ElementKind::Text {
        origin: clamp_block(origin, w, h, Rect::new(0.0, 0.0, image.width, image.height)),
        text: text.to_string(),
        color,
        size,
    }
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

/// Font size of new text and callout elements, in image pixels.
pub(crate) const DEFAULT_TEXT_SIZE_PX: f64 = 32.0;

/// What the open field editor is editing; also the payload of
/// `Signal::BeginText`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TextTarget {
    /// A brand-new text or callout element; the color is frozen at the
    /// click, so a mid-edit palette change cannot rewrite it.
    New {
        origin: Point,
        callout: bool,
        color: Rgba,
    },
    /// An existing text/callout element being re-edited.
    Existing { id: u64 },
}

/// What a mouse-down decided about the element selection. The selection
/// itself stays window state — the markers and the toolbar ride on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Select {
    /// The click was consumed without touching the selection.
    Keep,
    /// Select this element.
    Element(u64),
    /// Deselect.
    Clear,
}

/// An AppKit follow-up the window owes the interaction.
#[derive(Debug, PartialEq)]
pub(crate) enum Signal {
    /// Open the field editor for this target (a brand-new text/callout
    /// element or an existing element's re-edit).
    BeginText(TextTarget),
    /// Viewing-mode press on empty image space: the window tracks the
    /// window move itself.
    DragWindow,
}

pub(crate) struct DownOutcome {
    pub(crate) selection: Select,
    pub(crate) signal: Option<Signal>,
}

pub(crate) struct UpOutcome {
    /// The journal command the drag committed, if any.
    pub(crate) command: Option<Command>,
    pub(crate) signal: Option<Signal>,
}

/// What a mouse-down needs beyond the point: the window's interaction
/// settings and a snapshot of the document elements for hit-testing.
pub(crate) struct DownCtx<'a> {
    pub(crate) tool: Option<Tool>,
    pub(crate) color: Rgba,
    pub(crate) stroke_width: f64,
    pub(crate) elements: &'a [Element],
    /// The active crop/resize frame rect, if the mode is up.
    pub(crate) frame: Option<Rect>,
    /// Frame-handle hit tolerance in image pixels, per axis (the window
    /// converts its view-space handle radius through the view scale).
    pub(crate) handle_tolerance: Point,
}

/// What to draw over the composite for the in-flight interaction.
pub(crate) struct Preview {
    /// The element to hide from the document layer (its dragged position
    /// is drawn from `extras` instead).
    pub(crate) hide: Option<u64>,
    /// View-only elements on top — the moved or endpoint-shifted element,
    /// the drawn draft, the pending callout's arrow — in draw order.
    pub(crate) extras: Vec<Element>,
}

/// The editor's interaction state machine: mouse events in image-pixel
/// space on one side, previews, journal commands and text-editing signals
/// on the other. Owns the three families of interaction state — the
/// in-flight drag, the two-phase callout and the crop/resize frame.
#[derive(Default)]
pub(crate) struct DragMachine {
    drag: Option<Drag>,
    /// The callout whose arrow is drawn and whose text is being entered —
    /// the second phase of the callout interaction (the first is the arrow
    /// drag in `drag`).
    callout: Option<PendingCallout>,
    /// The open text entry — opened together with `Signal::BeginText`,
    /// fed by `update_text`, closed by `commit_text`/`cancel_text`.
    text: Option<TextDraft>,
    frame: Option<FrameEdit>,
}

impl DragMachine {
    pub(crate) fn new() -> DragMachine {
        DragMachine::default()
    }

    /// Begin the interaction a mouse-down selects: a frame-handle grab for
    /// the crop/resize tools (the click never draws or selects there), a
    /// draw/move/endpoint drag for everything else, or a text-editing or
    /// window-move signal.
    pub(crate) fn down(&mut self, ctx: DownCtx<'_>, point: Point) -> DownOutcome {
        if matches!(ctx.tool, Some(Tool::Crop) | Some(Tool::Resize)) {
            let resize = ctx.tool == Some(Tool::Resize);
            let spot = match ctx
                .frame
                .and_then(|r| spot_at(r, point, ctx.handle_tolerance.x, ctx.handle_tolerance.y))
            {
                Some((spot, _)) => Some(spot),
                // Crop drags that miss the handles move the whole frame;
                // resize has no move at all.
                None if !resize => Some(FrameSpot::Center),
                None => None,
            };
            if let (Some(spot), Some(orig)) = (spot, ctx.frame) {
                self.drag = Some(Drag::Frame {
                    spot,
                    orig,
                    start: point,
                    last: point,
                });
            }
            return DownOutcome {
                selection: Select::Keep,
                signal: None,
            };
        }

        let draft_kind: Option<ElementKind> = match ctx.tool {
            Some(Tool::Arrow) => Some(ElementKind::Arrow {
                a: point,
                b: point,
                color: ctx.color,
                width: ctx.stroke_width,
            }),
            Some(Tool::Rect) => Some(ElementKind::Rect {
                rect: Rect::new(point.x, point.y, 0.0, 0.0),
                color: ctx.color,
                width: ctx.stroke_width,
            }),
            Some(Tool::Line) => Some(ElementKind::Line {
                a: point,
                b: point,
                color: ctx.color,
                width: ctx.stroke_width,
            }),
            Some(Tool::Pen) => Some(ElementKind::Pen {
                points: vec![point],
                color: ctx.color,
                width: ctx.stroke_width,
            }),
            Some(Tool::Blur) => Some(ElementKind::Blur {
                rect: Rect::new(point.x, point.y, 0.0, 0.0),
            }),
            // The callout draws its arrow by drag, from the base to the
            // tip, exactly like the arrow tool (same color, same stroke
            // width); the text follows in the second phase, at mouse-up.
            Some(Tool::Callout) => Some(ElementKind::Callout {
                tip: point,
                anchor: point,
                text: String::new(),
                color: ctx.color,
                size: DEFAULT_TEXT_SIZE_PX,
                width: ctx.stroke_width,
            }),
            // The text tool starts editing on the click (below); the frame
            // tools returned above, so nothing else produces a draft.
            _ => None,
        };

        // A hit on an existing element decides between moving it, dragging
        // one of its ends, and editing its text; the empty frame draws a
        // new element (or starts a text).
        let hit = crate::editor::hit_test::top_hit(ctx.elements, point);
        let (selection, drag) = match hit {
            Some(crate::editor::hit_test::Hit::End(id, endpoint)) => (
                Select::Element(id),
                Drag::End {
                    id,
                    endpoint,
                    start: point,
                    last: point,
                },
            ),
            Some(crate::editor::hit_test::Hit::Body(id)) => {
                // A click on editable text edits it in place; a click on a
                // callout's arrow moves the whole callout (text follows).
                if hit_edits_text(&ctx, id, point) {
                    let target = TextTarget::Existing { id };
                    self.begin_text(target, ctx.elements);
                    return DownOutcome {
                        selection: Select::Keep,
                        signal: Some(Signal::BeginText(target)),
                    };
                }
                if ctx.tool == Some(Tool::Text) && !id_is_textlike(ctx.elements, id) {
                    // The text tool starts a new text over any other
                    // element.
                    let target = TextTarget::New {
                        origin: point,
                        callout: false,
                        color: ctx.color,
                    };
                    self.begin_text(target, ctx.elements);
                    return DownOutcome {
                        selection: Select::Keep,
                        signal: Some(Signal::BeginText(target)),
                    };
                }
                (
                    Select::Element(id),
                    Drag::Move {
                        id,
                        start: point,
                        last: point,
                        moved: false,
                    },
                )
            }
            _ => {
                // Drawing on the empty frame starts from a clean selection;
                // a plain click here is the deselect rule. The text tool
                // opens its editor instead; in viewing mode the press moves
                // the window. The frame handles, the text field and the
                // elements have already claimed their clicks above.
                    match (ctx.tool, draft_kind) {
                    (Some(Tool::Text), _) => {
                        let target = TextTarget::New {
                            origin: point,
                            callout: false,
                            color: ctx.color,
                        };
                        self.begin_text(target, ctx.elements);
                        return DownOutcome {
                            selection: Select::Clear,
                            signal: Some(Signal::BeginText(target)),
                        };
                    }
                    (_, Some(kind)) => (Select::Clear, Drag::Draw(kind)),
                    (None, None) => {
                        return DownOutcome {
                            selection: Select::Clear,
                            signal: Some(Signal::DragWindow),
                        };
                    }
                    // A tool is up but produced no draft (cannot happen
                    // today): a plain deselect, the window stays put.
                    _ => {
                        return DownOutcome {
                            selection: Select::Clear,
                            signal: None,
                        };
                    }
                }
            }
        };
        self.drag = Some(drag);
        DownOutcome {
            selection,
            signal: None,
        }
    }

    /// Move the drag's trailing point (or grow the drawn trace) to `point`.
    /// False when no drag is in progress.
    pub(crate) fn drag_to(&mut self, point: Point) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        match drag {
            Drag::Draw(kind) => match kind {
                ElementKind::Arrow { b, .. } | ElementKind::Line { b, .. } => *b = point,
                // The callout's tip follows the cursor; the anchor (its
                // base) stays at the drag start.
                ElementKind::Callout { tip, .. } => *tip = point,
                // The drag start stays the origin; the size may go negative
                // and is standardized at draw/commit time.
                ElementKind::Rect { rect, .. } | ElementKind::Blur { rect } => {
                    *rect = Rect::new(rect.x, rect.y, point.x - rect.x, point.y - rect.y);
                }
                ElementKind::Pen { points, .. } if points.last() != Some(&point) => {
                    points.push(point);
                }
                _ => {}
            },
            Drag::Move { start, last, moved, .. } => {
                *last = point;
                if point_dist(*start, point) >= MIN_DRAG_PX {
                    *moved = true;
                }
            }
            Drag::End { last, .. } => *last = point,
            Drag::Frame { last, .. } => *last = point,
        }
        true
    }

    /// Commit the drag at `point` — the up-point is the drag's final
    /// trailing position even when no dragged event preceded it.
    /// `next_id` is the document's next free element id — the machine
    /// never invents ids. None when no drag is in progress.
    pub(crate) fn up(&mut self, point: Point, next_id: u64) -> Option<UpOutcome> {
        self.drag_to(point);
        let drag = self.drag.take()?;
        let mut outcome = UpOutcome {
            command: None,
            signal: None,
        };
        match drag {
            Drag::Draw(kind) => {
                if !draft_is_real(&kind) {
                    // Plain click: the draft is discarded and the selection
                    // was already cleared at mouse-down.
                    return Some(outcome);
                }
                // A callout splits into two phases: the drawn arrow stays
                // up as the pending preview while the text is entered at
                // its base; the element lands (or is dropped) with the text.
                if let ElementKind::Callout {
                    anchor,
                    tip,
                    color,
                    size,
                    width,
                    text: _,
                } = kind
                {
                    self.callout = Some(PendingCallout {
                        base: anchor,
                        tip,
                        color,
                        size,
                        width,
                    });
                    let target = TextTarget::New {
                        origin: anchor,
                        callout: true,
                        color,
                    };
                    // A new draft carries no document text; elements stay
                    // untouched here.
                    self.begin_text(target, &[]);
                    outcome.signal = Some(Signal::BeginText(target));
                    return Some(outcome);
                }
                // The fresh element stays unselected: drawing must not end
                // in markers and a snapped slider — a click selects it.
                outcome.command = Some(Command::AddElement(Element { id: next_id, kind }));
            }
            Drag::Move {
                id,
                start,
                last,
                moved,
            } => {
                if moved {
                    outcome.command = Some(Command::MoveElement(
                        id,
                        Point::new(last.x - start.x, last.y - start.y),
                    ));
                }
            }
            Drag::End {
                id,
                endpoint,
                start,
                last,
            } => {
                if point_dist(start, last) >= MIN_DRAG_PX {
                    outcome.command = Some(Command::MovePoint(id, endpoint, last));
                }
            }
            Drag::Frame {
                spot,
                orig,
                start,
                last,
            } => {
                // Persist the settled rect; nothing is applied yet — the
                // OK button and Enter do that.
                if let Some(edit) = self.frame.as_mut() {
                    edit.rect = frame_rect(orig, spot, start, last);
                }
            }
        }
        Some(outcome)
    }

    /// What to draw over the composite for the in-flight interaction.
    pub(crate) fn preview(&self, elements: &[Element], image: Size) -> Preview {
        let mut preview = Preview {
            hide: None,
            extras: Vec::new(),
        };
        match self.drag.as_ref() {
            Some(Drag::Move { id, start, last, .. }) => {
                if let Some(mut el) = elements.iter().find(|el| el.id == *id).cloned() {
                    shift_element(&mut el.kind, last.x - start.x, last.y - start.y);
                    preview.hide = Some(*id);
                    preview.extras.push(el);
                }
            }
            Some(Drag::End { id, endpoint, last, .. }) => {
                if let Some(mut el) = elements.iter().find(|el| el.id == *id).cloned() {
                    set_endpoint(&mut el.kind, *endpoint, *last);
                    preview.hide = Some(*id);
                    preview.extras.push(el);
                }
            }
            Some(Drag::Draw(kind)) => {
                preview.extras.push(Element {
                    id: 0,
                    kind: kind.clone(),
                });
            }
            // The frame drag previews through the frame overlay, not here.
            Some(Drag::Frame { .. }) | None => {}
        }
        // The open text entry draws its draft as the committed element:
        // live text in the measured, clamped geometry — halo and all. The
        // element being re-edited hides behind its draft.
        if let Some(kind) = self.text_draft_kind(elements, image) {
            if let Some(TextTarget::Existing { id }) = self.text.as_ref().map(|d| d.target) {
                preview.hide = Some(id);
            }
            preview.extras.push(Element { id: 0, kind });
        } else if let Some(pending) = self.callout.as_ref() {
            // A pending callout without a text entry (a transient state —
            // the entry opens with the signal): the bare arrow at its base.
            preview.extras.push(Element {
                id: 0,
                kind: ElementKind::Callout {
                    tip: pending.tip,
                    anchor: pending.base,
                    text: String::new(),
                    color: pending.color,
                    size: pending.size,
                    width: pending.width,
                },
            });
        }
        preview
    }

    /// Bring up the crop (`resize = false`) or resize frame with its
    /// initial rect: the whole image for crop, the central 75% for resize.
    pub(crate) fn begin_frame(&mut self, resize: bool, image: Size) {
        let rect = if resize {
            let (w, h) = (image.width * 0.75, image.height * 0.75);
            Rect::new((image.width - w) / 2.0, (image.height - h) / 2.0, w, h)
        } else {
            Rect::new(0.0, 0.0, image.width, image.height)
        };
        self.frame = Some(FrameEdit { resize, rect });
    }

    /// Tear the frame mode down. True when a frame was up.
    pub(crate) fn end_frame(&mut self) -> bool {
        self.frame.take().is_some()
    }

    /// Whether the crop/resize frame mode is up.
    pub(crate) fn frame_active(&self) -> bool {
        self.frame.is_some()
    }

    /// The frame rect in image pixels, including an in-flight drag.
    pub(crate) fn current_frame(&self) -> Option<Rect> {
        match self.drag.as_ref() {
            Some(Drag::Frame {
                spot,
                orig,
                start,
                last,
            }) => Some(frame_rect(*orig, *spot, *start, *last)),
            _ => self.frame.as_ref().map(|edit| edit.rect),
        }
    }

    /// The eight frame handles (image pixels), corners first for hit
    /// priority.
    pub(crate) fn frame_spot_points(&self) -> Vec<(FrameSpot, Point)> {
        match self.current_frame() {
            Some(r) => spot_points(r),
            None => Vec::new(),
        }
    }

    /// Apply the frame as a Crop or Resize command, clamped into the image
    /// and rounded to whole pixels. None keeps the frame up for another
    /// try: the clamped rect is degenerate (under a pixel).
    pub(crate) fn commit_frame(&mut self, image: Size) -> Option<Command> {
        let rect = clamp_crop_rect(self.frame.as_ref()?.rect, image.width, image.height);
        let (w, h) = (rect.width.round(), rect.height.round());
        if w < 1.0 || h < 1.0 {
            return None;
        }
        let edit = self.frame.take()?;
        Some(if edit.resize {
            Command::Resize(Size::new(w, h))
        } else {
            Command::Crop(Rect::new(rect.x, rect.y, w, h))
        })
    }

    /// Where the pending callout's text block — or its field editor — sits
    /// (in image pixels): `callout_anchor` against the pending arrow, with
    /// `block` already in image pixels.
    pub(crate) fn pending_callout_anchor(&self, block: Size, image: Size) -> Option<Point> {
        let pending = self.callout.as_ref()?;
        Some(callout_anchor(
            pending.base,
            pending.tip,
            block.width,
            block.height,
            Rect::new(0.0, 0.0, image.width, image.height),
        ))
    }

    /// Resize the pending callout's draft text (the size slider's text-mode
    /// drag in the second phase) and re-anchor the block beside the arrow
    /// base for the new size — the same placement `pending_callout_anchor`
    /// computes. `block` is the field's block at the new size, already in
    /// image pixels (the window measures its own field). Returns the new
    /// block position; None without a pending callout. The arrow's tip and
    /// stroke width stay as drawn.
    pub(crate) fn set_pending_callout_size(
        &mut self,
        size: f64,
        block: Size,
        image: Size,
    ) -> Option<Point> {
        let pending = self.callout.as_mut()?;
        pending.size = size;
        if let Some(draft) = self.text.as_mut() {
            draft.size = size;
        }
        Some(callout_anchor(
            pending.base,
            pending.tip,
            block.width,
            block.height,
            Rect::new(0.0, 0.0, image.width, image.height),
        ))
    }

    /// Open a text entry for `target`: a new element starts empty at the
    /// default size; an existing one carries its text and size (the field
    /// editor still reads the initial string from the document for its
    /// prefill). Called as the matching `Signal::BeginText` goes out.
    pub(crate) fn begin_text(&mut self, target: TextTarget, elements: &[Element]) {
        let draft = match target {
            TextTarget::New { .. } => TextDraft {
                target,
                size: DEFAULT_TEXT_SIZE_PX,
                text: String::new(),
            },
            TextTarget::Existing { id } => {
                let Some((text, size)) = elements.iter().find(|el| el.id == id).and_then(|el| {
                    match &el.kind {
                        ElementKind::Text { text, size, .. }
                        | ElementKind::Callout { text, size, .. } => {
                            Some((text.clone(), *size))
                        }
                        _ => None,
                    }
                }) else {
                    return;
                };
                TextDraft { target, size, text }
            }
        };
        self.text = Some(draft);
    }

    /// Feed the field editor's live string into the draft — every
    /// keystroke; the canvas re-renders the draft from it.
    pub(crate) fn update_text(&mut self, text: &str) {
        if let Some(draft) = self.text.as_mut() {
            draft.text = text.to_string();
        }
    }

    /// The size slider's text-mode change mid-edit: resize the active
    /// draft (and the pending callout, whose anchor math re-runs at the
    /// new size).
    pub(crate) fn update_text_size(&mut self, size: f64) {
        if let Some(draft) = self.text.as_mut() {
            draft.size = size;
        }
        if let Some(pending) = self.callout.as_mut() {
            pending.size = size;
        }
    }

    /// The draft element exactly as `commit_text` will freeze it: the live
    /// text in the committed geometry — a new block measured and clamped
    /// into the frame, the pending callout anchored beside its base, or
    /// the existing element's kind with the live text. None without an
    /// open entry. The window also reads this to place the field editor
    /// over the block.
    pub(crate) fn text_draft_kind(
        &self,
        elements: &[Element],
        image: Size,
    ) -> Option<ElementKind> {
        let draft = self.text.as_ref()?;
        match draft.target {
            TextTarget::New {
                origin,
                callout: false,
                color,
            } => Some(text_element_kind(
                origin,
                &draft.text,
                color,
                draft.size,
                image,
            )),
            TextTarget::New { callout: true, .. } => {
                let pending = self.callout.as_ref()?;
                let (w, h) = block_size(&draft.text, pending.size);
                Some(ElementKind::Callout {
                    tip: pending.tip,
                    anchor: callout_anchor(
                        pending.base,
                        pending.tip,
                        w,
                        h,
                        Rect::new(0.0, 0.0, image.width, image.height),
                    ),
                    text: draft.text.clone(),
                    color: pending.color,
                    size: pending.size,
                    width: pending.width,
                })
            }
            TextTarget::Existing { id } => {
                let el = elements.iter().find(|el| el.id == id)?;
                match &el.kind {
                    ElementKind::Text {
                        origin,
                        text: _,
                        color,
                        size,
                    } => Some(text_element_kind(
                        *origin,
                        &draft.text,
                        *color,
                        *size,
                        image,
                    )),
                    // The authored anchor holds; the block only re-clamps
                    // into the frame as the edited text grows.
                    ElementKind::Callout {
                        tip,
                        anchor,
                        text: _,
                        color,
                        size,
                        width,
                    } => {
                        let (w, h) = block_size(&draft.text, *size);
                        Some(ElementKind::Callout {
                            tip: *tip,
                            anchor: clamp_block(
                                *anchor,
                                w,
                                h,
                                Rect::new(0.0, 0.0, image.width, image.height),
                            ),
                            text: draft.text.clone(),
                            color: *color,
                            size: *size,
                            width: *width,
                        })
                    }
                    _ => None,
                }
            }
        }
    }

    /// Close the text entry — the end of every text interaction: a
    /// non-empty new text or callout lands as the element the draft showed
    /// (same builder, so Enter cannot move it); an emptied existing
    /// element deletes; an unchanged or empty new input changes nothing.
    /// None when there is no open entry or nothing to journal.
    pub(crate) fn commit_text(
        &mut self,
        elements: &[Element],
        image: Size,
        next_id: u64,
    ) -> Option<Command> {
        let draft = self.text.as_ref()?;
        let kind = self.text_draft_kind(elements, image);
        let text = draft.text.clone();
        let target = draft.target;
        // The entry — and its pending callout — close whatever the input.
        self.text = None;
        self.callout = None;
        match target {
            TextTarget::Existing { id } => {
                if text.trim().is_empty() {
                    return Some(Command::DeleteElement(id));
                }
                let changed = elements
                    .iter()
                    .find(|el| el.id == id)
                    .and_then(|el| match &el.kind {
                        ElementKind::Text { text: old, .. }
                        | ElementKind::Callout { text: old, .. } => {
                            Some(old.as_str() != text.as_str())
                        }
                        _ => None,
                    })
                    .unwrap_or(true);
                changed.then_some(Command::EditText(id, text))
            }
            TextTarget::New { .. } => {
                if text.trim().is_empty() {
                    return None;
                }
                kind.map(|kind| Command::AddElement(Element { id: next_id, kind }))
            }
        }
    }

    /// Drop the open text entry without committing (cancelled input, a new
    /// shot arrived), together with its pending callout.
    pub(crate) fn cancel_text(&mut self) {
        self.text = None;
        self.callout = None;
    }
}

/// Whether a Body hit on element `id` at `point` is a click on editable
/// text: a callout's text block (any tool) or a text element (text tools
/// only).
fn hit_edits_text(ctx: &DownCtx<'_>, id: u64, point: Point) -> bool {
    match ctx.elements.iter().find(|el| el.id == id).map(|el| &el.kind) {
        Some(ElementKind::Callout {
            anchor, text, size, ..
        }) => crate::editor::hit_test::text_bounds(*anchor, text, *size)
            .is_some_and(|b| b.contains(point)),
        Some(ElementKind::Text { .. }) => {
            matches!(ctx.tool, Some(Tool::Text) | Some(Tool::Callout))
        }
        _ => false,
    }
}

/// Whether the element is a text or callout element.
fn id_is_textlike(elements: &[Element], id: u64) -> bool {
    elements
        .iter()
        .find(|el| el.id == id)
        .is_some_and(|el| matches!(el.kind, ElementKind::Text { .. } | ElementKind::Callout { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::hit_test::text_bounds;

    const RED: Rgba = [1.0, 0.0, 0.0, 1.0];

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    /// A viewing-mode context: no tool, no frame, empty document.
    fn ctx(elements: &[Element]) -> DownCtx<'_> {
        DownCtx {
            tool: None,
            color: RED,
            stroke_width: 4.0,
            elements,
            frame: None,
            handle_tolerance: Point::new(8.0, 8.0),
        }
    }

    fn tool_ctx(mut base: DownCtx<'_>, tool: Tool) -> DownCtx<'_> {
        base.tool = Some(tool);
        base
    }

    fn rect_element(id: u64) -> Element {
        Element {
            id,
            kind: ElementKind::Rect {
                rect: Rect::new(20.0, 40.0, 40.0, 30.0),
                color: RED,
                width: 4.0,
            },
        }
    }

    fn line_element(id: u64) -> Element {
        Element {
            id,
            kind: ElementKind::Line {
                a: pt(10.0, 10.0),
                b: pt(50.0, 50.0),
                color: RED,
                width: 4.0,
            },
        }
    }

    fn text_element(id: u64) -> Element {
        Element {
            id,
            kind: ElementKind::Text {
                origin: pt(5.0, 80.0),
                text: "WW".into(),
                color: RED,
                size: 32.0,
            },
        }
    }

    fn assert_add(command: Option<Command>, id: u64) -> Element {
        match command {
            Some(Command::AddElement(el)) => {
                assert_eq!(el.id, id, "the machine must use the passed id");
                el
            }
            other => panic!("expected AddElement, got {other:?}"),
        }
    }

    #[test]
    fn short_drag_with_any_draw_tool_is_a_click_without_an_element() {
        for tool in [Tool::Arrow, Tool::Rect, Tool::Line, Tool::Pen, Tool::Blur, Tool::Callout] {
            let mut machine = DragMachine::new();
            let elements: Vec<Element> = Vec::new();
            let outcome = machine.down(tool_ctx(ctx(&elements), tool), pt(10.0, 10.0));
            assert_eq!(outcome.selection, Select::Clear);
            machine.drag_to(pt(12.0, 11.0));
            let outcome = machine.up(pt(12.0, 11.0), 1).expect("drag was active");
            assert_eq!(outcome.command, None, "a 2 px drag is a plain click");
            assert_eq!(outcome.signal, None, "no text phase for a short callout");
            assert!(
                machine.preview(&elements, Size::new(400.0, 300.0)).extras.is_empty(),
                "nothing previews after a click"
            );
        }
    }

    #[test]
    fn real_drag_commits_the_drawn_element_with_the_passed_id() {
        let mut machine = DragMachine::new();
        let elements: Vec<Element> = Vec::new();
        machine.down(tool_ctx(ctx(&elements), Tool::Arrow), pt(10.0, 10.0));
        machine.drag_to(pt(50.0, 60.0));
        let outcome = machine.up(pt(50.0, 60.0), 7).expect("drag was active");
        let el = assert_add(outcome.command, 7);
        match el.kind {
            ElementKind::Arrow { a, b, color, width } => {
                assert_eq!((a.x, a.y), (10.0, 10.0));
                assert_eq!((b.x, b.y), (50.0, 60.0));
                assert_eq!(color, RED);
                assert_eq!(width, 4.0);
            }
            other => panic!("expected arrow, got {other:?}"),
        }
        // The fresh element stays unselected: no markers after drawing.
        assert_eq!(outcome.signal, None);
    }

    #[test]
    fn move_element_needs_a_real_drag_and_reports_the_delta() {
        let elements = vec![rect_element(3)];
        // Two pixels: a click, not a move.
        let mut machine = DragMachine::new();
        let outcome = machine.down(ctx(&elements), pt(21.0, 55.0));
        assert_eq!(outcome.selection, Select::Element(3));
        machine.drag_to(pt(23.0, 55.0));
        let outcome = machine.up(pt(23.0, 55.0), 1).expect("drag was active");
        assert_eq!(outcome.command, None, "a 2 px drag moves nothing");

        // Forty pixels: one MoveElement with the whole delta.
        let mut machine = DragMachine::new();
        machine.down(ctx(&elements), pt(21.0, 55.0));
        machine.drag_to(pt(45.0, 56.0));
        machine.drag_to(pt(61.0, 55.0));
        let outcome = machine.up(pt(61.0, 55.0), 1).expect("drag was active");
        match outcome.command {
            Some(Command::MoveElement(id, delta)) => {
                assert_eq!(id, 3);
                assert_eq!((delta.x, delta.y), (40.0, 0.0));
            }
            other => panic!("expected MoveElement, got {other:?}"),
        }
    }

    #[test]
    fn endpoint_drag_moves_only_that_end() {
        let elements = vec![line_element(5)];
        let mut machine = DragMachine::new();
        // The B end zone of the line (within 10 px of (50, 50)).
        let outcome = machine.down(ctx(&elements), pt(49.0, 49.0));
        assert_eq!(outcome.selection, Select::Element(5));
        machine.drag_to(pt(80.0, 20.0));
        let outcome = machine.up(pt(80.0, 20.0), 1).expect("drag was active");
        match outcome.command {
            Some(Command::MovePoint(id, endpoint, to)) => {
                assert_eq!(id, 5);
                assert_eq!(endpoint, Endpoint::B);
                assert_eq!(to, pt(80.0, 20.0));
            }
            other => panic!("expected MovePoint, got {other:?}"),
        }
    }

    #[test]
    fn viewing_press_on_empty_space_signals_window_drag() {
        let mut machine = DragMachine::new();
        let elements: Vec<Element> = Vec::new();
        let outcome = machine.down(ctx(&elements), pt(100.0, 100.0));
        assert_eq!(outcome.selection, Select::Clear);
        assert!(matches!(outcome.signal, Some(Signal::DragWindow)));
        // No drag state behind the press: mouse-up is a no-op.
        assert!(machine.up(pt(110.0, 110.0), 1).is_none());
    }

    #[test]
    fn text_tool_starts_new_text_on_empty_and_over_shapes() {
        let elements = vec![rect_element(2)];
        // On the empty frame: the text editor opens, the selection clears.
        let mut machine = DragMachine::new();
        let outcome = machine.down(tool_ctx(ctx(&elements), Tool::Text), pt(100.0, 100.0));
        assert_eq!(outcome.selection, Select::Clear);
        assert!(matches!(
            outcome.signal,
            Some(Signal::BeginText(TextTarget::New { callout: false, .. }))
        ));
        // Over a non-text element: a new text still starts, but the
        // selection is untouched.
        let mut machine = DragMachine::new();
        let outcome = machine.down(tool_ctx(ctx(&elements), Tool::Text), pt(21.0, 55.0));
        assert_eq!(outcome.selection, Select::Keep);
        assert!(matches!(
            outcome.signal,
            Some(Signal::BeginText(TextTarget::New { callout: false, .. }))
        ));
        assert!(machine.up(pt(21.0, 55.0), 1).is_none());
    }

    #[test]
    fn click_on_a_text_element_edits_it_only_for_text_tools() {
        let elements = vec![text_element(4)];
        let bounds = text_bounds(pt(5.0, 80.0), "WW", 32.0).expect("bounds");
        let inside = pt(5.0 + bounds.width / 2.0, 80.0 + bounds.height / 2.0);
        // A text tool edits in place.
        let mut machine = DragMachine::new();
        let outcome = machine.down(tool_ctx(ctx(&elements), Tool::Text), inside);
        assert_eq!(outcome.selection, Select::Keep);
        assert_eq!(
            outcome.signal,
            Some(Signal::BeginText(TextTarget::Existing { id: 4 }))
        );
        // Any other tool moves the element instead.
        let mut machine = DragMachine::new();
        let outcome = machine.down(tool_ctx(ctx(&elements), Tool::Arrow), inside);
        assert_eq!(outcome.selection, Select::Element(4));
        assert_eq!(outcome.signal, None);
    }

    #[test]
    fn draft_is_real_rules() {
        let stroke = |kind| draft_is_real(&kind);
        // Rect: zero size is a click, both dimensions must clear the bar.
        assert!(!stroke(ElementKind::Rect {
            rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            color: RED,
            width: 4.0,
        }));
        assert!(!stroke(ElementKind::Rect {
            rect: Rect::new(0.0, 0.0, 10.0, 2.0),
            color: RED,
            width: 4.0,
        }));
        assert!(stroke(ElementKind::Rect {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            color: RED,
            width: 4.0,
        }));
        // Pen: a single point is a click; distant ends are a stroke.
        assert!(!stroke(ElementKind::Pen {
            points: vec![pt(1.0, 1.0)],
            color: RED,
            width: 4.0,
        }));
        assert!(stroke(ElementKind::Pen {
            points: vec![pt(1.0, 1.0), pt(1.0, 2.0), pt(10.0, 10.0)],
            color: RED,
            width: 4.0,
        }));
        // Text drafts never come from drags.
        assert!(!stroke(ElementKind::Text {
            origin: pt(0.0, 0.0),
            text: String::new(),
            color: RED,
            size: 32.0,
        }));
    }

    #[test]
    fn frame_rect_math_standardizes_and_moves_whole() {
        let orig = Rect::new(20.0, 20.0, 60.0, 40.0);
        // An NW handle dragged past the opposite corner: standardized, not
        // negative.
        let flipped = frame_rect(orig, FrameSpot::Nw, pt(20.0, 20.0), pt(100.0, 80.0));
        assert_eq!(flipped, Rect::new(80.0, 60.0, 20.0, 20.0));
        // The center spot translates the whole rect.
        let moved = frame_rect(orig, FrameSpot::Center, pt(50.0, 40.0), pt(60.0, 55.0));
        assert_eq!(moved, Rect::new(30.0, 35.0, 60.0, 40.0));
        // An edge handle moves one edge only.
        let east = frame_rect(orig, FrameSpot::E, pt(80.0, 40.0), pt(30.0, 40.0));
        assert_eq!(east, Rect::new(20.0, 20.0, 10.0, 40.0));
    }

    #[test]
    fn spot_at_honors_the_per_axis_tolerance() {
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(spot_at(r, pt(2.0, -1.0), 8.0, 8.0), Some((FrameSpot::Nw, pt(0.0, 0.0))));
        assert_eq!(
            spot_at(r, pt(99.0, 101.0), 8.0, 8.0),
            Some((FrameSpot::Se, pt(100.0, 100.0)))
        );
        assert_eq!(spot_at(r, pt(50.0, 50.0), 8.0, 8.0), None, "no center handle");
        assert_eq!(spot_at(r, pt(20.0, 20.0), 4.0, 4.0), None, "outside the tolerance");
    }

    #[test]
    fn crop_frame_roundtrip_settles_and_commits() {
        let image = Size::new(200.0, 100.0);
        let mut machine = DragMachine::new();
        machine.begin_frame(false, image);
        assert!(machine.frame_active());
        assert_eq!(machine.current_frame(), Some(Rect::new(0.0, 0.0, 200.0, 100.0)));

        // Grab the SE handle (cursor at the corner, tolerance 8 px) and
        // pull in.
        let frame = machine.current_frame();
        let ctx = DownCtx {
            tool: Some(Tool::Crop),
            frame,
            ..ctx(&[])
        };
        let outcome = machine.down(ctx, pt(200.0, 100.0));
        assert_eq!(outcome.selection, Select::Keep, "frame clicks never select");
        machine.drag_to(pt(150.0, 80.0));
        // The in-flight rect previews.
        assert_eq!(machine.current_frame(), Some(Rect::new(0.0, 0.0, 150.0, 80.0)));
        // Settling commits nothing — the OK button and Enter do.
        let outcome = machine.up(pt(150.0, 80.0), 1).expect("drag was active");
        assert_eq!(outcome.command, None);
        assert_eq!(machine.current_frame(), Some(Rect::new(0.0, 0.0, 150.0, 80.0)));

        let command = machine.commit_frame(image);
        assert_eq!(command, Some(Command::Crop(Rect::new(0.0, 0.0, 150.0, 80.0))));
        assert!(!machine.frame_active(), "the mode is over");
    }

    #[test]
    fn frame_commit_clamps_into_the_image() {
        let image = Size::new(200.0, 100.0);
        let mut machine = DragMachine::new();
        machine.begin_frame(false, image);
        let frame = machine.current_frame();
        let ctx = DownCtx {
            tool: Some(Tool::Crop),
            frame,
            ..ctx(&[])
        };
        machine.down(ctx, pt(200.0, 100.0));
        // Dragging past the image edges: the clamp presses the rect back.
        machine.drag_to(pt(250.0, 150.0));
        machine.up(pt(250.0, 150.0), 1).expect("drag was active");
        assert_eq!(
            machine.commit_frame(image),
            Some(Command::Crop(Rect::new(0.0, 0.0, 200.0, 100.0)))
        );
    }

    #[test]
    fn degenerate_frame_stays_up_for_another_try() {
        let image = Size::new(200.0, 100.0);
        let mut machine = DragMachine::new();
        machine.begin_frame(false, image);
        let frame = machine.current_frame();
        let ctx = DownCtx {
            tool: Some(Tool::Crop),
            frame,
            ..ctx(&[])
        };
        machine.down(ctx, pt(200.0, 100.0));
        // Collapse the frame onto its own corner: under a pixel.
        machine.drag_to(pt(0.0, 0.0));
        machine.up(pt(0.0, 0.0), 1).expect("drag was active");
        assert_eq!(machine.commit_frame(image), None, "degenerate frames wait");
        assert!(machine.frame_active(), "the mode keeps running");
    }

    #[test]
    fn resize_frame_commits_a_resize_command() {
        let image = Size::new(160.0, 120.0);
        let mut machine = DragMachine::new();
        machine.begin_frame(true, image);
        // The initial rect: the central 75%.
        assert_eq!(machine.current_frame(), Some(Rect::new(20.0, 15.0, 120.0, 90.0)));
        let frame = machine.current_frame();
        let ctx = DownCtx {
            tool: Some(Tool::Resize),
            frame,
            ..ctx(&[])
        };
        machine.down(ctx, pt(140.0, 105.0));
        machine.drag_to(pt(80.0, 75.0));
        machine.up(pt(80.0, 75.0), 1).expect("drag was active");
        assert_eq!(
            machine.commit_frame(image),
            Some(Command::Resize(Size::new(60.0, 60.0)))
        );
    }

    #[test]
    fn crop_frame_miss_grabs_whole_resize_miss_grabs_nothing() {
        let image = Size::new(160.0, 120.0);
        // Crop: a click far from every handle moves the whole frame.
        let mut machine = DragMachine::new();
        machine.begin_frame(false, image);
        let crop_ctx = DownCtx {
            tool: Some(Tool::Crop),
            frame: machine.current_frame(),
            ..ctx(&[])
        };
        machine.down(crop_ctx, pt(60.0, 50.0));
        machine.drag_to(pt(70.0, 55.0));
        machine.up(pt(70.0, 55.0), 1).expect("drag was active");
        assert_eq!(
            machine.current_frame(),
            Some(Rect::new(10.0, 5.0, 160.0, 120.0)),
            "the whole frame followed the drag"
        );

        // Resize: a miss away from the handles starts no drag at all.
        let mut machine = DragMachine::new();
        machine.begin_frame(true, image);
        let resize_ctx = DownCtx {
            tool: Some(Tool::Resize),
            frame: machine.current_frame(),
            ..ctx(&[])
        };
        machine.down(resize_ctx, pt(60.0, 50.0));
        machine.drag_to(pt(70.0, 55.0));
        assert!(machine.up(pt(70.0, 55.0), 1).is_none(), "no drag was started");
        assert_eq!(
            machine.current_frame(),
            Some(Rect::new(20.0, 15.0, 120.0, 90.0)),
            "the frame never moved"
        );
    }

    #[test]
    fn callout_up_opens_the_text_phase_and_commit_lands_the_element() {
        let mut machine = DragMachine::new();
        let elements: Vec<Element> = Vec::new();
        machine.down(tool_ctx(ctx(&elements), Tool::Callout), pt(50.0, 50.0));
        machine.drag_to(pt(100.0, 50.0));
        let outcome = machine.up(pt(100.0, 50.0), 9).expect("drag was active");
        // Phase two: no command yet, the field editor signal instead.
        assert_eq!(outcome.command, None);
        assert_eq!(
            outcome.signal,
            Some(Signal::BeginText(TextTarget::New {
                origin: pt(50.0, 50.0),
                callout: true,
                color: RED,
            }))
        );

        // The arrow previews (with empty text) while the input is open.
        let preview = machine.preview(&elements, Size::new(400.0, 300.0));
        assert_eq!(preview.extras.len(), 1);
        assert!(
            matches!(&preview.extras[0].kind, ElementKind::Callout { text, tip, .. }
                if text.is_empty() && *tip == pt(100.0, 50.0))
        );

        // The anchor query places the block beside the base, away from the tip.
        let anchor = machine
            .pending_callout_anchor(Size::new(40.0, 20.0), Size::new(400.0, 300.0))
            .expect("a pending callout");
        assert!(anchor.x < 50.0, "the block sits left of the base");

        // Typing feeds the draft; the commit freezes the very element the
        // preview showed (the no-jump invariant).
        machine.update_text("hi");
        let shown = machine
            .preview(&elements, Size::new(400.0, 300.0))
            .extras[0]
            .kind
            .clone();
        let command = machine
            .commit_text(&elements, Size::new(400.0, 300.0), 9)
            .expect("a real text");
        let el = assert_add(Some(command), 9);
        assert_eq!(el.kind, shown, "commit freezes the draft the canvas showed");
        match el.kind {
            ElementKind::Callout { tip, anchor, text, color, size, width } => {
                assert_eq!(tip, pt(100.0, 50.0));
                assert!(anchor.x < 50.0, "anchored beside the base, away from the tip");
                assert_eq!(text, "hi");
                assert_eq!(color, RED);
                assert_eq!(size, 32.0);
                assert_eq!(width, 4.0);
            }
            other => panic!("expected callout, got {other:?}"),
        }
        assert!(
            machine.preview(&elements, Size::new(400.0, 300.0)).extras.is_empty(),
            "the phase is over"
        );
        assert_eq!(
            machine.commit_text(&elements, Size::new(400.0, 300.0), 10),
            None,
            "a second commit has nothing to land"
        );
    }

    #[test]
    fn empty_text_drops_the_pending_callout() {
        let mut machine = DragMachine::new();
        let elements: Vec<Element> = Vec::new();
        machine.down(tool_ctx(ctx(&elements), Tool::Callout), pt(50.0, 50.0));
        machine.drag_to(pt(100.0, 50.0));
        machine.up(pt(100.0, 50.0), 9).expect("drag was active");
        // Whitespace-only input is as good as empty.
        machine.update_text("  ");
        assert_eq!(
            machine.commit_text(&elements, Size::new(400.0, 300.0), 9),
            None
        );
        assert!(
            machine
                .preview(&elements, Size::new(400.0, 300.0))
                .extras
                .is_empty(),
            "the arrow is gone"
        );
    }

    #[test]
    fn text_draft_equals_commit_across_strings_sizes_and_edges() {
        // The no-jump invariant: at every keystroke, whatever the canvas
        // shows is exactly what Enter lands — across the growing string,
        // the size and the click position, edge-clamped ones included.
        let image = Size::new(400.0, 300.0);
        for text in ["x", "hi", "hello brave new world", "многострочный\nтекст"] {
            for size in [12.0, 32.0, 64.0] {
                for origin in [pt(5.0, 5.0), pt(390.0, 290.0), pt(200.0, 150.0)] {
                    let mut typed = String::new();
                    for ch in text.chars() {
                        typed.push(ch);
                        let mut machine = DragMachine::new();
                        let elements: Vec<Element> = Vec::new();
                        let outcome =
                            machine.down(tool_ctx(ctx(&elements), Tool::Text), origin);
                        assert!(matches!(
                            outcome.signal,
                            Some(Signal::BeginText(TextTarget::New {
                                callout: false,
                                ..
                            }))
                        ));
                        machine.update_text_size(size);
                        machine.update_text(&typed);
                        let shown = machine
                            .preview(&elements, image)
                            .extras[0]
                            .kind
                            .clone();
                        let el = assert_add(
                            machine.commit_text(&elements, image, 7),
                            7,
                        );
                        assert_eq!(
                            el.kind, shown,
                            "commit moved the draft: {typed:?} at {size}/{origin:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn text_draft_slides_inward_as_it_grows_at_an_edge() {
        // A click near the right edge types a block that re-clamps into
        // the frame while it grows — no jump waits for the commit.
        let image = Size::new(400.0, 300.0);
        let mut machine = DragMachine::new();
        let elements: Vec<Element> = Vec::new();
        machine.down(tool_ctx(ctx(&elements), Tool::Text), pt(390.0, 150.0));
        machine.update_text("a");
        let short = match &machine.preview(&elements, image).extras[0].kind {
            ElementKind::Text { origin, .. } => *origin,
            other => panic!("expected a text draft, got {other:?}"),
        };
        machine.update_text("a much longer line of text");
        let long = match &machine.preview(&elements, image).extras[0].kind {
            ElementKind::Text { origin, .. } => *origin,
            other => panic!("expected a text draft, got {other:?}"),
        };
        assert!(long.x < short.x, "the block slides left as it grows");
        assert_eq!(long.y, short.y, "the vertical anchor holds");
    }

    #[test]
    fn callout_draft_equals_commit_while_typing() {
        let image = Size::new(400.0, 300.0);
        for text in ["x", "hello callout world", "две\nстроки"] {
            let mut machine = DragMachine::new();
            let elements: Vec<Element> = Vec::new();
            machine.down(tool_ctx(ctx(&elements), Tool::Callout), pt(50.0, 50.0));
            machine.drag_to(pt(100.0, 50.0));
            machine.up(pt(100.0, 50.0), 9).expect("drag was active");
            machine.update_text(text);
            let shown = machine.preview(&elements, image).extras[0].kind.clone();
            let el = assert_add(machine.commit_text(&elements, image, 9), 9);
            assert_eq!(el.kind, shown, "commit moved the callout: {text:?}");
        }
    }

    #[test]
    fn existing_text_edit_commits_through_the_draft() {
        let image = Size::new(400.0, 300.0);
        let elements = vec![text_element(4)];
        // A click on the element with the text tool opens the re-edit.
        let mut machine = DragMachine::new();
        let outcome = machine.down(tool_ctx(ctx(&elements), Tool::Text), pt(10.0, 90.0));
        assert_eq!(
            outcome.signal,
            Some(Signal::BeginText(TextTarget::Existing { id: 4 }))
        );
        // The draft replaces the element's text and re-clamps with it;
        // the commit is the plain EditText of what was on screen.
        machine.update_text("WWWW");
        let preview = machine.preview(&elements, image);
        assert_eq!(preview.hide, Some(4), "the element hides behind its draft");
        assert!(matches!(
            &preview.extras[0].kind,
            ElementKind::Text { text, .. } if text == "WWWW"
        ));
        assert_eq!(
            machine.commit_text(&elements, image, 5),
            Some(Command::EditText(4, "WWWW".into()))
        );
        // An unchanged re-edit journals nothing; an emptied one deletes.
        let mut machine = DragMachine::new();
        machine.down(tool_ctx(ctx(&elements), Tool::Text), pt(10.0, 90.0));
        machine.update_text("WW");
        assert_eq!(machine.commit_text(&elements, image, 5), None);
        let mut machine = DragMachine::new();
        machine.down(tool_ctx(ctx(&elements), Tool::Text), pt(10.0, 90.0));
        machine.update_text(" ");
        assert_eq!(
            machine.commit_text(&elements, image, 5),
            Some(Command::DeleteElement(4))
        );
    }

    #[test]
    fn frame_edit_survives_a_frame_drag_settle_only() {
        // A drag of a NON-frame kind must not disturb the frame rect.
        let image = Size::new(200.0, 100.0);
        let mut machine = DragMachine::new();
        machine.begin_frame(false, image);
        let elements = vec![rect_element(3)];
        let ctx = DownCtx {
            tool: Some(Tool::Crop),
            frame: machine.current_frame(),
            ..ctx(&elements)
        };
        machine.down(ctx, pt(200.0, 100.0));
        machine.up(pt(195.0, 95.0), 1).expect("drag was active");
        assert_eq!(
            machine.current_frame(),
            Some(Rect::new(0.0, 0.0, 195.0, 95.0)),
            "the frame settles at the up point without a dragged event"
        );
    }

    #[test]
    fn callout_anchor_moves_where_dragged() {
        // Regression guard for the anchor math through the machine: an
        // up-pointing arrow puts the block below the base.
        let anchor = callout_anchor(
            pt(100.0, 100.0),
            pt(100.0, 40.0),
            60.0,
            30.0,
            Rect::new(0.0, 0.0, 400.0, 300.0),
        );
        assert_eq!(anchor, Point::new(70.0, 116.0));
    }

    #[test]
    fn set_pending_callout_size_reanchors_and_keeps_the_arrow() {
        let image = Size::new(400.0, 300.0);
        let mut machine = DragMachine::new();
        let elements: Vec<Element> = Vec::new();
        // A left-pointing arrow: the block sits right of the base.
        machine.down(tool_ctx(ctx(&elements), Tool::Callout), pt(150.0, 50.0));
        machine.drag_to(pt(100.0, 50.0));
        machine.up(pt(100.0, 50.0), 9).expect("drag was active");

        let small = machine
            .set_pending_callout_size(32.0, Size::new(40.0, 20.0), image)
            .expect("a pending callout");
        let grown = machine
            .set_pending_callout_size(64.0, Size::new(80.0, 40.0), image)
            .expect("a pending callout");
        // The block grows away from the tip: the near edge holds its gap
        // from the base, the taller block re-centers on it.
        assert_eq!(grown.x, small.x, "the near edge holds the gap");
        assert!(grown.y < small.y, "the taller block re-centers on the base");
        assert!(grown.x > 150.0, "the block sits on the side away from the tip");
        // The position is exactly callout_anchor's for the new block.
        assert_eq!(
            grown,
            callout_anchor(
                pt(150.0, 50.0),
                pt(100.0, 50.0),
                80.0,
                40.0,
                Rect::new(0.0, 0.0, 400.0, 300.0)
            )
        );
        // The arrow is untouched: tip and stroke width stay as drawn, the
        // draft size took the new value.
        let pending = machine.callout.as_ref().expect("pending");
        assert_eq!(pending.tip, pt(100.0, 50.0));
        assert_eq!(pending.width, 4.0);
        assert_eq!(pending.size, 64.0);

        // No pending callout: nothing to resize.
        let mut fresh = DragMachine::new();
        assert_eq!(
            fresh.set_pending_callout_size(32.0, Size::new(40.0, 20.0), image),
            None
        );
    }

    #[test]
    fn clamp_block_keeps_a_fitting_block_in_place() {
        let frame = Rect::new(0.0, 0.0, 400.0, 300.0);
        // Fully inside with room to spare: a pass-through.
        assert_eq!(clamp_block(pt(100.0, 100.0), 60.0, 30.0, frame), pt(100.0, 100.0));
        // Exactly on the margin lines: still a no-op.
        assert_eq!(clamp_block(pt(336.0, 266.0), 60.0, 30.0, frame), pt(336.0, 266.0));
    }

    #[test]
    fn clamp_block_presses_each_edge_with_the_margin() {
        let frame = Rect::new(0.0, 0.0, 400.0, 300.0);
        // Overshooting right and bottom: pressed to max - block - margin.
        assert_eq!(clamp_block(pt(380.0, 280.0), 60.0, 30.0, frame), pt(336.0, 266.0));
        // Negative origin: pressed to the margin itself.
        assert_eq!(clamp_block(pt(-10.0, -5.0), 60.0, 30.0, frame), pt(4.0, 4.0));
        // A frame away from the origin clamps against its own edges.
        let shifted = Rect::new(50.0, 50.0, 100.0, 100.0);
        assert_eq!(clamp_block(pt(0.0, 0.0), 20.0, 10.0, shifted), pt(54.0, 54.0));
        assert_eq!(clamp_block(pt(200.0, 200.0), 20.0, 10.0, shifted), pt(126.0, 136.0));
    }

    #[test]
    fn clamp_block_parks_an_oversized_block_at_the_margin() {
        let frame = Rect::new(0.0, 0.0, 50.0, 50.0);
        // Wider than the frame: that axis parks at the margin.
        assert_eq!(clamp_block(pt(10.0, 10.0), 80.0, 10.0, frame), pt(4.0, 10.0));
        // Taller than the frame on one axis only: the other still clamps.
        assert_eq!(clamp_block(pt(10.0, 10.0), 10.0, 90.0, frame), pt(10.0, 4.0));
    }

    #[test]
    fn clamp_block_does_not_bounce_after_a_shrink() {
        let frame = Rect::new(0.0, 0.0, 400.0, 300.0);
        // Grown past the right edge and clamped; shrinking the size keeps
        // the shifted position.
        let grown = clamp_block(pt(380.0, 100.0), 200.0, 40.0, frame);
        assert_eq!(grown, pt(196.0, 100.0));
        let shrunk = clamp_block(grown, 60.0, 40.0, frame);
        assert_eq!(shrunk, pt(196.0, 100.0), "a smaller block stays put");
    }
}

