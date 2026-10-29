//! Editor document: vector annotation elements over a captured frame and a
//! journal of deterministic commands.
//!
//! The working state is `{ base frame, elements }`; every edit is a
//! `Command`. Undo/redo never apply inverse operations — the state is
//! rebuilt by replaying the applied command prefix against the pristine
//! base frame, so it can never drift from the journal.

use objc2::rc::Retained;
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreateImage, CGImage, CGContext};
use serde::{Deserialize, Serialize};

pub mod hit_test;
pub mod render;
pub mod session;
pub mod text;

pub use session::OpenShot;

/// RGBA color with components in 0..1 (straight alpha; strokes are opaque).
pub type Rgba = [f64; 4];

/// Point in base-frame pixel space (top-left origin, Y down) — the same
/// space the captured images live in. A serde-friendly stand-in for
/// `CGPoint`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Point {
        Point { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub fn new(width: f64, height: f64) -> Size {
        Size { width, height }
    }
}

/// Rect in base-frame pixel space (top-left origin). A serde-friendly
/// stand-in for `CGRect`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect { x, y, width, height }
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// The rect with negative sizes flipped, if it is non-empty.
    pub fn standardize(&self) -> Rect {
        let (x, width) = if self.width >= 0.0 {
            (self.x, self.width)
        } else {
            (self.x + self.width, -self.width)
        };
        let (y, height) = if self.height >= 0.0 {
            (self.y, self.height)
        } else {
            (self.y + self.height, -self.height)
        };
        Rect { x, y, width, height }
    }

    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }

    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.max_x() && p.y >= self.y && p.y < self.max_y()
    }
}

/// An annotation element with a stable id (monotonic u64, not an index, so
/// the journal stays valid across deletions).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub id: u64,
    pub kind: ElementKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ElementKind {
    /// Wedge arrow from `a` (thin tail) to `b` (head).
    Arrow {
        a: Point,
        b: Point,
        color: Rgba,
        width: f64,
    },
    Rect {
        rect: Rect,
        color: Rgba,
        width: f64,
    },
    Line {
        a: Point,
        b: Point,
        color: Rgba,
        width: f64,
    },
    /// Free-hand trace.
    Pen {
        points: Vec<Point>,
        color: Rgba,
        width: f64,
    },
    /// Text with `origin` at the top-left corner of the text block.
    Text {
        origin: Point,
        text: String,
        color: Rgba,
        size: f64,
    },
    /// Text with a pointer: `tip` points at the target, `anchor` is the
    /// top-left corner of the text block.
    Callout {
        tip: Point,
        anchor: Point,
        text: String,
        color: Rgba,
        size: f64,
    },
    /// Pixelated region of the base frame.
    Blur { rect: Rect },
}

/// Which end of an arrow/line/callout a `MovePoint` targets. For a callout,
/// A is the tip and B is the anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Endpoint {
    A,
    B,
}

/// One deterministic edit. `Crop`/`Resize` mutate the base frame and
/// transform the coordinates of every existing element.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    AddElement(Element),
    DeleteElement(u64),
    /// Shift an element by a delta in base pixel space.
    MoveElement(u64, Point),
    /// Move one end of an arrow/line/callout.
    MovePoint(u64, Endpoint, Point),
    /// Replace the text of a text/callout element.
    EditText(u64, String),
    /// Cut the base frame to the rect (in the current base pixel space).
    Crop(Rect),
    /// Resample the base frame to the size.
    Resize(Size),
}

/// The replayable working state.
struct DocState {
    base: Retained<CGImage>,
    elements: Vec<Element>,
}

/// Apply one command to a state in place. Deterministic — replaying the
/// same prefix always produces the same state.
fn apply_cmd(state: &mut DocState, cmd: &Command) {
    match cmd {
        Command::AddElement(element) => state.elements.push(element.clone()),
        Command::DeleteElement(id) => state.elements.retain(|el| el.id != *id),
        Command::MoveElement(id, delta) => {
            for el in &mut state.elements {
                if el.id == *id {
                    shift_element(&mut el.kind, delta.x, delta.y);
                }
            }
        }
        Command::MovePoint(id, end, point) => {
            for el in &mut state.elements {
                if el.id == *id {
                    set_endpoint(&mut el.kind, *end, *point);
                }
            }
        }
        Command::EditText(id, text) => {
            for el in &mut state.elements {
                if el.id == *id {
                    match &mut el.kind {
                        ElementKind::Text { text: t, .. } => *t = text.clone(),
                        ElementKind::Callout { text: t, .. } => *t = text.clone(),
                        _ => {}
                    }
                }
            }
        }
        Command::Crop(rect) => {
            let (w, h) = image_size(&state.base);
            let bounds = Rect::new(0.0, 0.0, w, h);
            let rect = rect.standardize();
            let x0 = rect.x.max(bounds.x);
            let y0 = rect.y.max(bounds.y);
            let x1 = rect.max_x().min(bounds.max_x());
            let y1 = rect.max_y().min(bounds.max_y());
            let clipped = Rect::new(x0, y0, x1 - x0, y1 - y0);
            if clipped.is_empty() {
                return;
            }
            let cg = CGRect::new(
                CGPoint::new(clipped.x, clipped.y),
                CGSize::new(clipped.width, clipped.height),
            );
            let Some(cropped) = crate::output::crop(&state.base, cg) else {
                return;
            };
            state.base = cropped.into();
            for el in &mut state.elements {
                shift_element(&mut el.kind, -clipped.x, -clipped.y);
            }
        }
        Command::Resize(size) => {
            if size.width <= 0.0 || size.height <= 0.0 {
                return;
            }
            let (w, h) = image_size(&state.base);
            if w == 0.0 || h == 0.0 {
                return;
            }
            let Some(resized) = redraw_at_size(
                &state.base,
                size.width.round() as usize,
                size.height.round() as usize,
            ) else {
                return;
            };
            state.base = resized.into();
            let (sx, sy) = (size.width / w, size.height / h);
            for el in &mut state.elements {
                scale_element(&mut el.kind, sx, sy);
            }
        }
    }
}

/// Image size in pixels (the space all element coordinates live in).
fn image_size(image: &CGImage) -> (f64, f64) {
    (
        CGImage::width(Some(image)) as f64,
        CGImage::height(Some(image)) as f64,
    )
}

/// Draw `image` into a fresh bitmap context of the given pixel size and
/// return the resulting image. A plain draw-then-snapshot keeps the source
/// orientation (image → context → image round trip does not flip).
fn redraw_at_size(image: &CGImage, width: usize, height: usize) -> Option<CFRetained<CGImage>> {
    let ctx = render::bitmap_context(width, height)?;
    CGContext::draw_image(
        Some(&ctx),
        CGRect::new(CGPoint::ZERO, CGSize::new(width as f64, height as f64)),
        Some(image),
    );
    CGBitmapContextCreateImage(Some(&ctx))
}

/// Shift every coordinate of an element by a delta.
fn shift_element(kind: &mut ElementKind, dx: f64, dy: f64) {
    let shift = |p: &mut Point| {
        p.x += dx;
        p.y += dy;
    };
    match kind {
        ElementKind::Arrow { a, b, .. } | ElementKind::Line { a, b, .. } => {
            shift(a);
            shift(b);
        }
        ElementKind::Rect { rect, .. } | ElementKind::Blur { rect } => {
            rect.x += dx;
            rect.y += dy;
        }
        ElementKind::Pen { points, .. } => {
            for p in points {
                shift(p);
            }
        }
        ElementKind::Text { origin, .. } => shift(origin),
        ElementKind::Callout { tip, anchor, .. } => {
            shift(tip);
            shift(anchor);
        }
    }
}

/// Scale every coordinate of an element by per-axis factors (the base frame
/// was resized). Stroke widths and font sizes stay as authored.
fn scale_element(kind: &mut ElementKind, sx: f64, sy: f64) {
    let scale = |p: &mut Point| {
        p.x *= sx;
        p.y *= sy;
    };
    match kind {
        ElementKind::Arrow { a, b, .. } | ElementKind::Line { a, b, .. } => {
            scale(a);
            scale(b);
        }
        ElementKind::Rect { rect, .. } | ElementKind::Blur { rect } => {
            rect.x *= sx;
            rect.y *= sy;
            rect.width *= sx;
            rect.height *= sy;
        }
        ElementKind::Pen { points, .. } => {
            for p in points {
                scale(p);
            }
        }
        ElementKind::Text { origin, .. } => scale(origin),
        ElementKind::Callout { tip, anchor, .. } => {
            scale(tip);
            scale(anchor);
        }
    }
}

/// Move one end of an arrow/line/callout. Other kinds have no ends.
fn set_endpoint(kind: &mut ElementKind, end: Endpoint, point: Point) {
    match kind {
        ElementKind::Arrow { a, b, .. } | ElementKind::Line { a, b, .. } => match end {
            Endpoint::A => *a = point,
            Endpoint::B => *b = point,
        },
        ElementKind::Callout { tip, anchor, .. } => match end {
            Endpoint::A => *tip = point,
            Endpoint::B => *anchor = point,
        },
        _ => {}
    }
}

/// The editor document: pristine base frame + replayed state + journal.
pub struct EditorDoc {
    /// The frame every replay starts from (the shot as captured, or the
    /// file as opened without a session).
    original: Retained<CGImage>,
    state: DocState,
    /// The full journal; `log[applied..]` is the redo tail.
    log: Vec<Command>,
    /// Number of leading journal commands applied to `state`.
    applied: usize,
}

impl EditorDoc {
    pub fn new(original: Retained<CGImage>) -> EditorDoc {
        let state = DocState {
            base: original.clone(),
            elements: Vec::new(),
        };
        EditorDoc {
            original,
            state,
            log: Vec::new(),
            applied: 0,
        }
    }

    /// The current base frame (after any crop/resize commands).
    pub fn base(&self) -> &Retained<CGImage> {
        &self.state.base
    }

    /// The pristine frame every replay starts from (persisted as the
    /// session's `original.png`).
    pub fn original(&self) -> &Retained<CGImage> {
        &self.original
    }

    /// The current elements, bottom-most first (paint order).
    pub fn elements(&self) -> &[Element] {
        &self.state.elements
    }

    pub fn element(&self, id: u64) -> Option<&Element> {
        self.state.elements.iter().find(|el| el.id == id)
    }

    /// The applied command prefix — what undo can retract and what a
    /// session persists.
    pub fn applied_log(&self) -> &[Command] {
        &self.log[..self.applied]
    }

    pub fn can_undo(&self) -> bool {
        self.applied > 0
    }

    pub fn can_redo(&self) -> bool {
        self.applied < self.log.len()
    }

    /// Apply a command as the new last step; the redo tail is discarded.
    pub fn apply(&mut self, cmd: Command) {
        self.log.truncate(self.applied);
        self.log.push(cmd.clone());
        self.applied += 1;
        apply_cmd(&mut self.state, &cmd);
    }

    /// Revert the last applied command. False when nothing to undo.
    pub fn undo(&mut self) -> bool {
        if !self.can_undo() {
            return false;
        }
        self.applied -= 1;
        self.rebuild();
        true
    }

    /// Re-apply the next retracted command. False when nothing to redo.
    pub fn redo(&mut self) -> bool {
        if !self.can_redo() {
            return false;
        }
        self.applied += 1;
        self.rebuild();
        true
    }

    /// Rebuild the state by replaying the applied prefix against the
    /// pristine base.
    fn rebuild(&mut self) {
        let mut state = DocState {
            base: self.original.clone(),
            elements: Vec::new(),
        };
        for cmd in &self.log[..self.applied] {
            apply_cmd(&mut state, cmd);
        }
        self.state = state;
    }

    /// The next free element id: one past every id that appears anywhere in
    /// the journal (the redo tail included — its commands may replay again).
    pub fn next_element_id(&self) -> u64 {
        let mut max = 0u64;
        for cmd in &self.log {
            if let Command::AddElement(el) = cmd {
                max = max.max(el.id);
            }
        }
        max + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::CFRetained;
    use objc2_core_foundation::CFData;
    use objc2_core_graphics::{CGDataProvider, CGColorSpace, CGImageAlphaInfo, CGContext};
    use objc2_core_graphics::CGBitmapContextCreateImage;

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

    /// A solid gray frame of the given size for document tests.
    pub(crate) fn solid_frame(w: usize, h: usize) -> Retained<CGImage> {
        let buf = vec![128u8; w * h * 4];
        let space = CGColorSpace::new_device_rgb().expect("rgb colorspace");
        // SAFETY: buf outlives the context within this function.
        let ctx_ptr = unsafe {
            CGBitmapContextCreate(
                buf.as_ptr() as *mut std::ffi::c_void,
                w,
                h,
                8,
                w * 4,
                &space,
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        };
        assert!(!ctx_ptr.is_null(), "bitmap context");
        // SAFETY: non-null context from the constructor above.
        let ctx = unsafe { CFRetained::from_raw(std::ptr::NonNull::new_unchecked(ctx_ptr)) };
        CGBitmapContextCreateImage(Some(&ctx))
            .expect("image from context")
            .into()
    }

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn arrow(id: u64, ax: f64, ay: f64, bx: f64, by: f64) -> Element {
        Element {
            id,
            kind: ElementKind::Arrow {
                a: pt(ax, ay),
                b: pt(bx, by),
                color: [1.0, 0.0, 1.0, 1.0],
                width: 3.0,
            },
        }
    }

    #[test]
    fn add_move_delete_replays_from_journal() {
        let mut doc = EditorDoc::new(solid_frame(64, 64));
        doc.apply(Command::AddElement(arrow(1, 10.0, 10.0, 40.0, 40.0)));
        doc.apply(Command::MoveElement(1, pt(5.0, -5.0)));
        doc.apply(Command::AddElement(Element {
            id: 2,
            kind: ElementKind::Blur {
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            },
        }));
        doc.apply(Command::DeleteElement(1));

        assert_eq!(doc.elements().len(), 1);
        assert_eq!(doc.elements()[0].id, 2);

        // Replaying the same journal against a fresh document reproduces
        // the state exactly.
        let mut replayed = EditorDoc::new(solid_frame(64, 64));
        for cmd in doc.applied_log() {
            replayed.apply(cmd.clone());
        }
        assert_eq!(replayed.elements(), doc.elements());
        assert_eq!(replayed.applied_log(), doc.applied_log());
    }

    #[test]
    fn undo_redo_restore_exact_previous_states() {
        let mut doc = EditorDoc::new(solid_frame(64, 64));
        let e1 = arrow(1, 10.0, 10.0, 40.0, 40.0);
        doc.apply(Command::AddElement(e1.clone()));
        let after_add: Vec<Element> = vec![e1];
        doc.apply(Command::MoveElement(1, pt(3.0, 4.0)));
        let after_move = doc.elements().to_vec();

        assert!(doc.undo());
        assert_eq!(doc.elements(), after_add.as_slice());
        assert!(doc.redo());
        assert_eq!(doc.elements(), after_move.as_slice());

        // Two undos land on the empty state; a third is a no-op.
        assert!(doc.undo());
        assert!(doc.undo());
        assert!(doc.elements().is_empty());
        assert!(!doc.undo());
        // Redo walks the journal forward again.
        assert!(doc.redo());
        assert_eq!(doc.elements(), after_add.as_slice());
        assert!(doc.redo());
        assert_eq!(doc.elements(), after_move.as_slice());
        assert!(!doc.redo());
    }

    #[test]
    fn apply_after_undo_discards_the_redo_tail() {
        let mut doc = EditorDoc::new(solid_frame(64, 64));
        doc.apply(Command::AddElement(arrow(1, 10.0, 10.0, 40.0, 40.0)));
        doc.apply(Command::MoveElement(1, pt(3.0, 4.0)));
        doc.apply(Command::DeleteElement(1));

        assert!(doc.undo()); // back to "moved"
        assert!(doc.undo()); // back to "added"
        assert_eq!(doc.applied_log().len(), 1);

        doc.apply(Command::MoveElement(1, pt(7.0, 7.0)));
        assert!(!doc.can_redo(), "the retracted tail must be dropped");
        assert_eq!(doc.applied_log().len(), 2);
        let el = doc.element(1).expect("element alive");
        assert_eq!(
            el.kind,
            ElementKind::Arrow {
                a: pt(17.0, 17.0),
                b: pt(47.0, 47.0),
                color: [1.0, 0.0, 1.0, 1.0],
                width: 3.0,
            }
        );
    }

    #[test]
    fn move_point_edits_one_end_only() {
        let mut doc = EditorDoc::new(solid_frame(64, 64));
        doc.apply(Command::AddElement(arrow(1, 10.0, 10.0, 40.0, 40.0)));
        doc.apply(Command::MovePoint(1, Endpoint::B, pt(50.0, 60.0)));
        match &doc.element(1).expect("arrow").kind {
            ElementKind::Arrow { a, b, .. } => {
                assert_eq!((a.x, a.y), (10.0, 10.0), "tail stays put");
                assert_eq!((b.x, b.y), (50.0, 60.0), "head moved");
            }
            other => panic!("expected arrow, got {other:?}"),
        }

        // Callout: A is the tip, B is the anchor.
        doc.apply(Command::AddElement(Element {
            id: 2,
            kind: ElementKind::Callout {
                tip: pt(5.0, 5.0),
                anchor: pt(20.0, 30.0),
                text: "hi".into(),
                color: [1.0, 0.0, 1.0, 1.0],
                size: 14.0,
            },
        }));
        doc.apply(Command::MovePoint(2, Endpoint::A, pt(8.0, 9.0)));
        match &doc.element(2).expect("callout").kind {
            ElementKind::Callout { tip, anchor, .. } => {
                assert_eq!((tip.x, tip.y), (8.0, 9.0));
                assert_eq!((anchor.x, anchor.y), (20.0, 30.0));
            }
            other => panic!("expected callout, got {other:?}"),
        }
    }

    #[test]
    fn edit_text_and_next_element_id() {
        let mut doc = EditorDoc::new(solid_frame(64, 64));
        assert_eq!(doc.next_element_id(), 1);
        doc.apply(Command::AddElement(Element {
            id: 1,
            kind: ElementKind::Text {
                origin: pt(4.0, 4.0),
                text: "old".into(),
                color: [1.0, 0.0, 1.0, 1.0],
                size: 14.0,
            },
        }));
        doc.apply(Command::EditText(1, "привет".into()));
        match &doc.element(1).expect("text").kind {
            ElementKind::Text { text, .. } => assert_eq!(text, "привет"),
            other => panic!("expected text, got {other:?}"),
        }
        assert_eq!(doc.next_element_id(), 2);

        // The redo tail counts toward used ids: undo leaves the Add in the
        // journal, so the id is still taken.
        doc.apply(Command::AddElement(Element {
            id: 2,
            kind: ElementKind::Blur {
                rect: Rect::new(0.0, 0.0, 5.0, 5.0),
            },
        }));
        assert!(doc.undo());
        assert_eq!(doc.next_element_id(), 3);
    }

    /// Red above the middle, blue below (top-left origin) — an asymmetric
    /// frame for orientation assertions.
    pub(crate) fn split_frame(w: usize, h: usize) -> Retained<CGImage> {
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let (r, g, b) = if y < h / 2 { (255, 0, 0) } else { (0, 0, 255) };
                buf[i] = r;
                buf[i + 1] = g;
                buf[i + 2] = b;
                buf[i + 3] = 255;
            }
        }
        let space = CGColorSpace::new_device_rgb().expect("rgb colorspace");
        // SAFETY: buf outlives the context within this function.
        let ctx_ptr = unsafe {
            CGBitmapContextCreate(
                buf.as_ptr() as *mut std::ffi::c_void,
                w,
                h,
                8,
                w * 4,
                &space,
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        };
        assert!(!ctx_ptr.is_null(), "bitmap context");
        // SAFETY: non-null context from the constructor above.
        let ctx = unsafe { CFRetained::from_raw(std::ptr::NonNull::new_unchecked(ctx_ptr)) };
        CGBitmapContextCreateImage(Some(&ctx))
            .expect("image from context")
            .into()
    }

    /// A deterministic high-frequency noise frame: no flat regions, so any
    /// meaningful cell of a pixelation differs from any other.
    pub(crate) fn noisy_frame(w: usize, h: usize) -> Retained<CGImage> {
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let v = (((x * 13 + y * 7) % 11) * 23) as u8;
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
                buf.as_ptr() as *mut std::ffi::c_void,
                w,
                h,
                8,
                w * 4,
                &space,
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        };
        assert!(!ctx_ptr.is_null(), "bitmap context");
        // SAFETY: non-null context from the constructor above.
        let ctx = unsafe { CFRetained::from_raw(std::ptr::NonNull::new_unchecked(ctx_ptr)) };
        CGBitmapContextCreateImage(Some(&ctx))
            .expect("image from context")
            .into()
    }

    /// The image's raw RGBA bytes, top row first (the CGImage's own data).
    pub(crate) fn pixel_bytes(image: &CGImage) -> Vec<u8> {
        let provider = CGImage::data_provider(Some(image)).expect("data provider");
        let data: CFRetained<CFData> = CGDataProvider::data(Some(&provider)).expect("data");
        data.to_vec()
    }

    fn assert_blue(px: [u8; 3]) {
        assert_eq!(px, [0, 0, 255], "expected blue pixel");
    }

    fn assert_red(px: [u8; 3]) {
        assert_eq!(px, [255, 0, 0], "expected red pixel");
    }

    fn top_pixel(bytes: &[u8]) -> [u8; 3] {
        [bytes[0], bytes[1], bytes[2]]
    }

    fn bottom_pixel(bytes: &[u8], row_width_bytes: usize) -> [u8; 3] {
        let start = bytes.len() - row_width_bytes;
        [bytes[start], bytes[start + 1], bytes[start + 2]]
    }

    #[test]
    fn crop_shifts_elements_and_cuts_the_base() {
        let mut doc = EditorDoc::new(split_frame(64, 64));
        doc.apply(Command::AddElement(arrow(1, 10.0, 40.0, 20.0, 50.0)));
        // The bottom half in top-left origin pixel space.
        doc.apply(Command::Crop(Rect::new(0.0, 32.0, 64.0, 32.0)));

        assert_eq!(image_size(doc.base()), (64.0, 32.0));
        match &doc.element(1).expect("arrow").kind {
            ElementKind::Arrow { a, b, .. } => {
                assert_eq!((a.x, a.y), (10.0, 8.0), "shifted by -origin");
                assert_eq!((b.x, b.y), (20.0, 18.0));
            }
            other => panic!("expected arrow, got {other:?}"),
        }
        // The kept region is the original's bottom half: blue on top.
        let bytes = pixel_bytes(doc.base());
        assert_blue(top_pixel(&bytes));

        // Undo restores the full frame and the original coordinates.
        assert!(doc.undo());
        assert_eq!(image_size(doc.base()), (64.0, 64.0));
        match &doc.element(1).expect("arrow").kind {
            ElementKind::Arrow { a, b, .. } => {
                assert_eq!((a.x, a.y), (10.0, 40.0));
                assert_eq!((b.x, b.y), (20.0, 50.0));
            }
            other => panic!("expected arrow, got {other:?}"),
        }
    }

    #[test]
    fn resize_scales_elements_and_the_base() {
        let mut doc = EditorDoc::new(split_frame(64, 64));
        doc.apply(Command::AddElement(arrow(1, 10.0, 40.0, 20.0, 50.0)));
        doc.apply(Command::Resize(Size::new(32.0, 16.0)));

        assert_eq!(image_size(doc.base()), (32.0, 16.0));
        match &doc.element(1).expect("arrow").kind {
            ElementKind::Arrow { a, b, .. } => {
                assert_eq!((a.x, a.y), (5.0, 10.0), "coordinates halved");
                assert_eq!((b.x, b.y), (10.0, 12.5));
            }
            other => panic!("expected arrow, got {other:?}"),
        }
        // Orientation survives the resample: red on top, blue at the bottom.
        let bytes = pixel_bytes(doc.base());
        assert_red(top_pixel(&bytes));
        assert_blue(bottom_pixel(&bytes, 32 * 4));

        // Undo restores the size and the coordinates.
        assert!(doc.undo());
        assert_eq!(image_size(doc.base()), (64.0, 64.0));
        match &doc.element(1).expect("arrow").kind {
            ElementKind::Arrow { a, b, .. } => {
                assert_eq!((a.x, a.y), (10.0, 40.0));
                assert_eq!((b.x, b.y), (20.0, 50.0));
            }
            other => panic!("expected arrow, got {other:?}"),
        }
    }

    #[test]
    fn off_bounds_crop_and_zero_resize_change_nothing() {
        let mut doc = EditorDoc::new(split_frame(64, 64));
        doc.apply(Command::AddElement(arrow(1, 10.0, 40.0, 20.0, 50.0)));

        doc.apply(Command::Crop(Rect::new(100.0, 100.0, 10.0, 10.0)));
        doc.apply(Command::Resize(Size::new(0.0, 10.0)));
        assert_eq!(image_size(doc.base()), (64.0, 64.0));
        match &doc.element(1).expect("arrow").kind {
            ElementKind::Arrow { a, b, .. } => {
                assert_eq!((a.x, a.y), (10.0, 40.0));
                assert_eq!((b.x, b.y), (20.0, 50.0));
            }
            other => panic!("expected arrow, got {other:?}"),
        }
    }
}
