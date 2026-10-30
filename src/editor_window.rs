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
    NSCursor, NSEvent, NSEventModifierFlags, NSFocusRingType, NSFont, NSFontWeightRegular,
    NSGraphicsContext, NSImage,
    NSImageSymbolConfiguration, NSPopover, NSPopoverDelegate, NSScreen, NSStackView, NSTextField,
    NSTextFieldDelegate, NSUserInterfaceLayoutOrientation, NSView, NSViewController, NSWindow,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGImage, CGContext};
use objc2_foundation::{NSString, NSNotification, NSRectEdge};

use crate::editor::hit_test::{Hit, top_hit};
use crate::editor::text;
use crate::editor::{
    render, set_endpoint, shift_element, Command, Element, ElementKind, Endpoint, OpenShot, Point,
    Rect, Rgba, Size,
};

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

/// Radius of the crop/resize frame handles, in view points.
const HANDLE_RADIUS_PT: f64 = 5.0;

/// Size of the frame mode's OK button, in view points.
const FRAME_OK_SIZE: CGSize = CGSize::new(52.0, 26.0);

/// Toolbar tags of the ◄/► leafing buttons.
const PAGE_PREV_TAG: isize = -2;
const PAGE_NEXT_TAG: isize = -3;

/// What the open field editor is editing.
#[derive(Clone, Copy)]
enum TextTarget {
    /// A brand-new text or callout element.
    New { origin: Point, callout: bool },
    /// An existing text/callout element being re-edited.
    Existing { id: u64 },
}

/// The in-progress mouse interaction; committed as one command on mouse-up.
enum Drag {
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
enum FrameSpot {
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
struct FrameEdit {
    /// True for the resize tool (resample to the frame size), false for
    /// crop (cut the frame out).
    resize: bool,
    rect: Rect,
}

/// Color presets offered by the popover; magenta is the default.
const COLOR_PRESETS: [Rgba; 9] = [
    [1.0, 0.0, 1.0, 1.0],  // magenta
    [1.0, 0.0, 0.0, 1.0],  // red
    [1.0, 0.5, 0.0, 1.0],  // orange
    [1.0, 0.85, 0.0, 1.0], // yellow
    [0.0, 0.75, 0.0, 1.0], // green
    [0.0, 0.7, 0.9, 1.0],  // cyan
    [0.1, 0.3, 1.0, 1.0],  // blue
    [0.0, 0.0, 0.0, 1.0],  // black
    [1.0, 1.0, 1.0, 1.0],  // white
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
    /// The active annotation tool (None when no tool is picked: clicks only
    /// select and move elements).
    tool: Cell<Option<Tool>>,
    /// Color for new elements.
    color: Cell<Rgba>,
    /// Stroke width for new elements, in image pixels.
    stroke_width: Cell<f64>,
    /// The in-progress mouse drag (draw, move, or endpoint drag).
    drag: RefCell<Option<Drag>>,
    /// The open field editor (text/callout input), if any.
    text_editor: RefCell<Option<Retained<NSTextField>>>,
    /// What the open field editor will commit.
    text_target: Cell<Option<TextTarget>>,
    /// The active crop/resize frame, if the tool is up.
    frame_edit: RefCell<Option<FrameEdit>>,
    /// The frame mode's size label and OK button subviews.
    frame_label: RefCell<Option<Retained<NSTextField>>>,
    frame_ok: RefCell<Option<Retained<NSButton>>>,
    /// The folder the shots are saved to (for the ◄/► leafing).
    save_dir: RefCell<std::path::PathBuf>,
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
                tool: Cell::new(Some(Tool::Arrow)),
                color: Cell::new(COLOR_PRESETS[0]),
                stroke_width: Cell::new(DEFAULT_STROKE_WIDTH),
                drag: RefCell::new(None),
                text_editor: RefCell::new(None),
                text_target: Cell::new(None),
                frame_edit: RefCell::new(None),
                frame_label: RefCell::new(None),
                frame_ok: RefCell::new(None),
                save_dir: RefCell::new(std::path::PathBuf::new()),
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
            match event.keyCode() {
                53 => {
                    // Esc: cancel the crop/resize frame, else close.
                    if self.ivars().frame_edit.borrow().is_some() {
                        self.cancel_frame_edit();
                    } else if let Some(window) = self.window() {
                        window.close();
                    }
                }
                // ⌘Q closes the viewer; the app stays in the tray.
                12 if event.modifierFlags().contains(NSEventModifierFlags::Command) => {
                    if let Some(window) = self.window() {
                        window.close();
                    }
                }
                // Return and keypad enter apply the frame.
                36 | 76 => self.commit_frame_edit(),
                // ⌘C copies the composite (no markers, no chrome).
                8 if event.modifierFlags().contains(NSEventModifierFlags::Command) => {
                    self.copy_composite()
                }
                // ⌘Z undoes, ⌘⇧Z redoes, through the journal.
                6 if event.modifierFlags().contains(NSEventModifierFlags::Command) => self
                    .undo_redo(event.modifierFlags().contains(NSEventModifierFlags::Shift)),
                // ←/→ leaf through the saved shots.
                123 => self.page(-1),
                124 => self.page(1),
                // Backspace and forward delete remove the selection.
                51 | 117 => self.delete_selected(),
                _ => {}
            }
        }

        #[unsafe(method(resetCursorRects))]
        fn __reset_cursor_rects(&self) {
            if self.ivars().frame_edit.borrow().is_none() {
                return;
            }
            let cursor = NSCursor::crosshairCursor();
            for (_, point) in self.frame_spot_points() {
                let rect = self.spot_cursor_rect(point);
                self.addCursorRect_cursor(rect, &cursor);
            }
        }

        #[unsafe(method(frameOkClicked:))]
        fn __frame_ok_clicked(&self, _sender: Option<&AnyObject>) {
            self.commit_frame_edit();
        }

        #[unsafe(method(pageClicked:))]
        fn __page_clicked(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is the clicked NSButton; `tag` is the
            // NSView accessor we stamped on the button.
            let tag: isize = unsafe { msg_send![sender, tag] };
            if tag == PAGE_PREV_TAG {
                self.page(-1);
            } else {
                self.page(1);
            }
        }

        #[unsafe(method(toolClicked:))]
        fn __tool_clicked(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is the clicked NSButton; `tag` is the
            // NSView accessor we stamped on the button.
            let tag: isize = unsafe { msg_send![sender, tag] };
            if let Some(tool) = Tool::from_tag(tag) {
                // Clicking the active tool again deselects it.
                let next = if self.ivars().tool.get() == Some(tool) {
                    None
                } else {
                    Some(tool)
                };
                self.ivars().tool.set(next);
                self.update_tool_highlight();
                // The crop/resize tools bring up their adjustable frame;
                // anything else (including deselecting the frame tool)
                // tears it down.
                match next {
                    Some(Tool::Crop) => self.enter_frame_edit(false),
                    Some(Tool::Resize) => self.enter_frame_edit(true),
                    _ => self.exit_frame_edit(),
                }
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
            let color = COLOR_PRESETS[index];
            self.ivars().color.set(color);
            self.update_color_swatch();
            // A picked preset recolors the selected element (blur, which has
            // no color, is skipped so undo never holds a no-op step).
            if let Some(id) = self.ivars().selection.get() {
                let recolorable = self
                    .ivars()
                    .shot
                    .borrow()
                    .as_ref()
                    .and_then(|shot| shot.doc.element(id))
                    .is_some_and(|el| !matches!(el.kind, ElementKind::Blur { .. }));
                if recolorable {
                    let result = self
                        .ivars()
                        .shot
                        .borrow_mut()
                        .as_mut()
                        .map(|shot| shot.apply(Command::Recolor(id, color)));
                    if let Some(Err(err)) = result {
                        eprintln!("editor: failed to recolor the element: {err}");
                    }
                }
            }
            if let Some(popover) = self.ivars().color_popover.borrow_mut().take() {
                // SAFETY: plain close on a live popover.
                unsafe { popover.performClose(None) };
            }
            self.redraw();
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
    // canvas view outlives the field editor it parents. The method belongs
    // on this protocol itself: objc2's debug build rejects registering it
    // via the sub-protocol NSTextFieldDelegate, which only inherits it.
    unsafe impl NSControlTextEditingDelegate for EditorView {
        #[unsafe(method(controlTextDidEndEditing:))]
        fn __control_text_did_end_editing(&self, _notification: &NSNotification) {
            self.commit_text_editor();
        }
    }

    // SAFETY: empty — NSTextField's delegate property is typed
    // id<NSTextFieldDelegate>, so the sub-protocol conformance is what the
    // ProtocolObject cast in the field editor needs.
    unsafe impl NSTextFieldDelegate for EditorView {}

    // SAFETY: only the did-close callback is implemented; the canvas view
    // outlives the popover it anchors.
    unsafe impl NSPopoverDelegate for EditorView {
        #[unsafe(method(popoverDidClose:))]
        fn __popover_did_close(&self, _notification: &NSNotification) {
            // Forget the closed popover so the next click on the swatch
            // button opens a fresh one.
            *self.ivars().color_popover.borrow_mut() = None;
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

    /// Remember the shots folder for leafing.
    fn set_save_dir(&self, dir: std::path::PathBuf) {
        *self.ivars().save_dir.borrow_mut() = dir;
    }

    /// The image pixel size of the open shot.
    fn image_size(&self) -> (usize, usize) {
        self.ivars()
            .shot
            .borrow()
            .as_ref()
            .map(|shot| {
                let base = shot.doc.base();
                (CGImage::width(Some(base)), CGImage::height(Some(base)))
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
        // pixel units (`draw_elements` applies its own Y flip). A move or
        // endpoint drag hides its element and shows a transformed preview;
        // an element under in-place text editing is hidden entirely.
        CGContext::save_g_state(Some(&cg));
        CGContext::scale_ctm(Some(&cg), sx, sy);
        CGContext::draw_image(
            Some(&cg),
            CGRect::new(CGPoint::ZERO, CGSize::new(iw as f64, ih as f64)),
            Some(base),
        );
        let drag_guard = ivars.drag.borrow();
        let preview: Option<(u64, Element)> = drag_guard.as_ref().and_then(|drag| match drag {
            Drag::Move { id, start, last, .. } => {
                let mut el = shot.doc.element(*id)?.clone();
                shift_element(&mut el.kind, last.x - start.x, last.y - start.y);
                Some((*id, el))
            }
            Drag::End { id, endpoint, last, .. } => {
                let mut el = shot.doc.element(*id)?.clone();
                set_endpoint(&mut el.kind, *endpoint, *last);
                Some((*id, el))
            }
            Drag::Draw(_) => None,
            // The frame drag previews through the frame overlay, not here.
            Drag::Frame { .. } => None,
        });
        let hidden = preview.as_ref().map(|(id, _)| *id);
        let editing = match ivars.text_target.get() {
            Some(TextTarget::Existing { id }) => Some(id),
            _ => None,
        };
        if hidden.is_some() || editing.is_some() {
            let visible: Vec<Element> = shot
                .doc
                .elements()
                .iter()
                .filter(|el| Some(el.id) != hidden && Some(el.id) != editing)
                .cloned()
                .collect();
            render::draw_elements(&cg, base, ih as f64, &visible);
        } else {
            render::draw_elements(&cg, base, ih as f64, shot.doc.elements());
        }
        if let Some((_, el)) = preview.as_ref() {
            render::draw_elements(&cg, base, ih as f64, std::slice::from_ref(el));
        }
        drop(drag_guard);
        // Live preview of the element being drawn (view-only until the
        // mouse-up commit).
        if let Some(Drag::Draw(kind)) = ivars.drag.borrow().as_ref() {
            let draw_preview = Element {
                id: 0,
                kind: kind.clone(),
            };
            render::draw_elements(&cg, base, ih as f64, std::slice::from_ref(&draw_preview));
        }
        CGContext::restore_g_state(Some(&cg));

        // Selection markers on top, in screen points so they stay constant
        // sized; they never reach a saved composite.
        if let Some(id) = ivars.selection.get() {
            self.draw_markers(&cg, shot, id, &bounds, sx, sy);
        }

        // The crop/resize frame overlay on top of everything.
        if ivars.frame_edit.borrow().is_some() {
            self.draw_frame_overlay(&cg, &bounds, sx, sy);
        }
    }

    /// The crop/resize frame: dimmed frame surroundings, white border,
    /// nine handle dots.
    fn draw_frame_overlay(&self, cg: &CGContext, bounds: &CGRect, sx: f64, sy: f64) {
        let Some(rect) = self.current_frame_rect() else {
            return;
        };
        let vx = rect.x * sx;
        let vw = rect.width * sx;
        let vh = rect.height * sy;
        let y0 = bounds.size.height - rect.max_y() * sy;
        // Dim everything outside the frame.
        CGContext::set_rgb_fill_color(Some(cg), 0.0, 0.0, 0.0, 0.45);
        let dim = [
            CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(bounds.size.width, y0.max(0.0))),
            CGRect::new(
                CGPoint::new(0.0, y0 + vh),
                CGSize::new(bounds.size.width, (bounds.size.height - y0 - vh).max(0.0)),
            ),
            CGRect::new(CGPoint::new(0.0, y0), CGSize::new(vx.max(0.0), vh)),
            CGRect::new(
                CGPoint::new(vx + vw, y0),
                CGSize::new((bounds.size.width - vx - vw).max(0.0), vh),
            ),
        ];
        for r in dim {
            CGContext::fill_rect(Some(cg), r);
        }
        // Frame border.
        CGContext::set_rgb_stroke_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
        CGContext::set_line_width(Some(cg), 1.5);
        CGContext::stroke_rect(Some(cg), CGRect::new(CGPoint::new(vx, y0), CGSize::new(vw, vh)));
        // Handle dots: white fill with the accent ring.
        for (_, point) in self.frame_spot_points() {
            let c = CGPoint::new(point.x * sx, bounds.size.height - point.y * sy);
            let r = HANDLE_RADIUS_PT;
            let marker =
                CGRect::new(CGPoint::new(c.x - r, c.y - r), CGSize::new(2.0 * r, 2.0 * r));
            CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
            CGContext::fill_ellipse_in_rect(Some(cg), marker);
            let accent: Retained<NSColor> = NSColor::controlAccentColor();
            let accent_cg = accent.CGColor();
            CGContext::set_stroke_color_with_color(Some(cg), Some(&accent_cg));
            CGContext::set_line_width(Some(cg), 1.5);
            CGContext::stroke_ellipse_in_rect(Some(cg), marker);
        }
    }

    /// Handles of the selected element: endpoint dots and bounding-box
    /// corners, white fill with the system accent ring.
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
        let points: Vec<CGPoint> = match &element.kind {
            ElementKind::Arrow { a, b, .. } | ElementKind::Line { a, b, .. } => {
                vec![to_point(*a), to_point(*b)]
            }
            ElementKind::Callout { tip, anchor, .. } => vec![to_point(*tip), to_point(*anchor)],
            ElementKind::Rect { rect, .. } => rect_corners(*rect).map(to_point).to_vec(),
            ElementKind::Blur { rect } => rect_corners(*rect).map(to_point).to_vec(),
            ElementKind::Text { origin, text, size, .. } => {
                match crate::editor::hit_test::text_bounds(*origin, text, *size) {
                    Some(bbox) => rect_corners(bbox).map(to_point).to_vec(),
                    None => Vec::new(),
                }
            }
            ElementKind::Pen { points: trace, .. } => {
                // Endpoint dots only; the trace itself is the shape.
                match (trace.first(), trace.last()) {
                    (Some(first), Some(last)) => vec![to_point(*first), to_point(*last)],
                    _ => Vec::new(),
                }
            }
        };
        let accent: Retained<NSColor> = NSColor::controlAccentColor();
        let accent_cg = accent.CGColor();
        for p in points {
            let r = MARKER_RADIUS_PT;
            let marker_rect =
                CGRect::new(CGPoint::new(p.x - r, p.y - r), CGSize::new(2.0 * r, 2.0 * r));
            CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
            CGContext::fill_ellipse_in_rect(Some(cg), marker_rect);
            CGContext::set_stroke_color_with_color(Some(cg), Some(&accent_cg));
            CGContext::set_line_width(Some(cg), 1.5);
            CGContext::stroke_ellipse_in_rect(Some(cg), marker_rect);
        }
    }

    fn handle_mouse_down(&self, event: &NSEvent) {
        let point = self.pixel_point(event);
        let ivars = self.ivars();
        // Finish any in-flight text edit before interpreting the click.
        self.commit_text_editor();
        let draft_kind: Option<ElementKind> = match ivars.tool.get() {
            Some(Tool::Arrow) => Some(ElementKind::Arrow {
                a: point,
                b: point,
                color: ivars.color.get(),
                width: ivars.stroke_width.get(),
            }),
            Some(Tool::Rect) => Some(ElementKind::Rect {
                rect: Rect::new(point.x, point.y, 0.0, 0.0),
                color: ivars.color.get(),
                width: ivars.stroke_width.get(),
            }),
            Some(Tool::Line) => Some(ElementKind::Line {
                a: point,
                b: point,
                color: ivars.color.get(),
                width: ivars.stroke_width.get(),
            }),
            Some(Tool::Pen) => Some(ElementKind::Pen {
                points: vec![point],
                color: ivars.color.get(),
                width: ivars.stroke_width.get(),
            }),
            Some(Tool::Blur) => Some(ElementKind::Blur {
                rect: Rect::new(point.x, point.y, 0.0, 0.0),
            }),
            // Text and callout take the click, not a drag: open (or
            // re-open) the field editor.
            Some(Tool::Text) | Some(Tool::Callout) => {
                let callout = ivars.tool.get() == Some(Tool::Callout);
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
            // Crop/resize drag the frame's spots; the click never draws
            // or selects elements.
            Some(Tool::Crop) | Some(Tool::Resize) => {
                if let Some((spot, _)) = self.frame_spot_at(point) {
                    let orig = self
                        .ivars()
                        .frame_edit
                        .borrow()
                        .as_ref()
                        .map(|edit| edit.rect);
                    if let Some(orig) = orig {
                        *self.ivars().drag.borrow_mut() = Some(Drag::Frame {
                            spot,
                            orig,
                            start: point,
                            last: point,
                        });
                    }
                }
                return;
            }
            // No tool picked: the click only selects or deselects.
            None => None,
        };
        // A hit on an existing element decides between moving it and
        // dragging one of its ends; the empty frame draws a new element.
        let hit = ivars
            .shot
            .borrow()
            .as_ref()
            .and_then(|shot| top_hit(shot.doc.elements(), point));
        let drag = match hit {
            Some(Hit::End(id, endpoint)) => {
                ivars.selection.set(Some(id));
                Drag::End {
                    id,
                    endpoint,
                    start: point,
                    last: point,
                }
            }
            Some(Hit::Body(id)) => {
                ivars.selection.set(Some(id));
                Drag::Move {
                    id,
                    start: point,
                    last: point,
                    moved: false,
                }
            }
            _ => {
                // Drawing on the empty frame starts from a clean selection;
                // a plain click here is the deselect rule. Without a tool
                // the click just deselects.
                ivars.selection.set(None);
                match draft_kind {
                    Some(kind) => Drag::Draw(kind),
                    None => {
                        self.redraw();
                        return;
                    }
                }
            }
        };
        *ivars.drag.borrow_mut() = Some(drag);
        self.redraw();
    }

    fn handle_mouse_dragged(&self, event: &NSEvent) {
        self.update_drag(self.pixel_point(event));
    }

    fn handle_mouse_up(&self, event: &NSEvent) {
        self.update_drag(self.pixel_point(event));
        let ivars = self.ivars();
        let Some(drag) = ivars.drag.borrow_mut().take() else {
            return;
        };
        match drag {
            Drag::Draw(kind) => {
                if !Self::draft_is_real(&kind) {
                    // Plain click: the draft is discarded and the selection
                    // was already cleared at mouse-down.
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
            Drag::Move { id, start, last, moved } => {
                if moved {
                    let delta = Point::new(last.x - start.x, last.y - start.y);
                    let result = ivars
                        .shot
                        .borrow_mut()
                        .as_mut()
                        .map(|shot| shot.apply(Command::MoveElement(id, delta)));
                    if let Some(Err(err)) = result {
                        eprintln!("editor: failed to move the element: {err}");
                    }
                }
                self.redraw();
            }
            Drag::End { id, endpoint, start, last } => {
                if point_dist(start, last) >= MIN_DRAG_PX {
                    let result = ivars
                        .shot
                        .borrow_mut()
                        .as_mut()
                        .map(|shot| shot.apply(Command::MovePoint(id, endpoint, last)));
                    if let Some(Err(err)) = result {
                        eprintln!("editor: failed to drag the endpoint: {err}");
                    }
                }
                self.redraw();
            }
            Drag::Frame { spot, orig, start, last } => {
                // Persist the settled rect; nothing is applied yet — the
                // OK button and Enter do that.
                let rect = frame_rect(orig, spot, start, last);
                if let Some(edit) = ivars.frame_edit.borrow_mut().as_mut() {
                    edit.rect = rect;
                }
                self.layout_frame_chrome();
                self.redraw();
            }
        }
    }

    /// Move the drag's trailing point (or grow the drawn trace) to `point`.
    fn update_drag(&self, point: Point) {
        let ivars = self.ivars();
        {
            let mut drag_slot = ivars.drag.borrow_mut();
            let Some(drag) = drag_slot.as_mut() else {
                return;
            };
            match drag {
                Drag::Draw(kind) => match kind {
                    ElementKind::Arrow { b, .. } | ElementKind::Line { b, .. } => *b = point,
                    // The drag start stays the origin; the size may go
                    // negative and is standardized at draw/commit time.
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
        }
        self.layout_frame_chrome();
        self.redraw();
    }

    /// Delete the selected element, if any.
    fn delete_selected(&self) {
        let ivars = self.ivars();
        let Some(id) = ivars.selection.get() else {
            return;
        };
        let result = ivars
            .shot
            .borrow_mut()
            .as_mut()
            .map(|shot| shot.apply(Command::DeleteElement(id)));
        if let Some(Err(err)) = result {
            eprintln!("editor: failed to delete the element: {err}");
        }
        ivars.selection.set(None);
        self.redraw();
    }

    /// Put the current composite — frame plus elements, no markers or
    /// chrome — on the general pasteboard.
    fn copy_composite(&self) {
        let ivars = self.ivars();
        let shot_guard = ivars.shot.borrow();
        let Some(shot) = shot_guard.as_ref() else {
            return;
        };
        let Some(image) = render::render_composite(shot.doc.base(), shot.doc.elements()) else {
            eprintln!("editor: failed to render the composite for copying");
            return;
        };
        drop(shot_guard);
        if let Err(err) = crate::output::copy_to_pasteboard(&image) {
            eprintln!("editor: failed to copy to the pasteboard: {err}");
        }
    }

    /// Undo (⌘Z) or redo (⌘⇧Z) one journal command; a base-size change
    /// (crop/resize) refits the window.
    fn undo_redo(&self, redo: bool) {
        let ivars = self.ivars();
        // An in-flight text edit commits first, then participates in undo.
        self.commit_text_editor();
        let size_before = self.image_size();
        let result = ivars.shot.borrow_mut().as_mut().map(|shot| {
            if redo {
                shot.redo()
            } else {
                shot.undo()
            }
        });
        let changed = match result {
            Some(Ok(changed)) => changed,
            Some(Err(err)) => {
                eprintln!("editor: undo/redo failed: {err}");
                return;
            }
            None => return,
        };
        if !changed {
            return;
        }
        // A selection may now point at an element that is gone (or come
        // back after a redo of a delete — the id survives the replay).
        let selection = ivars.selection.get();
        let selection_alive = selection.is_some_and(|id| {
            ivars
                .shot
                .borrow()
                .as_ref()
                .is_some_and(|shot| shot.doc.element(id).is_some())
        });
        if !selection_alive {
            ivars.selection.set(None);
        }
        if self.image_size() != size_before {
            self.refit_window();
        } else {
            self.redraw();
        }
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

    /// The crop/resize frame rect in image pixels, including an in-flight
    /// drag.
    fn current_frame_rect(&self) -> Option<Rect> {
        let ivars = self.ivars();
        let edit = ivars.frame_edit.borrow();
        let edit = edit.as_ref()?;
        let drag = ivars.drag.borrow();
        Some(match drag.as_ref() {
            Some(Drag::Frame { spot, orig, start, last }) => frame_rect(*orig, *spot, *start, *last),
            _ => edit.rect,
        })
    }

    /// The nine frame spots (image pixels), corners first for hit priority.
    fn frame_spot_points(&self) -> Vec<(FrameSpot, Point)> {
        let Some(r) = self.current_frame_rect() else {
            return Vec::new();
        };
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
            (FrameSpot::Center, Point::new(cx, cy)),
        ]
    }

    /// The frame spot under an image-space cursor, if any.
    fn frame_spot_at(&self, p: Point) -> Option<(FrameSpot, Point)> {
        let (sx, sy) = self.view_scale();
        let tol_x = (HANDLE_RADIUS_PT + 3.0) / sx.max(0.001);
        let tol_y = (HANDLE_RADIUS_PT + 3.0) / sy.max(0.001);
        self.frame_spot_points()
            .into_iter()
            .find(|(_, c)| (p.x - c.x).abs() <= tol_x && (p.y - c.y).abs() <= tol_y)
    }

    /// The view-space cursor rect around a frame spot.
    fn spot_cursor_rect(&self, point: Point) -> CGRect {
        let bounds = self.bounds();
        let (sx, sy) = self.view_scale();
        let pad = HANDLE_RADIUS_PT + 3.0;
        let cx = point.x * sx;
        let cy = bounds.size.height - point.y * sy;
        CGRect::new(
            CGPoint::new(cx - pad, cy - pad),
            CGSize::new(pad * 2.0, pad * 2.0),
        )
    }

    /// Bring up the crop (`resize = false`) or resize frame with its size
    /// label, OK button, and crosshair cursor zones.
    fn enter_frame_edit(&self, resize: bool) {
        let ivars = self.ivars();
        self.exit_frame_edit();
        // Entering crop/resize drops the element selection.
        ivars.selection.set(None);
        let (iw, ih) = self.image_size();
        if iw == 0 || ih == 0 {
            return;
        }
        let rect = if resize {
            let (w, h) = (iw as f64 * 0.75, ih as f64 * 0.75);
            Rect::new((iw as f64 - w) / 2.0, (ih as f64 - h) / 2.0, w, h)
        } else {
            Rect::new(0.0, 0.0, iw as f64, ih as f64)
        };
        *ivars.frame_edit.borrow_mut() = Some(FrameEdit { resize, rect });

        let mtm = MainThreadMarker::new().expect("frame chrome on main thread");
        let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        label.setTextColor(Some(&NSColor::whiteColor()));
        self.addSubview(&label);
        // SAFETY: class convenience constructor, plain setters afterwards.
        let ok = unsafe {
            NSButton::buttonWithTitle_target_action(&NSString::from_str("OK"), None, None, mtm)
        };
        // SAFETY: target is this canvas view, alive for the process.
        unsafe {
            ok.setTarget(Some(self));
            ok.setAction(Some(sel!(frameOkClicked:)));
        }
        self.addSubview(&ok);
        *ivars.frame_label.borrow_mut() = Some(label);
        *ivars.frame_ok.borrow_mut() = Some(ok);
        self.layout_frame_chrome();
        self.redraw();
    }

    /// Position the frame mode's size label and OK button along the frame's
    /// bottom edge (above it when the frame reaches the window bottom).
    fn layout_frame_chrome(&self) {
        let ivars = self.ivars();
        let Some(rect) = self.current_frame_rect() else {
            return;
        };
        let bounds = self.bounds();
        let (sx, sy) = self.view_scale();
        let bottom_y = bounds.size.height - rect.max_y() * sy;
        let top_y = bounds.size.height - rect.y * sy;
        let below = bottom_y - FRAME_OK_SIZE.height - 6.0;
        let strip_y = if below >= 2.0 { below } else { top_y + 6.0 };
        if let Some(ok) = ivars.frame_ok.borrow().as_ref() {
            let x = (rect.max_x() * sx - FRAME_OK_SIZE.width)
                .clamp(0.0, (bounds.size.width - FRAME_OK_SIZE.width).max(0.0));
            ok.setFrame(CGRect::new(
                CGPoint::new(x, strip_y),
                CGSize::new(FRAME_OK_SIZE.width, FRAME_OK_SIZE.height),
            ));
        }
        if let Some(label) = ivars.frame_label.borrow().as_ref() {
            let text = format!("{} × {}", rect.width.round(), rect.height.round());
            label.setStringValue(&NSString::from_str(&text));
            let label_w = 160.0;
            let x = (rect.x * sx).clamp(0.0, (bounds.size.width - label_w).max(0.0));
            label.setFrame(CGRect::new(
                CGPoint::new(x, strip_y),
                CGSize::new(label_w, 18.0),
            ));
        }
    }

    /// Apply the frame as a Crop or Resize command and return to the arrow
    /// tool. A degenerate (sub-pixel) frame is kept up for another try.
    fn commit_frame_edit(&self) {
        let ivars = self.ivars();
        let Some(edit) = ivars.frame_edit.borrow_mut().take() else {
            return;
        };
        self.remove_frame_chrome();
        let (iw, ih) = self.image_size();
        let r = edit.rect.standardize();
        let x0 = r.x.max(0.0);
        let y0 = r.y.max(0.0);
        let x1 = r.max_x().min(iw as f64);
        let y1 = r.max_y().min(ih as f64);
        let (w, h) = ((x1 - x0).round(), (y1 - y0).round());
        if w < 1.0 || h < 1.0 {
            *ivars.frame_edit.borrow_mut() = Some(edit);
            self.layout_frame_chrome();
            self.redraw();
            return;
        }
        let cmd = if edit.resize {
            Command::Resize(Size::new(w, h))
        } else {
            Command::Crop(Rect::new(x0, y0, w, h))
        };
        let result = ivars
            .shot
            .borrow_mut()
            .as_mut()
            .map(|shot| shot.apply(cmd));
        if let Some(Err(err)) = result {
            eprintln!("editor: failed to apply the frame: {err}");
        }
        ivars.tool.set(None);
        self.update_tool_highlight();
        self.redraw();
        self.refit_window();
    }

    /// Cancel the frame mode (Esc): drop the frame, deselect the tool.
    fn cancel_frame_edit(&self) {
        self.exit_frame_edit();
        self.ivars().tool.set(None);
        self.update_tool_highlight();
        self.redraw();
    }

    /// Tear the frame mode down (tool switch, new shot).
    fn exit_frame_edit(&self) {
        let ivars = self.ivars();
        if ivars.frame_edit.borrow().is_some() {
            *ivars.frame_edit.borrow_mut() = None;
            self.remove_frame_chrome();
            self.redraw();
        }
    }

    /// Remove the frame mode subviews.
    fn remove_frame_chrome(&self) {
        let ivars = self.ivars();
        if let Some(label) = ivars.frame_label.borrow_mut().take() {
            label.removeFromSuperview();
        }
        if let Some(ok) = ivars.frame_ok.borrow_mut().take() {
            ok.removeFromSuperview();
        }
    }

    /// Resize the window to fit the current shot's frame into the screen.
    fn refit_window(&self) {
        let Some(window) = self.window() else {
            return;
        };
        let (iw, ih) = self.image_size();
        if iw == 0 || ih == 0 {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let content_size = NSScreen::mainScreen(mtm)
            .map(|screen| screen.visibleFrame().size)
            .map(|visible| {
                fit_size(iw as f64, ih as f64, visible.width, visible.height - TOOLBAR_HEIGHT)
            })
            .unwrap_or_else(|| CGSize::new(iw as f64, ih as f64));
        window.setContentSize(CGSize::new(
            content_size.width,
            content_size.height + TOOLBAR_HEIGHT,
        ));
        window.center();
        self.redraw();
    }

    /// Show (or replace) the shot in the window: title, size, full redraw.
    fn display_shot(&self, shot: OpenShot) {
        let title = shot.file_name();
        // Any in-flight interaction belongs to the outgoing shot.
        self.discard_text_editor();
        self.exit_frame_edit();
        *self.ivars().shot.borrow_mut() = Some(shot);
        self.ivars().selection.set(None);
        if let Some(window) = self.window() {
            window.setTitle(&NSString::from_str(&title));
        }
        self.refit_window();
    }

    /// Show the previous (-1) or next (1) shot of the save folder by time;
    /// the edges stop the leafing. Pending text edits are committed so
    /// they land in the outgoing shot's file.
    fn page(&self, delta: isize) {
        let ivars = self.ivars();
        self.commit_text_editor();
        self.exit_frame_edit();
        let current = ivars.shot.borrow().as_ref().map(|shot| shot.shot_path.clone());
        let Some(current) = current else {
            return;
        };
        let shots = crate::output::sorted_shots(&ivars.save_dir.borrow());
        let Some(index) = shots.iter().position(|path| *path == current) else {
            return;
        };
        let target = (index as isize + delta).clamp(0, shots.len() as isize - 1);
        if target == index as isize {
            return;
        }
        let sessions_root = crate::editor::session::default_root();
        match OpenShot::open(shots[target as usize].clone(), &sessions_root) {
            Some(shot) => self.display_shot(shot),
            None => eprintln!("editor: cannot open the paged shot"),
        }
    }

    /// Accent-tint the active tool's button, clear the others.
    fn update_tool_highlight(&self) {
        let active = self.ivars().tool.get().map(|tool| tool.tag());
        for (tag, button) in self.ivars().tool_buttons.borrow().iter() {
            if Some(*tag) == active {
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
        // Take the slot first: performClose re-enters popoverDidClose,
        // which borrows the same cell. A stale closed popover (closed
        // outside this toggle) is simply replaced below.
        let open = self.ivars().color_popover.borrow_mut().take();
        if let Some(popover) = open.as_ref() {
            if popover.isShown() {
                // SAFETY: plain close on a live, shown popover.
                unsafe { popover.performClose(None) };
                return;
            }
        }
        let Some(anchor) = sender else { return };
        let Some(anchor_button) = anchor.downcast_ref::<NSButton>() else {
            return;
        };
        let mtm = MainThreadMarker::new().expect("popover on main thread");
        // SAFETY: plain init on a freshly allocated popover.
        let popover: Retained<NSPopover> = unsafe { msg_send![NSPopover::alloc(mtm), init] };
        // Closes only where the code closes it (a preset pick or a second
        // click on the swatch button), so the toggle stays deterministic.
        popover.setBehavior(objc2_app_kit::NSPopoverBehavior::ApplicationDefined);
        // SAFETY: the delegate is this canvas view, alive for the process.
        unsafe { popover.setDelegate(Some(ProtocolObject::from_ref(self))) };

        let content = NSStackView::new(mtm);
        content.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        content.setSpacing(6.0);
        // Edge insets keep the preset outlines inside the popover: without
        // them the border of the first and last swatch is clipped.
        content.setEdgeInsets(objc2_foundation::NSEdgeInsets {
            top: 8.0,
            left: 8.0,
            bottom: 8.0,
            right: 8.0,
        });
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
                // A visible outline: light presets (white) would blend into
                // the popover background.
                layer.setBorderWidth(1.0);
                layer.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
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
        *self.ivars().color_popover.borrow_mut() = Some(popover);
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

/// The crop/resize frame rect while dragging `spot` from the original rect
/// `orig`, with the drag start at `start` and the cursor at `cur`.
fn frame_rect(orig: Rect, spot: FrameSpot, start: Point, cur: Point) -> Rect {
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
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(
        15.0,
        // SAFETY: NSFontWeightRegular is a plain extern CGFloat constant.
        unsafe { NSFontWeightRegular },
    );
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
    pub fn new(mtm: MainThreadMarker, save_dir: std::path::PathBuf) -> EditorWindow {
        let view = EditorView::new(mtm);
        view.set_save_dir(save_dir);
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

        // Toolbar: ◄/► leafing on the left, then tool buttons, the color
        // swatch with a preset popover, and the stroke-width slider.
        let toolbar = NSStackView::new(mtm);
        toolbar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        toolbar.setSpacing(6.0);
        for (tag, symbol) in [
            (PAGE_PREV_TAG, "chevron.left"),
            (PAGE_NEXT_TAG, "chevron.right"),
        ] {
            let button = symbol_button(mtm, symbol, tag);
            // SAFETY: target is this canvas view, alive for the process.
            unsafe {
                button.setTarget(Some(&view));
                button.setAction(Some(sel!(pageClicked:)));
            }
            toolbar.addArrangedSubview(&button);
        }
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

        // Layout: toolbar strip at the top, canvas below it. The flexible
        // min-Y margin keeps the toolbar pinned to the top edge when the
        // window resizes; the canvas keeps its rigid 40-pt top gap and
        // grows into the freed space.
        let content = window.contentView().expect("content view");
        let size = content.bounds().size;
        toolbar.setFrame(CGRect::new(
            CGPoint::new(8.0, size.height - TOOLBAR_HEIGHT),
            CGSize::new(size.width - 16.0, TOOLBAR_HEIGHT),
        ));
        toolbar.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
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

    /// Show (or replace) the shot in this window and bring it to the front.
    pub fn show_shot(&self, shot: OpenShot) {
        self.view.display_shot(shot);
        self.window.makeKeyAndOrderFront(None);
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

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::ClassType;

    #[test]
    fn class_registration_succeeds() {
        // objc2 debug builds verify protocol overrides during class
        // registration, which is exactly what panicked when the
        // end-editing callback sat on the sub-protocol.
        let _ = EditorView::class();
    }
}
