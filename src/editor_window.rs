//! The editor window: one reusable `NSWindow` showing the open shot — the
//! base frame plus annotations, drawn with CoreGraphics into a custom
//! `NSView`, with an annotation toolbar above the canvas. Selection markers
//! are drawn on top in the canvas and never become part of a saved
//! composite.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    define_class, extern_methods, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSColor, NSControlTextEditingDelegate,
    NSEvent, NSFocusRingType, NSFont, NSGraphicsContext, NSImage, NSImageSymbolConfiguration,
    NSPopover, NSScreen, NSStackView, NSTextField, NSTextFieldDelegate,
    NSUserInterfaceLayoutOrientation, NSView, NSViewController, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGImage, CGContext};
use objc2_foundation::{NSString, NSNotification, NSRectEdge};

use crate::editor::hit_test::{Hit, top_hit};
use crate::editor::text;
use crate::editor::{render, Command, Element, ElementKind, OpenShot, Point, Rect, Rgba};

/// The dark backdrop the frame sits on.
const BACKDROP: [f64; 4] = [0.12, 0.12, 0.14, 1.0];

/// Selection marker radius, in view points (constant on screen).
const MARKER_RADIUS_PT: f64 = 4.5;

/// Toolbar strip height, in points.
const TOOLBAR_HEIGHT: f64 = 40.0;

/// Initial canvas size before the first shot arrives.
const INITIAL_SIZE: CGSize = CGSize::new(480.0, 320.0);

/// Default stroke width for new elements, in image pixels.
const DEFAULT_STROKE_WIDTH: f64 = 4.0;

/// A drag shorter than this (in image pixels) is a plain click, not a
/// drawn element.
const MIN_DRAG_PX: f64 = 4.0;

/// Font size of new text and callout elements, in image pixels.
const DEFAULT_TEXT_SIZE_PX: f64 = 32.0;

/// Offset of a new callout's text block from its tip, in image pixels.
const CALLOUT_TEXT_OFFSET_PX: f64 = 16.0;

/// Width of the field editor for new (empty) text, in view points.
const TEXT_EDITOR_WIDTH_PT: f64 = 200.0;

/// What the open field editor is editing.
#[derive(Clone, Copy)]
enum TextTarget {
    /// A brand-new text or callout element.
    New { origin: Point, callout: bool },
    /// An existing text/callout element being re-edited.
    Existing { id: u64 },
}

/// Color presets offered by the popover; magenta is the default.
const COLOR_PRESETS: [Rgba; 8] = [
    [1.0, 0.0, 1.0, 1.0],  // magenta
    [1.0, 0.0, 0.0, 1.0],  // red
    [1.0, 0.5, 0.0, 1.0],  // orange
    [1.0, 0.85, 0.0, 1.0], // yellow
    [0.0, 0.75, 0.0, 1.0], // green
    [0.0, 0.7, 0.9, 1.0],  // cyan
    [0.1, 0.3, 1.0, 1.0],  // blue
    [0.0, 0.0, 0.0, 1.0],  // black
];

/// The annotation tools, in toolbar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Arrow,
    Rect,
    Line,
    Pen,
    Text,
    Callout,
    Blur,
    Crop,
    Resize,
}

impl Tool {
    fn all() -> [Tool; 9] {
        [
            Tool::Arrow,
            Tool::Rect,
            Tool::Line,
            Tool::Pen,
            Tool::Text,
            Tool::Callout,
            Tool::Blur,
            Tool::Crop,
            Tool::Resize,
        ]
    }

    /// SF Symbol name for the toolbar button.
    fn symbol(self) -> &'static str {
        match self {
            Tool::Arrow => "arrow.up.right",
            Tool::Rect => "rectangle",
            Tool::Line => "line.diagonal",
            Tool::Pen => "scribble",
            Tool::Text => "textformat",
            Tool::Callout => "ellipsis",
            Tool::Blur => "drop",
            Tool::Crop => "crop",
            Tool::Resize => "arrow.up.left.and.arrow.down.right",
        }
    }

    fn tag(self) -> isize {
        self as isize
    }

    fn from_tag(tag: isize) -> Option<Tool> {
        Tool::all().into_iter().find(|tool| tool.tag() == tag)
    }
}

pub struct EditorWindow {
    pub window: Retained<NSWindow>,
    view: Retained<EditorView>,
}

struct EditorIvars {
    shot: RefCell<Option<OpenShot>>,
    /// Selected element id, if any.
    selection: Cell<Option<u64>>,
    /// The active annotation tool.
    tool: Cell<Tool>,
    /// Color for new elements.
    color: Cell<Rgba>,
    /// Stroke width for new elements, in image pixels.
    stroke_width: Cell<f64>,
    /// The element being drawn by the current drag, if any; committed on
    /// mouse-up.
    draft: RefCell<Option<ElementKind>>,
    /// The open field editor (text/callout input), if any.
    text_editor: RefCell<Option<Retained<NSTextField>>>,
    /// What the open field editor will commit.
    text_target: Cell<Option<TextTarget>>,
    /// Toolbar buttons (tool tag → button) for the active highlight.
    tool_buttons: RefCell<Vec<(isize, Retained<NSButton>)>>,
    /// The color-swatch toolbar button (its bezel shows the current color).
    color_button: RefCell<Option<Retained<NSButton>>>,
    /// The open color popover, if any.
    color_popover: RefCell<Option<Retained<NSPopover>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = EditorIvars]
    struct EditorView;

    unsafe impl NSObjectProtocol for EditorView {}

    impl EditorView {
        #[unsafe(method_id(initWithFrame:))]
        unsafe fn __init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self> {
            let this = this.set_ivars(EditorIvars {
                shot: RefCell::new(None),
                selection: Cell::new(None),
                tool: Cell::new(Tool::Arrow),
                color: Cell::new(COLOR_PRESETS[0]),
                stroke_width: Cell::new(DEFAULT_STROKE_WIDTH),
                draft: RefCell::new(None),
                text_editor: RefCell::new(None),
                text_target: Cell::new(None),
                tool_buttons: RefCell::new(Vec::new()),
                color_button: RefCell::new(None),
                color_popover: RefCell::new(None),
            });
            // SAFETY: NSView's designated initializer with plain ivars set.
            unsafe { msg_send![super(this), initWithFrame: frame_rect] }
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn __accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn __draw_rect(&self, _dirty_rect: CGRect) {
            self.draw();
        }

        #[unsafe(method(mouseDown:))]
        fn __mouse_down(&self, event: &NSEvent) {
            self.handle_mouse_down(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn __mouse_dragged(&self, event: &NSEvent) {
            self.handle_mouse_dragged(event);
        }

        #[unsafe(method(mouseUp:))]
        fn __mouse_up(&self, event: &NSEvent) {
            self.handle_mouse_up(event);
        }

        #[unsafe(method(keyDown:))]
        fn __key_down(&self, event: &NSEvent) {
            if event.keyCode() == 53 {
                // Esc: close the editor.
                if let Some(window) = self.window() {
                    window.close();
                }
            }
        }

        #[unsafe(method(toolClicked:))]
        fn __tool_clicked(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is the clicked NSButton; `tag` is the
            // NSView accessor we stamped on the button.
            let tag: isize = unsafe { msg_send![sender, tag] };
            if let Some(tool) = Tool::from_tag(tag) {
                self.ivars().tool.set(tool);
                self.update_tool_highlight();
            }
        }

        #[unsafe(method(colorClicked:))]
        fn __color_clicked(&self, sender: Option<&AnyObject>) {
            self.toggle_color_popover(sender);
        }

        #[unsafe(method(presetClicked:))]
        fn __preset_clicked(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is the clicked preset NSButton.
            let tag: isize = unsafe { msg_send![sender, tag] };
            let index = tag.clamp(0, COLOR_PRESETS.len() as isize - 1) as usize;
            self.ivars().color.set(COLOR_PRESETS[index]);
            self.update_color_swatch();
            if let Some(popover) = self.ivars().color_popover.borrow_mut().take() {
                // SAFETY: plain close on a live popover.
                unsafe { popover.performClose(None) };
            }
        }

        #[unsafe(method(widthChanged:))]
        fn __width_changed(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is the NSSlider; `doubleValue` is its
            // NSControl accessor.
            let value: f64 = unsafe { msg_send![sender, doubleValue] };
            self.ivars().stroke_width.set(value);
        }
    }

    // SAFETY: only the optional end-editing callback is implemented; the
    // canvas view outlives the field editor it parents.
    unsafe impl NSControlTextEditingDelegate for EditorView {}

    unsafe impl NSTextFieldDelegate for EditorView {
        #[unsafe(method(controlTextDidEndEditing:))]
        fn __control_text_did_end_editing(&self, _notification: &NSNotification) {
            self.commit_text_editor();
        }
    }
);

impl EditorView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    fn new(mtm: MainThreadMarker) -> Retained<EditorView> {
        // SAFETY: designated initializer with plain ivars.
        unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::new(CGPoint::ZERO, INITIAL_SIZE)) }
    }

    /// The image pixel size of the open shot.
    fn image_size(&self) -> (usize, usize) {
        self.ivars()
            .shot
            .borrow()
            .as_ref()
            .map(|shot| {
                let base = shot.doc.base();
                (
                    CGImage::width(Some(base)) as usize,
                    CGImage::height(Some(base)) as usize,
                )
            })
            .unwrap_or((0, 0))
    }

    /// Cursor position in image pixel space (top-left origin): view points
    /// scale by the current frame-to-pixel ratio and flip vertically.
    fn pixel_point(&self, event: &NSEvent) -> Point {
        let location = event.locationInWindow();
        let local = self.convertPoint_fromView(location, None);
        let bounds = self.bounds();
        let (iw, ih) = self.image_size();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 || iw == 0 || ih == 0 {
            return Point::new(0.0, 0.0);
        }
        // Clamp into the frame: drags that leave the image stick to its
        // edge instead of drawing outside it.
        Point::new(
            (local.x * iw as f64 / bounds.size.width).clamp(0.0, iw as f64),
            ((bounds.size.height - local.y) * ih as f64 / bounds.size.height)
                .clamp(0.0, ih as f64),
        )
    }

    /// Redraw everything: backdrop, composite (base + elements), markers.
    fn draw(&self) {
        let Some(context) = NSGraphicsContext::currentContext() else {
            return;
        };
        let cg = context.CGContext();
        let ivars = self.ivars();
        let shot_guard = ivars.shot.borrow();
        let bounds = self.bounds();
        let (bw, bh) = (bounds.size.width, bounds.size.height);
        if bw <= 0.0 || bh <= 0.0 {
            return;
        }

        // Backdrop behind the frame.
        CGContext::set_rgb_fill_color(
            Some(&cg),
            BACKDROP[0],
            BACKDROP[1],
            BACKDROP[2],
            BACKDROP[3],
        );
        CGContext::fill_rect(Some(&cg), bounds);

        let Some(shot) = shot_guard.as_ref() else {
            return;
        };
        let base = shot.doc.base();
        let (iw, ih) = self.image_size();
        if iw == 0 || ih == 0 {
            return;
        }
        let (sx, sy) = (bw / iw as f64, bh / ih as f64);

        // The composite content: base frame + elements, drawn in image
        // pixel units (`draw_elements` applies its own Y flip). The
        // element being edited in place is hidden so it doesn't double
        // under the transparent field editor.
        CGContext::save_g_state(Some(&cg));
        CGContext::scale_ctm(Some(&cg), sx, sy);
        CGContext::draw_image(
            Some(&cg),
            CGRect::new(CGPoint::ZERO, CGSize::new(iw as f64, ih as f64)),
            Some(base),
        );
        let editing = match ivars.text_target.get() {
            Some(TextTarget::Existing { id }) => Some(id),
            _ => None,
        };
        if let Some(id) = editing {
            let visible: Vec<Element> = shot
                .doc
                .elements()
                .iter()
                .filter(|el| el.id != id)
                .cloned()
                .collect();
            render::draw_elements(&cg, base, ih as f64, &visible);
        } else {
            render::draw_elements(&cg, base, ih as f64, shot.doc.elements());
        }
        CGContext::restore_g_state(Some(&cg));

        // Live preview of the element being dragged; view-only, it never
        // reaches a saved composite until mouse-up commits it.
        if let Some(draft) = ivars.draft.borrow().as_ref() {
            let preview = Element {
                id: 0,
                kind: draft.clone(),
            };
            render::draw_elements(&cg, base, ih as f64, std::slice::from_ref(&preview));
        }

        // Selection markers on top, in screen points so they stay constant
        // sized; they never reach a saved composite.
        if let Some(id) = ivars.selection.get() {
            self.draw_markers(&cg, shot, id, &bounds, sx, sy);
        }
    }

    /// Handles of the selected element: endpoint dots and bounding-box
    /// corners, white fill with the element's color ring.
    fn draw_markers(
        &self,
        cg: &CGContext,
        shot: &OpenShot,
        id: u64,
        bounds: &CGRect,
        sx: f64,
        sy: f64,
    ) {
        let Some(element) = shot.doc.element(id) else {
            return;
        };
        let to_point = |p: Point| -> CGPoint { CGPoint::new(p.x * sx, bounds.size.height - p.y * sy) };
        let mut points: Vec<CGPoint> = Vec::new();
        let mut ring: [f64; 4] = [1.0, 0.0, 1.0, 1.0];
        match &element.kind {
            ElementKind::Arrow { a, b, color, .. } | ElementKind::Line { a, b, color, .. } => {
                points = vec![to_point(*a), to_point(*b)];
                ring = *color;
            }
            ElementKind::Callout { tip, anchor, color, .. } => {
                points = vec![to_point(*tip), to_point(*anchor)];
                ring = *color;
            }
            ElementKind::Rect { rect, color, .. } => {
                points = rect_corners(*rect).map(to_point).to_vec();
                ring = *color;
            }
            ElementKind::Blur { rect } => {
                points = rect_corners(*rect).map(to_point).to_vec();
            }
            ElementKind::Text { origin, text, color, size } => {
                if let Some(bbox) = crate::editor::hit_test::text_bounds(*origin, text, *size) {
                    points = rect_corners(bbox).map(to_point).to_vec();
                }
                ring = *color;
            }
            ElementKind::Pen { points: trace, color, .. } => {
                // Endpoint dots only; the trace itself is the shape.
                if let (Some(first), Some(last)) = (trace.first(), trace.last()) {
                    points = vec![to_point(*first), to_point(*last)];
                }
                ring = *color;
            }
        }
        for p in points {
            let r = MARKER_RADIUS_PT;
            let marker_rect =
                CGRect::new(CGPoint::new(p.x - r, p.y - r), CGSize::new(2.0 * r, 2.0 * r));
            CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
            CGContext::fill_ellipse_in_rect(Some(cg), marker_rect);
            CGContext::set_rgb_stroke_color(Some(cg), ring[0], ring[1], ring[2], ring[3]);
            CGContext::set_line_width(Some(cg), 1.5);
            CGContext::stroke_ellipse_in_rect(Some(cg), marker_rect);
        }
    }

    fn handle_mouse_down(&self, event: &NSEvent) {
        let point = self.pixel_point(event);
        let ivars = self.ivars();
        let draft = match ivars.tool.get() {
            Tool::Arrow => ElementKind::Arrow {
                a: point,
                b: point,
                color: ivars.color.get(),
                width: ivars.stroke_width.get(),
            },
            Tool::Rect => ElementKind::Rect {
                rect: Rect::new(point.x, point.y, 0.0, 0.0),
                color: ivars.color.get(),
                width: ivars.stroke_width.get(),
            },
            Tool::Line => ElementKind::Line {
                a: point,
                b: point,
                color: ivars.color.get(),
                width: ivars.stroke_width.get(),
            },
            Tool::Pen => ElementKind::Pen {
                points: vec![point],
                color: ivars.color.get(),
                width: ivars.stroke_width.get(),
            },
            Tool::Blur => ElementKind::Blur {
                rect: Rect::new(point.x, point.y, 0.0, 0.0),
            },
            // Text and callout take the click, not a drag: finish any
            // in-flight edit, then open (or re-open) the field editor.
            Tool::Text | Tool::Callout => {
                self.commit_text_editor();
                let callout = ivars.tool.get() == Tool::Callout;
                // A click on an existing text/callout re-edits it in place.
                let existing = ivars
                    .shot
                    .borrow()
                    .as_ref()
                    .and_then(|shot| top_hit(shot.doc.elements(), point))
                    .and_then(|hit| match hit {
                        Hit::Body(id) => Some(id),
                        Hit::End(..) => None,
                    })
                    .filter(|id| {
                        ivars.shot.borrow().as_ref().and_then(|shot| shot.doc.element(*id)).is_some_and(
                            |el| matches!(el.kind, ElementKind::Text { .. } | ElementKind::Callout { .. }),
                        )
                    });
                match existing {
                    Some(id) => self.begin_text_edit(TextTarget::Existing { id }),
                    None => self.begin_text_edit(TextTarget::New { origin: point, callout }),
                }
                return;
            }
            // Crop/resize have their own click protocol (later task).
            Tool::Crop | Tool::Resize => return,
        };
        // A drawing interaction starts from a clean selection; if this
        // drag ends up a plain click, that is the "click on empty space
        // deselects" rule.
        ivars.selection.set(None);
        *ivars.draft.borrow_mut() = Some(draft);
        self.redraw();
    }

    fn handle_mouse_dragged(&self, event: &NSEvent) {
        self.update_drag(self.pixel_point(event));
    }

    fn handle_mouse_up(&self, event: &NSEvent) {
        self.update_drag(self.pixel_point(event));
        let ivars = self.ivars();
        let Some(kind) = ivars.draft.borrow_mut().take() else {
            return;
        };
        if !Self::draft_is_real(&kind) {
            // Plain click: the draft is discarded and the selection was
            // already cleared at mouse-down.
            self.redraw();
            return;
        }
        let mut shot_slot = ivars.shot.borrow_mut();
        let Some(shot) = shot_slot.as_mut() else {
            return;
        };
        let id = shot.doc.next_element_id();
        let element = Element { id, kind };
        if let Err(err) = shot.apply(Command::AddElement(element)) {
            eprintln!("editor: failed to commit the drawn element: {err}");
            self.redraw();
            return;
        }
        drop(shot_slot);
        ivars.selection.set(Some(id));
        self.redraw();
    }

    /// Move the draft's far end (or grow its trace) to `point`.
    fn update_drag(&self, point: Point) {
        let ivars = self.ivars();
        let mut draft = ivars.draft.borrow_mut();
        let Some(kind) = draft.as_mut() else {
            return;
        };
        match kind {
            ElementKind::Arrow { b, .. } | ElementKind::Line { b, .. } => *b = point,
            // The drag start stays the origin; the size may go negative
            // and is standardized at draw/commit time.
            ElementKind::Rect { rect, .. } | ElementKind::Blur { rect } => {
                *rect = Rect::new(rect.x, rect.y, point.x - rect.x, point.y - rect.y);
            }
            ElementKind::Pen { points, .. } if points.last() != Some(&point) => {
                points.push(point);
            }
            _ => {}
        }
        drop(draft);
        self.redraw();
    }

    /// Whether a finished drag drew something real (as opposed to a plain
    /// click, which must not produce zero-size elements).
    fn draft_is_real(kind: &ElementKind) -> bool {
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
            _ => false,
        }
    }

    /// Schedule a full redraw of the canvas.
    fn redraw(&self) {
        self.setNeedsDisplayInRect(self.bounds());
    }

    /// Scale factors from image pixels to view points.
    fn view_scale(&self) -> (f64, f64) {
        let bounds = self.bounds();
        let (iw, ih) = self.image_size();
        if iw == 0 || ih == 0 {
            return (1.0, 1.0);
        }
        (bounds.size.width / iw as f64, bounds.size.height / ih as f64)
    }

    /// View-space rect for the field editor over a text block whose
    /// top-left sits at `origin` (image pixels), holding `text` (may be
    /// empty) at `size` image pixels.
    fn text_editor_frame(&self, origin: Point, text: &str, size: f64) -> CGRect {
        let bounds = self.bounds();
        let (sx, sy) = self.view_scale();
        let font_pt = (size * sy).max(9.0);
        let width = if text.is_empty() {
            TEXT_EDITOR_WIDTH_PT
        } else {
            let width_px = text::measure(text, size).map(|m| m.width).unwrap_or(0.0);
            (width_px * sx).max(TEXT_EDITOR_WIDTH_PT) + 12.0
        };
        let height = font_pt * 1.6 + 6.0;
        let x = origin.x * sx - 3.0;
        let y_top = bounds.size.height - origin.y * sy;
        CGRect::new(CGPoint::new(x, y_top - height), CGSize::new(width, height))
    }

    /// Open the field editor for `target`: a transparent single-line text
    /// field over the text block. The commit happens in
    /// `controlTextDidEndEditing` — on Enter, on a click elsewhere, or on
    /// Esc (which reverts first, so it reads as cancel).
    fn begin_text_edit(&self, target: TextTarget) {
        let ivars = self.ivars();
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let (initial, frame, color, size_px) = match target {
            TextTarget::New { origin, callout } => {
                let size = DEFAULT_TEXT_SIZE_PX;
                let anchor = if callout {
                    Point::new(
                        origin.x + CALLOUT_TEXT_OFFSET_PX,
                        (origin.y - size * 1.5).max(0.0),
                    )
                } else {
                    origin
                };
                (
                    String::new(),
                    self.text_editor_frame(anchor, "", size),
                    ivars.color.get(),
                    size,
                )
            }
            TextTarget::Existing { id } => {
                let shot_guard = ivars.shot.borrow();
                let Some(shot) = shot_guard.as_ref() else {
                    return;
                };
                let Some(el) = shot.doc.element(id) else {
                    return;
                };
                match &el.kind {
                    ElementKind::Text { origin, text, color, size }
                    | ElementKind::Callout { anchor: origin, text, color, size, tip: _ } => (
                        text.clone(),
                        self.text_editor_frame(*origin, text, *size),
                        *color,
                        *size,
                    ),
                    _ => return,
                }
            }
        };
        let font_pt = (size_px * self.view_scale().1).max(9.0);
        let font = NSFont::fontWithName_size(&NSString::from_str(text::FONT_NAME), font_pt);
        let [r, g, b, a] = color;
        let ns_color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        // SAFETY: class convenience constructor, plain setters afterwards.
        let field = NSTextField::textFieldWithString(&NSString::from_str(&initial), mtm);
        field.setBordered(false);
        field.setDrawsBackground(false);
        field.setFocusRingType(NSFocusRingType::None);
        field.setFrame(frame);
        if let Some(font) = font.as_ref() {
            field.setFont(Some(font));
        }
        field.setTextColor(Some(&ns_color));
        // SAFETY: the delegate is this canvas view, alive for the process.
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(self))) };
        self.addSubview(&field);
        *ivars.text_editor.borrow_mut() = Some(field.clone());
        ivars.text_target.set(Some(target));
        if self.window().is_some_and(|window| window.makeFirstResponder(Some(&field))) {
            if let Some(editor) = field.currentEditor() {
                // SAFETY: plain selection message with no sender.
                unsafe { editor.selectAll(None) };
            }
        }
        self.redraw();
    }

    /// Finish the open field editor: commit its text as a command — add a
    /// new element, edit the existing one's text, or delete an emptied
    /// element. Safe to call when no editor is open.
    fn commit_text_editor(&self) {
        let ivars = self.ivars();
        let Some(field) = ivars.text_editor.borrow_mut().take() else {
            return;
        };
        let target = ivars.text_target.take();
        let text = field.stringValue().to_string();
        // Removing the field ends editing, which re-enters this method;
        // the taken slots above make that a no-op.
        field.removeFromSuperview();

        let Some(target) = target else {
            self.redraw();
            return;
        };
        let mut shot_slot = ivars.shot.borrow_mut();
        let Some(shot) = shot_slot.as_mut() else {
            self.redraw();
            return;
        };
        match target {
            TextTarget::New { origin, callout } => {
                if text.trim().is_empty() {
                    self.redraw();
                    return;
                }
                let size = DEFAULT_TEXT_SIZE_PX;
                let kind = if callout {
                    ElementKind::Callout {
                        tip: origin,
                        anchor: callout_anchor(origin, &text, size),
                        text,
                        color: ivars.color.get(),
                        size,
                    }
                } else {
                    ElementKind::Text {
                        origin,
                        text,
                        color: ivars.color.get(),
                        size,
                    }
                };
                let id = shot.doc.next_element_id();
                if let Err(err) = shot.apply(Command::AddElement(Element { id, kind })) {
                    eprintln!("editor: failed to add the text element: {err}");
                } else {
                    ivars.selection.set(Some(id));
                }
            }
            TextTarget::Existing { id } => {
                let old_text = shot.doc.element(id).and_then(|el| match &el.kind {
                    ElementKind::Text { text, .. } | ElementKind::Callout { text, .. } => {
                        Some(text.clone())
                    }
                    _ => None,
                });
                if text.trim().is_empty() {
                    if let Err(err) = shot.apply(Command::DeleteElement(id)) {
                        eprintln!("editor: failed to delete the emptied text: {err}");
                    } else {
                        ivars.selection.set(None);
                    }
                } else if old_text.as_deref() != Some(text.as_str()) {
                    if let Err(err) = shot.apply(Command::EditText(id, text)) {
                        eprintln!("editor: failed to update the text: {err}");
                    }
                }
            }
        }
        drop(shot_slot);
        self.redraw();
    }

    /// Drop the open field editor without committing (a new shot arrived).
    fn discard_text_editor(&self) {
        let ivars = self.ivars();
        if let Some(field) = ivars.text_editor.borrow_mut().take() {
            // Ends editing, which re-enters commit with both slots empty.
            field.removeFromSuperview();
        }
        ivars.text_target.take();
    }

    /// Accent-tint the active tool's button, clear the others.
    fn update_tool_highlight(&self) {
        let active = self.ivars().tool.get().tag();
        for (tag, button) in self.ivars().tool_buttons.borrow().iter() {
            if *tag == active {
                let accent: Retained<NSColor> = NSColor::controlAccentColor();
                button.setBezelColor(Some(&accent));
            } else {
                button.setBezelColor(None);
            }
        }
    }

    /// Show the current color on the toolbar swatch.
    fn update_color_swatch(&self) {
        let button = self.ivars().color_button.borrow().clone();
        let Some(button) = button.as_ref() else {
            return;
        };
        let [r, g, b, a] = self.ivars().color.get();
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        button.setBezelColor(Some(&color));
    }

    /// Open (or close) the color preset popover under the swatch button.
    fn toggle_color_popover(&self, sender: Option<&AnyObject>) {
        let mut slot = self.ivars().color_popover.borrow_mut();
        if let Some(open) = slot.as_ref() {
            // SAFETY: plain close on a live popover.
            unsafe { open.performClose(None) };
            *slot = None;
            return;
        }
        let Some(anchor) = sender else { return };
        let Some(anchor_button) = anchor.downcast_ref::<NSButton>() else {
            return;
        };
        let mtm = MainThreadMarker::new().expect("popover on main thread");
        // SAFETY: plain init on a freshly allocated popover.
        let popover: Retained<NSPopover> = unsafe { msg_send![NSPopover::alloc(mtm), init] };
        popover.setBehavior(objc2_app_kit::NSPopoverBehavior::Transient);

        let content = NSStackView::new(mtm);
        content.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        content.setSpacing(6.0);
        for (index, color) in COLOR_PRESETS.iter().enumerate() {
            let [r, g, b, a] = *color;
            let ns_color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
            // SAFETY: class convenience constructor, no external invariants.
            let button = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    &NSString::from_str(""),
                    None,
                    None,
                    mtm,
                )
            };
            button.setBordered(false);
            button.setWantsLayer(true);
            if let Some(layer) = button.layer() {
                // SAFETY: plain CGColor bridge of a live color.
                layer.setBackgroundColor(Some(&ns_color.CGColor()));
                layer.setCornerRadius(8.0);
            }
            button.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(18.0, 18.0)));
            button.setTag(index as isize);
            // SAFETY: target is this canvas view, alive for the process.
            unsafe {
                button.setTarget(Some(self));
                button.setAction(Some(sel!(presetClicked:)));
            }
            content.addArrangedSubview(&button);
        }

        let controller = NSViewController::new(mtm);
        let content_view: &NSView = &content;
        controller.setView(content_view);
        popover.setContentViewController(Some(&controller));
        // SAFETY: plain popover presentation with valid rect/view.
        unsafe {
            popover.showRelativeToRect_ofView_preferredEdge(
                anchor_button.bounds(),
                &anchor_button,
                NSRectEdge::MinY,
            );
        }
        *slot = Some(popover);
    }
}

/// Distance between two points.
fn point_dist(a: Point, b: Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Top-left of a new callout's text block: above-right of the tip.
fn callout_anchor(tip: Point, text: &str, size: f64) -> Point {
    let height = text::measure(text, size)
        .map(|m| m.height())
        .unwrap_or(size * 1.25);
    Point::new(
        tip.x + CALLOUT_TEXT_OFFSET_PX,
        (tip.y - height - 6.0).max(0.0),
    )
}

/// The four corners of a rect (top-left, top-right, bottom-right,
/// bottom-left), standardized.
fn rect_corners(rect: Rect) -> [Point; 4] {
    let r = rect.standardize();
    [
        Point::new(r.x, r.y),
        Point::new(r.max_x(), r.y),
        Point::new(r.max_x(), r.max_y()),
        Point::new(r.x, r.max_y()),
    ]
}

/// An SF Symbol toolbar button with an empty title, tagged with the tool.
fn symbol_button(mtm: MainThreadMarker, symbol: &str, tag: isize) -> Retained<NSButton> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        Some(&NSString::from_str(symbol)),
    )
    .expect("SF Symbol image");
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(15.0, 5.0);
    let image = image
        .imageWithSymbolConfiguration(&config)
        .expect("configured symbol");
    // SAFETY: class convenience constructor, no external invariants.
    let button = unsafe {
        NSButton::buttonWithTitle_image_target_action(
            &NSString::from_str(""),
            &image,
            None,
            None,
            mtm,
        )
    };
    button.setTag(tag);
    button
}

impl EditorWindow {
    pub fn new(mtm: MainThreadMarker) -> EditorWindow {
        let view = EditorView::new(mtm);
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                CGRect::new(
                    CGPoint::ZERO,
                    CGSize::new(INITIAL_SIZE.width, INITIAL_SIZE.height + TOOLBAR_HEIGHT),
                ),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str("minisnap"));
        // The window lives in the app controller for the whole process;
        // closing only hides it, the next shot reuses it (no second window).
        // SAFETY: plain setter on the freshly created window.
        unsafe {
            window.setReleasedWhenClosed(false);
        }

        // Toolbar: tool buttons, a color swatch with a preset popover, a
        // stroke-width slider.
        let toolbar = NSStackView::new(mtm);
        toolbar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        toolbar.setSpacing(6.0);
        let mut tool_buttons = Vec::new();
        for tool in Tool::all() {
            let button = symbol_button(mtm, tool.symbol(), tool.tag());
            // SAFETY: target is this canvas view, alive for the process.
            unsafe {
                button.setTarget(Some(&view));
                button.setAction(Some(sel!(toolClicked:)));
            }
            toolbar.addArrangedSubview(&button);
            tool_buttons.push((tool.tag(), button));
        }
        let color_button = symbol_button(mtm, "paintpalette", -1);
        // SAFETY: target is this canvas view, alive for the process.
        unsafe {
            color_button.setTarget(Some(&view));
            color_button.setAction(Some(sel!(colorClicked:)));
        }
        toolbar.addArrangedSubview(&color_button);
        // SAFETY: class convenience constructor for the slider.
        let slider = unsafe {
            objc2_app_kit::NSSlider::sliderWithValue_minValue_maxValue_target_action(
                DEFAULT_STROKE_WIDTH,
                1.0,
                12.0,
                Some(&view),
                Some(sel!(widthChanged:)),
                mtm,
            )
        };
        toolbar.addArrangedSubview(&slider);

        // Layout: toolbar strip at the top, canvas below it.
        let content = window.contentView().expect("content view");
        let size = content.bounds().size;
        toolbar.setFrame(CGRect::new(
            CGPoint::new(8.0, size.height - TOOLBAR_HEIGHT),
            CGSize::new(size.width - 16.0, TOOLBAR_HEIGHT),
        ));
        toolbar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        content.addSubview(&toolbar);
        let canvas_height = size.height - TOOLBAR_HEIGHT;
        view.setFrame(CGRect::new(
            CGPoint::ZERO,
            CGSize::new(size.width, canvas_height),
        ));
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview(&view);

        *view.ivars().tool_buttons.borrow_mut() = tool_buttons;
        *view.ivars().color_button.borrow_mut() = Some(color_button);
        view.update_tool_highlight();
        view.update_color_swatch();

        EditorWindow { window, view }
    }

    /// Show (or replace) the shot in this window: title, size limited by the
    /// screen's visible frame, full redraw.
    pub fn show_shot(&self, shot: OpenShot) {
        let title = shot.file_name();
        let (iw, ih) = {
            let base = shot.doc.base();
            (
                CGImage::width(Some(base)) as usize,
                CGImage::height(Some(base)) as usize,
            )
        };
        let view = &self.view;
        // A new shot arrives: drop any open text editor without committing.
        view.discard_text_editor();
        *view.ivars().shot.borrow_mut() = Some(shot);
        view.ivars().selection.set(None);
        self.window.setTitle(&NSString::from_str(&title));

        // Content size = frame size in points plus the toolbar, uniformly
        // scaled down when the frame exceeds the screen's visible frame.
        let mtm = MainThreadMarker::new().expect("editor window on main thread");
        let content = NSScreen::mainScreen(mtm)
            .map(|screen| screen.visibleFrame().size)
            .map(|visible| {
                fit_size(iw as f64, ih as f64, visible.width, visible.height - TOOLBAR_HEIGHT)
            })
            .unwrap_or_else(|| CGSize::new(iw as f64, ih as f64));
        self.window
            .setContentSize(CGSize::new(content.width, content.height + TOOLBAR_HEIGHT));
        self.window.center();
        view.setNeedsDisplayInRect(view.bounds());
        self.window.makeKeyAndOrderFront(None);
    }

    /// The selected element id, if any.
    pub fn selection(&self) -> Option<u64> {
        self.view.ivars().selection.get()
    }

    pub fn set_selection(&self, id: Option<u64>) {
        let view = &self.view;
        view.ivars().selection.set(id);
        view.setNeedsDisplayInRect(view.bounds());
    }
}

/// Uniformly fit `(w, h)` inside `(max_w, max_h)` without upscaling.
fn fit_size(w: f64, h: f64, max_w: f64, max_h: f64) -> CGSize {
    if w <= 0.0 || h <= 0.0 {
        return CGSize::new(w, h);
    }
    let scale = 1.0_f64.min(max_w / w).min(max_h / h);
    CGSize::new(w * scale, h * scale)
}
