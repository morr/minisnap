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

/// Font size of new text and callout elements, in image pixels.
pub(crate) const DEFAULT_TEXT_SIZE_PX: f64 = 32.0;

/// What the open field editor is editing; also the payload of
/// `Signal::BeginText`.
#[derive(Clone, Copy)]
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
pub(crate) enum Select {
    /// The click was consumed without touching the selection.
    Keep,
    /// Select this element.
    Element(u64),
    /// Deselect.
    Clear,
}

/// An AppKit follow-up the window owes the interaction.
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
                    return DownOutcome {
                        selection: Select::Keep,
                        signal: Some(Signal::BeginText(TextTarget::Existing { id })),
                    };
                }
                if ctx.tool == Some(Tool::Text) && !id_is_textlike(ctx.elements, id) {
                    // The text tool starts a new text over any other
                    // element.
                    return DownOutcome {
                        selection: Select::Keep,
                        signal: Some(Signal::BeginText(TextTarget::New {
                            origin: point,
                            callout: false,
                            color: ctx.color,
                        })),
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
                        return DownOutcome {
                            selection: Select::Clear,
                            signal: Some(Signal::BeginText(TextTarget::New {
                                origin: point,
                                callout: false,
                                color: ctx.color,
                            })),
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
                    outcome.signal = Some(Signal::BeginText(TextTarget::New {
                        origin: anchor,
                        callout: true,
                        color,
                    }));
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
    pub(crate) fn preview(&self, elements: &[Element]) -> Preview {
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
        // The pending callout (arrow drawn, text being entered) previews on
        // top until the text commits or the input is cancelled.
        if let Some(pending) = self.callout.as_ref() {
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

    /// Commit the callout's text — the end of the second phase: a
    /// non-empty text lands as the element's final form, anchored beside
    /// the arrow base; an empty input drops the pending arrow and adds
    /// nothing. None when there is no pending callout or the input was
    /// empty.
    pub(crate) fn commit_callout_text(
        &mut self,
        text: &str,
        image: Size,
        next_id: u64,
    ) -> Option<Command> {
        let pending = self.callout.take()?;
        if text.trim().is_empty() {
            return None;
        }
        let (w, h) = text::measure_block(text, pending.size)
            .map(|m| (m.width, m.height))
            .unwrap_or((pending.size * 4.0, pending.size * 1.25));
        Some(Command::AddElement(Element {
            id: next_id,
            kind: ElementKind::Callout {
                tip: pending.tip,
                anchor: callout_anchor(
                    pending.base,
                    pending.tip,
                    w,
                    h,
                    Rect::new(0.0, 0.0, image.width, image.height),
                ),
                text: text.to_string(),
                color: pending.color,
                size: pending.size,
                width: pending.width,
            },
        }))
    }

    /// Drop the pending callout without committing (cancelled input, a new
    /// shot arrived).
    pub(crate) fn cancel_callout_text(&mut self) {
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
