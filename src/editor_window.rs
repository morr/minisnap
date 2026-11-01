//! The editor window: one reusable `NSWindow` showing the open shot — the
//! base frame plus annotations, drawn with CoreGraphics into a custom
//! `NSView`, with an annotation toolbar in the left column. Selection
//! markers are drawn on top in the canvas and never become part of a saved
//! composite.

use std::cell::{Cell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    define_class, extern_methods, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker,
    MainThreadOnly,
};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSAutoresizingMaskOptions, NSBackingStoreType, NSBezierPath, NSButton,
    NSColor, NSControl, NSControlTextEditingDelegate, NSCursor, NSEvent, NSEventMask,
    NSEventModifierFlags, NSEventTrackingRunLoopMode, NSEventType, NSFocusRingType, NSFont,
    NSGraphicsContext, NSImage, NSScreen, NSTextField, NSTextFieldDelegate, NSTextView,
    NSView, NSWindow, NSWindowTitleVisibility, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreateImage, CGImage, CGContext};
use objc2_foundation::{NSDate, NSString, NSNotification};

use crate::editor::drag::{
    callout_anchor, draft_is_real, frame_rect, point_dist, spot_at, spot_points, Drag, FrameEdit,
    FrameSpot, PendingCallout, MIN_DRAG_PX,
};
use crate::editor::hit_test::{Hit, text_bounds, top_hit};
use crate::editor::text;
use crate::editor::{
    clamp_crop_rect, name_density, render, set_endpoint, shift_element, Command, Element,
    ElementKind, OpenShot, Point, Rect, Rgba, Size,
};
use crate::editor_toolbar::{
    arrow_tuner_panel, stroke_width_of, ARROW_PARAM_SPECS, COLOR_PRESETS, DEFAULT_STROKE_WIDTH,
    STROKE_MAX, STROKE_MIN, ToolbarEvent, ToolbarState, ToolbarView, Tool, TOOLBAR_HEIGHT,
};

/// Checkerboard cell of the transparent-backdrop fill, in points: the
/// light checker image editors draw behind transparent areas.
const CHECKER_CELL_PT: f64 = 8.0;


/// Selection marker radius, in view points (constant on screen).
const MARKER_RADIUS_PT: f64 = 4.5;

/// Background of the whole window and the toolbar column: the flat light
/// gray (#F2F2F2) of the zcode chrome — no vibrancy anywhere.
const WINDOW_BG: [f64; 4] = [242.0 / 255.0, 242.0 / 255.0, 242.0 / 255.0, 1.0];

/// Gap between the image card and the window's left, right and bottom
/// edges, in points; the card touches the toolbar strip on top, so the
/// air around the buttons stays even.
const CARD_GAP: f64 = 4.0;

/// The image card: a rounded panel a step lighter than the window, the
/// "window in a window" the reference design reads as.
const CARD_BG: [f64; 4] = [248.0 / 255.0, 248.0 / 255.0, 248.0 / 255.0, 1.0];

/// Border of the image card: one point of light gray marking the card
/// against the window background.
const CARD_BORDER: [f64; 4] = [224.0 / 255.0, 224.0 / 255.0, 224.0 / 255.0, 1.0];

/// Corner radius of the image card, in points.
const CARD_RADIUS: f64 = 8.0;

/// Initial canvas size before the first shot arrives.
const INITIAL_SIZE: CGSize = CGSize::new(480.0, 320.0);

/// Font size of new text and callout elements, in image pixels.
const DEFAULT_TEXT_SIZE_PX: f64 = 32.0;

/// Minimum width of the callout field editor, in view points: only wide
/// enough for the caret, so an empty field never reaches over the arrow.
const CALLOUT_EDITOR_MIN_WIDTH_PT: f64 = 30.0;

/// Width of the field editor for new (empty) text, in view points.
const TEXT_EDITOR_WIDTH_PT: f64 = 200.0;

/// Radius of the crop/resize frame handles, in view points.
const HANDLE_RADIUS_PT: f64 = 5.0;

/// Size of the frame mode's OK button, in view points.
const FRAME_OK_SIZE: CGSize = CGSize::new(52.0, 26.0);

/// What the open field editor is editing.
#[derive(Clone, Copy)]
enum TextTarget {
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

define_class!(
    /// The editor's window. A titled window accepts key status by default;
    /// the `canBecomeKeyWindow` override is redundant for it and is kept
    /// only as harmless insurance — the canvas lives on keyboard commands
    /// (Esc, ⌘Q, ⌘C, ⌘Z, ←/→), so the window must accept key.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    pub struct KeyableWindow;

    unsafe impl NSObjectProtocol for KeyableWindow {}

    impl KeyableWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn __can_become_key_window(&self) -> bool {
            true
        }

        // The system hosts the traffic lights and calls these window methods
        // on a real click (verified with CGEvent clicks on macOS 26, change
        // editor-system-buttons). The miniaturize button sends
        // `miniaturize:`, not `performMiniaturize:`; an accessory app has no
        // dock to miniaturize into, so both paths hide the window and the
        // next `show_shot` brings it back. Green stays native zoom: the
        // window is `Resizable`, while `toggleFullScreen:` is a no-op
        // without the `FullScreen` style bit, which would break the system
        // corner rounding.
        #[unsafe(method(miniaturize:))]
        fn __miniaturize(&self, sender: Option<&AnyObject>) {
            self.orderOut(sender);
        }

        #[unsafe(method(performMiniaturize:))]
        fn __perform_miniaturize(&self, sender: Option<&AnyObject>) {
            self.orderOut(sender);
        }
    }
);

impl KeyableWindow {
    extern_methods!(
        #[unsafe(method(initWithContentRect:styleMask:backing:defer:))]
        unsafe fn init_with_content_rect(
            this: objc2::rc::Allocated<Self>,
            content_rect: CGRect,
            style: NSWindowStyleMask,
            backing: NSBackingStoreType,
            defer: bool,
        ) -> Retained<Self>;
    );
}

pub struct EditorWindow {
    pub window: Retained<KeyableWindow>,
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
    /// The callout awaiting its text (arrow drawn, editor open).
    pending_callout: RefCell<Option<PendingCallout>>,
    /// The frame mode's size label and OK button subviews.
    frame_label: RefCell<Option<Retained<NSTextField>>>,
    frame_ok: RefCell<Option<Retained<NSButton>>>,
    /// The folder the shots are saved to (for the ◄/► leafing).
    save_dir: RefCell<std::path::PathBuf>,
    /// The toolbar strip: owns the row, the hover pass and the chrome; the
    /// window talks to it only through its interface.
    toolbar: RefCell<Option<Retained<ToolbarView>>>,
    /// Pixel density of the open shot's display (from the file name's
    /// `@<n>x`): the shot is shown at 1/n, the size it had on screen.
    density: Cell<f64>,
    /// Displayed frame size, in view points: the shot at its display
    /// scale, centered in the canvas by `frame_rect`.
    frame_size: Cell<CGSize>,
    /// Cached checkerboard pattern color for the backdrop.
    checker: RefCell<Option<Retained<NSColor>>>,
    /// Whether the window has been placed already: the first refit centers
    /// it, later refits keep the top-left corner.
    placed: Cell<bool>,
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
                // The editor opens in viewing mode: no tool is picked, so a
                // plain drag moves the window (the tool is one click away).
                tool: Cell::new(None),
                color: Cell::new(COLOR_PRESETS[0]),
                stroke_width: Cell::new(DEFAULT_STROKE_WIDTH),
                drag: RefCell::new(None),
                text_editor: RefCell::new(None),
                text_target: Cell::new(None),
                frame_edit: RefCell::new(None),
                pending_callout: RefCell::new(None),
                frame_label: RefCell::new(None),
                frame_ok: RefCell::new(None),
                save_dir: RefCell::new(std::path::PathBuf::new()),
                toolbar: RefCell::new(None),
                density: Cell::new(1.0),
                frame_size: Cell::new(CGSize::ZERO),
                checker: RefCell::new(None),
                placed: Cell::new(false),
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
                // ⌘Q/⌘W close the viewer; the app stays in the tray.
                12 | 13 if event.modifierFlags().contains(NSEventModifierFlags::Command) => {
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
    }

    // SAFETY: only the end-editing and key-command callbacks are
    // implemented; the canvas view outlives the field editor it parents.
    // The methods belong on this protocol itself: objc2's debug build
    // rejects registering them via the sub-protocol NSTextFieldDelegate,
    // which only inherits it.
    unsafe impl NSControlTextEditingDelegate for EditorView {
        #[unsafe(method(controlTextDidEndEditing:))]
        fn __control_text_did_end_editing(&self, _notification: &NSNotification) {
            self.commit_text_editor();
        }

        #[unsafe(method(controlTextDidChange:))]
        fn __control_text_did_change(&self, _notification: &NSNotification) {
            // The callout editor follows its growing text so it never runs
            // over the arrow; other editors are static until they commit.
            self.refit_callout_editor();
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        unsafe fn __control_text_view_do_command(
            &self,
            _control: &NSControl,
            _text_view: &NSTextView,
            command: objc2::runtime::Sel,
        ) -> objc2::runtime::Bool {
            // Enter commits; Shift+Enter keeps the newline; Esc cancels the
            // input. Plain controlTextDidEndEditing stays the fallback for
            // clicks elsewhere — on Enter in this window it does not fire.
            if command == sel!(insertNewline:) {
                let shift = NSApplication::sharedApplication(
                    MainThreadMarker::new().expect("text editing on the main thread"),
                )
                .currentEvent()
                .is_some_and(|event| {
                    event.modifierFlags().contains(NSEventModifierFlags::Shift)
                });
                if shift {
                    return false.into();
                }
                self.commit_text_editor();
                return true.into();
            }
            if command == sel!(cancelOperation:) {
                self.discard_text_editor();
                return true.into();
            }
            false.into()
        }
    }

    // SAFETY: empty — NSTextField's delegate property is typed
    // id<NSTextFieldDelegate>, so the sub-protocol conformance is what the
    // ProtocolObject cast in the field editor needs.
    unsafe impl NSTextFieldDelegate for EditorView {}
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

    /// The image card area: the canvas inset by the card gap on the
    /// sides and the bottom (origin Y grows upward — the gap sits at the
    /// window's bottom edge), touching the toolbar strip on top. The
    /// card's own paint (fill, border, rounding) lives in `draw`; this is
    /// the geometry the frame centers in.
    fn card_rect(&self) -> CGRect {
        let bounds = self.bounds();
        CGRect::new(
            CGPoint::new(CARD_GAP, CARD_GAP),
            CGSize::new(
                (bounds.size.width - 2.0 * CARD_GAP).max(0.0),
                (bounds.size.height - CARD_GAP).max(0.0),
            ),
        )
    }

    /// The shot's frame centered in the card, in view points.
    fn frame_rect(&self) -> CGRect {
        let card = self.card_rect();
        let size = self.ivars().frame_size.get();
        CGRect::new(
            CGPoint::new(
                card.origin.x + (card.size.width - size.width) / 2.0,
                card.origin.y + (card.size.height - size.height) / 2.0,
            ),
            size,
        )
    }

    /// View-space position of an image-pixel point (top-left origin).
    fn pixel_to_view(&self, p: Point) -> CGPoint {
        let frame = self.frame_rect();
        let (sx, sy) = self.view_scale();
        CGPoint::new(
            frame.origin.x + p.x * sx,
            frame.origin.y + frame.size.height - p.y * sy,
        )
    }

    /// Cursor position in image pixel space (top-left origin): view points
    /// scale by the current frame-to-pixel ratio and flip vertically.
    fn pixel_point(&self, event: &NSEvent) -> Point {
        let location = event.locationInWindow();
        let local = self.convertPoint_fromView(location, None);
        let frame = self.frame_rect();
        let (iw, ih) = self.image_size();
        if frame.size.width <= 0.0 || frame.size.height <= 0.0 || iw == 0 || ih == 0 {
            return Point::new(0.0, 0.0);
        }
        // Clamp into the frame: drags that leave the image stick to its
        // edge instead of drawing outside it.
        Point::new(
            ((local.x - frame.origin.x) * iw as f64 / frame.size.width).clamp(0.0, iw as f64),
            ((frame.origin.y + frame.size.height - local.y) * ih as f64 / frame.size.height)
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

        // The card: a rounded panel a step lighter than the window — the
        // "window in a window" the reference reads as. The checker and
        // the shot live inside its border; the canvas paints nothing
        // else, so the window background and the system corner rounding
        // stay untouched.
        let card = self.card_rect();
        CGContext::save_g_state(Some(&cg));
        // SAFETY: class convenience constructor of a bezier path; the
        // drawing calls that follow touch only the current graphics
        // context.
        let card_path = unsafe {
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(card, CARD_RADIUS, CARD_RADIUS)
        };
        let card_bg = NSColor::colorWithSRGBRed_green_blue_alpha(
            CARD_BG[0],
            CARD_BG[1],
            CARD_BG[2],
            CARD_BG[3],
        );
        card_bg.setFill();
        card_path.fill();
        let card_border = NSColor::colorWithSRGBRed_green_blue_alpha(
            CARD_BORDER[0],
            CARD_BORDER[1],
            CARD_BORDER[2],
            CARD_BORDER[3],
        );
        card_border.setStroke();
        card_path.setLineWidth(1.0);
        card_path.stroke();
        card_path.addClip();
        // Backdrop: the light checkerboard image editors draw behind
        // transparent areas — the frame floats on it, centered in the
        // card.
        let mut cache = ivars.checker.borrow_mut();
        let checker = cache.get_or_insert_with(checker_color);
        checker.setFill();
        CGContext::fill_rect(Some(&cg), card);
        drop(cache);

        let Some(shot) = shot_guard.as_ref() else {
            CGContext::restore_g_state(Some(&cg));
            return;
        };
        let base = shot.doc.base();
        let (iw, ih) = self.image_size();
        if iw == 0 || ih == 0 {
            return;
        }
        let frame = self.frame_rect();
        let (sx, sy) = (frame.size.width / iw as f64, frame.size.height / ih as f64);

        // The composite content: base frame + elements, drawn in image
        // pixel units (`draw_elements` applies its own Y flip) into the
        // frame rect. A move or endpoint drag hides its element and shows
        // a transformed preview; an element under in-place text editing is
        // hidden entirely.
        CGContext::save_g_state(Some(&cg));
        CGContext::translate_ctm(Some(&cg), frame.origin.x, frame.origin.y);
        CGContext::scale_ctm(Some(&cg), sx, sy);
        CGContext::clip_to_rect(
            Some(&cg),
            CGRect::new(CGPoint::ZERO, CGSize::new(iw as f64, ih as f64)),
        );
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
        // The pending callout (arrow drawn, text being entered) previews on
        // top until the text commits or the input is cancelled.
        if let Some(pending) = ivars.pending_callout.borrow().as_ref() {
            let pending_el = Element {
                id: 0,
                kind: ElementKind::Callout {
                    tip: pending.tip,
                    anchor: pending.base,
                    text: String::new(),
                    color: pending.color,
                    size: pending.size,
                    width: pending.width,
                },
            };
            render::draw_elements(&cg, base, ih as f64, std::slice::from_ref(&pending_el));
        }
        CGContext::restore_g_state(Some(&cg));

        // The crop/resize frame overlay: the dim, the border and the
        // handles stay inside the card clip.
        if ivars.frame_edit.borrow().is_some() {
            self.draw_frame_overlay(&cg, &card, sx, sy);
        }
        CGContext::restore_g_state(Some(&cg));

        // Selection markers on top, in screen points so they stay constant
        // sized; they never reach a saved composite.
        if let Some(id) = ivars.selection.get() {
            self.draw_markers(&cg, shot, id);
        }
    }

    /// The crop/resize frame: dimmed frame surroundings, white border,
    /// nine handle dots — all within the card.
    fn draw_frame_overlay(&self, cg: &CGContext, card: &CGRect, sx: f64, sy: f64) {
        let Some(rect) = self.current_frame_rect() else {
            return;
        };
        let frame = self.frame_rect();
        let vx = frame.origin.x + rect.x * sx;
        let vw = rect.width * sx;
        let vh = rect.height * sy;
        let y0 = frame.origin.y + frame.size.height - rect.max_y() * sy;
        // Dim everything outside the frame, inside the card.
        CGContext::set_rgb_fill_color(Some(cg), 0.0, 0.0, 0.0, 0.45);
        let dim = [
            CGRect::new(
                CGPoint::new(card.origin.x, card.origin.y),
                CGSize::new(card.size.width, (y0 - card.origin.y).max(0.0)),
            ),
            CGRect::new(
                CGPoint::new(card.origin.x, y0 + vh),
                CGSize::new(
                    card.size.width,
                    (card.origin.y + card.size.height - y0 - vh).max(0.0),
                ),
            ),
            CGRect::new(
                CGPoint::new(card.origin.x, y0),
                CGSize::new((vx - card.origin.x).max(0.0), vh),
            ),
            CGRect::new(
                CGPoint::new(vx + vw, y0),
                CGSize::new(
                    (card.origin.x + card.size.width - vx - vw).max(0.0),
                    vh,
                ),
            ),
        ];
        for r in dim {
            CGContext::fill_rect(Some(cg), r);
        }
        // Frame border.
        CGContext::set_rgb_stroke_color(Some(cg), 1.0, 1.0, 1.0, 0.9);
        CGContext::set_line_width(Some(cg), 1.5);
        CGContext::stroke_rect(Some(cg), CGRect::new(CGPoint::new(vx, y0), CGSize::new(vw, vh)));
        // Handle dots: white fill with the accent ring. A dot sitting on
        // the window edge is pressed inward just enough to stay fully
        // visible; its hit zone stays at the true spot.
        let accent: Retained<NSColor> = NSColor::controlAccentColor();
        let accent_cg = accent.CGColor();
        for (_, point) in self.frame_spot_points() {
            let r = HANDLE_RADIUS_PT;
            let cx = (frame.origin.x + point.x * sx).clamp(
                card.origin.x + r,
                (card.origin.x + card.size.width - r).max(card.origin.x + r),
            );
            let cy = (frame.origin.y + frame.size.height - point.y * sy).clamp(
                card.origin.y + r,
                (card.origin.y + card.size.height - r).max(card.origin.y + r),
            );
            let marker = CGRect::new(
                CGPoint::new(cx - r, cy - r),
                CGSize::new(2.0 * r, 2.0 * r),
            );
            // A soft shadow keeps the dots readable over light frames.
            let shadow = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.35);
            let shadow_cg = shadow.CGColor();
            CGContext::set_shadow_with_color(
                Some(cg),
                CGSize::new(0.0, 1.0),
                2.0,
                Some(&shadow_cg),
            );
            CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
            CGContext::fill_ellipse_in_rect(Some(cg), marker);
            CGContext::set_shadow_with_color(Some(cg), CGSize::ZERO, 0.0, None);
            CGContext::set_stroke_color_with_color(Some(cg), Some(&accent_cg));
            CGContext::set_line_width(Some(cg), 1.5);
            CGContext::stroke_ellipse_in_rect(Some(cg), marker);
        }
    }

    /// Handles of the selected element: endpoint dots and bounding-box
    /// corners, white fill with the system accent ring.
    fn draw_markers(&self, cg: &CGContext, shot: &OpenShot, id: u64) {
        let Some(element) = shot.doc.element(id) else {
            return;
        };
        let to_point = |p: Point| -> CGPoint { self.pixel_to_view(p) };
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
            // A soft shadow keeps the markers readable over light frames.
            let shadow = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.35);
            let shadow_cg = shadow.CGColor();
            CGContext::set_shadow_with_color(
                Some(cg),
                CGSize::new(0.0, 1.0),
                2.0,
                Some(&shadow_cg),
            );
            CGContext::set_rgb_fill_color(Some(cg), 1.0, 1.0, 1.0, 1.0);
            CGContext::fill_ellipse_in_rect(Some(cg), marker_rect);
            CGContext::set_shadow_with_color(Some(cg), CGSize::ZERO, 0.0, None);
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
        let tool = ivars.tool.get();
        let draft_kind: Option<ElementKind> = match tool {
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
            // The callout draws its arrow by drag, from the base to the
            // tip, exactly like the arrow tool (same color, same stroke
            // width); the text follows in the second phase, at mouse-up.
            Some(Tool::Callout) => Some(ElementKind::Callout {
                tip: point,
                anchor: point,
                text: String::new(),
                color: ivars.color.get(),
                size: DEFAULT_TEXT_SIZE_PX,
                width: ivars.stroke_width.get(),
            }),
            // The text tool starts editing on the click (below).
            Some(Tool::Text) => None,
            // Crop/resize drag the frame's handles; the click never draws
            // or selects elements. The crop frame also drags from any
            // point (moving whole); the resize frame does not move.
            Some(Tool::Crop) | Some(Tool::Resize) => {
                let resize = ivars.tool.get() == Some(Tool::Resize);
                let spot = match self.frame_spot_at(point) {
                    Some((spot, _)) => Some(spot),
                    None if !resize => Some(FrameSpot::Center),
                    None => None,
                };
                if let Some(spot) = spot {
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
        // A hit on an existing element decides between moving it, dragging
        // one of its ends, and editing its text; the empty frame draws a
        // new element (or starts a text).
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
                // A click on editable text edits it in place; a click on a
                // callout's arrow moves the whole callout (text follows).
                if self.hit_edits_text(id, point) {
                    self.begin_text_edit(TextTarget::Existing { id });
                    return;
                }
                if tool == Some(Tool::Text) && !self.id_is_textlike(id) {
                    // The text tool starts a new text over any other element.
                    self.begin_text_edit(TextTarget::New {
                        origin: point,
                        callout: false,
                        color: ivars.color.get(),
                    });
                    return;
                }
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
                // a plain click here is the deselect rule. The text tool
                // opens its editor instead, the callout needs a drag.
                ivars.selection.set(None);
                match (tool, draft_kind) {
                    (Some(Tool::Text), _) => {
                        self.begin_text_edit(TextTarget::New {
                            origin: point,
                            callout: false,
                            color: ivars.color.get(),
                        });
                        return;
                    }
                    (_, Some(kind)) => Drag::Draw(kind),
                    // Viewing mode over free image space: the drag moves
                    // the window. The frame handles, the text field and the
                    // elements have already claimed their clicks above.
                    (None, None) => {
                        self.redraw();
                        self.drag_window_with_mouse();
                        return;
                    }
                    // A tool is up but produced no draft (cannot happen
                    // today): a plain deselect, the window stays put.
                    _ => {
                        self.redraw();
                        return;
                    }
                }
            }
        };
        *ivars.drag.borrow_mut() = Some(drag);
        self.redraw();
    }

    /// Move the window with the mouse after a viewing-mode press. The
    /// window is `setMovable(false)` — the toolbar strip must not drag it —
    /// and `performWindowDragWithEvent` is a no-op for presses the system
    /// does not count as titlebar ones, so the canvas tracks the drag
    /// itself: step the frame by the global mouse delta until mouse-up.
    fn drag_window_with_mouse(&self) {
        let Some(window) = self.window() else {
            return;
        };
        let frame = window.frame();
        let start = NSEvent::mouseLocation();
        loop {
            let Some(next) = window.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::LeftMouseUp | NSEventMask::LeftMouseDragged,
                Some(&NSDate::distantFuture()),
                unsafe { NSEventTrackingRunLoopMode },
                true,
            ) else {
                break;
            };
            match next.r#type() {
                NSEventType::LeftMouseDragged => {
                    let now = NSEvent::mouseLocation();
                    window.setFrameOrigin(CGPoint::new(
                        frame.origin.x + (now.x - start.x),
                        frame.origin.y + (now.y - start.y),
                    ));
                }
                NSEventType::LeftMouseUp => break,
                _ => {}
            }
        }
        window.discardEventsMatchingMask_beforeEvent(
            NSEventMask::LeftMouseUp | NSEventMask::LeftMouseDragged | NSEventMask::MouseMoved,
            None,
        );
    }

    /// Whether a Body hit at `point` is a click on editable text: a
    /// callout's text block (any tool) or a text element (text tools only).
    fn hit_edits_text(&self, id: u64, point: Point) -> bool {
        let ivars = self.ivars();
        let kind = ivars
            .shot
            .borrow()
            .as_ref()
            .and_then(|shot| shot.doc.element(id))
            .map(|el| el.kind.clone());
        match kind {
            Some(ElementKind::Callout { anchor, text, size, .. }) => {
                text_bounds(anchor, &text, size).is_some_and(|b| b.contains(point))
            }
            Some(ElementKind::Text { .. }) => {
                matches!(ivars.tool.get(), Some(Tool::Text) | Some(Tool::Callout))
            }
            _ => false,
        }
    }

    /// Whether the element is a text or callout element.
    fn id_is_textlike(&self, id: u64) -> bool {
        self.ivars()
            .shot
            .borrow()
            .as_ref()
            .and_then(|shot| shot.doc.element(id))
            .is_some_and(|el| {
                matches!(el.kind, ElementKind::Text { .. } | ElementKind::Callout { .. })
            })
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
                if !draft_is_real(&kind) {
                    // Plain click: the draft is discarded and the selection
                    // was already cleared at mouse-down.
                    self.redraw();
                    return;
                }
                // A callout splits into two phases: the drawn arrow stays
                // up as a pending preview while the text is entered at its
                // base; the element lands (or is dropped) with the text.
                if let ElementKind::Callout { anchor, tip, color, size, width, text: _ } = kind {
                    *ivars.pending_callout.borrow_mut() = Some(PendingCallout {
                        base: anchor,
                        tip,
                        color,
                        size,
                        width,
                    });
                    self.begin_text_edit(TextTarget::New {
                        origin: anchor,
                        callout: true,
                        color,
                    });
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
                // The fresh element stays unselected: drawing must not end
                // in markers and a snapped slider — a click selects it.
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
                    // The callout's tip follows the cursor; the anchor (its
                    // base) stays at the drag start.
                    ElementKind::Callout { tip, .. } => *tip = point,
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

    /// Schedule a full redraw of the canvas.
    fn redraw(&self) {
        // The redraw follows every state change a selection can ride on
        // (mouse-up commits, undo/redo), so it doubles as the hook that
        // keeps the toolbar in step with the selection (the slider handle
        // rides the selection's width).
        let state = self.toolbar_state();
        if let Some(toolbar) = self.toolbar() {
            toolbar.set_state(state);
        }
        self.setNeedsDisplayInRect(self.bounds());
    }

    /// Snapshot the toolbar state from the live document and ivars through
    /// the pure reduction. A selected stroke-bearing element pins the
    /// slider — and, as ever, the width for new elements — to its own.
    fn toolbar_state(&self) -> ToolbarState {
        let ivars = self.ivars();
        let shot = ivars.shot.borrow();
        let state = crate::editor_toolbar::toolbar_state(
            shot.as_ref().map(|shot| &shot.doc),
            shot.as_ref()
                .map(|shot| shot.file_name())
                .unwrap_or_default(),
            ivars.tool.get(),
            ivars.stroke_width.get(),
            ivars.color.get(),
            ivars.selection.get(),
        );
        if state.stroke_width != ivars.stroke_width.get() {
            // The slider pinned to a stroked selection: its width becomes
            // the width for new elements, like every redraw has done.
            ivars.stroke_width.set(state.stroke_width);
        }
        state
    }

    /// Scale factors from image pixels to view points.
    fn view_scale(&self) -> (f64, f64) {
        let frame = self.frame_rect();
        let (iw, ih) = self.image_size();
        if iw == 0 || ih == 0 || frame.size.width <= 0.0 || frame.size.height <= 0.0 {
            return (1.0, 1.0);
        }
        (frame.size.width / iw as f64, frame.size.height / ih as f64)
    }

    /// View-space size of the field editor for a text block holding `text`
    /// (may be empty) at `size` image pixels. A callout editor never pads to
    /// the text tool's 200 pt minimum: it stays as wide as its text, so the
    /// side-of-the-arrow placement holds for the field too.
    fn text_editor_size(&self, text: &str, size: f64, callout: bool) -> (f64, f64) {
        let (_, sy) = self.view_scale();
        let font_pt = (size * sy).max(9.0);
        let min_pt = if callout {
            CALLOUT_EDITOR_MIN_WIDTH_PT
        } else {
            TEXT_EDITOR_WIDTH_PT
        };
        if text.is_empty() {
            (min_pt, font_pt * 1.6 + 6.0)
        } else {
            match text::measure_block(text, size) {
                Some(m) => (
                    (m.width * self.view_scale().0).max(min_pt) + 12.0,
                    m.height * sy + 6.0,
                ),
                None => (min_pt, font_pt * 1.6 + 6.0),
            }
        }
    }

    /// View-space rect for the field editor over a text block whose
    /// top-left sits at `origin` (image pixels), holding `text` (may be
    /// empty) at `size` image pixels.
    fn text_editor_frame(&self, origin: Point, text: &str, size: f64, callout: bool) -> CGRect {
        let frame = self.frame_rect();
        let (sx, sy) = self.view_scale();
        let (width, height) = self.text_editor_size(text, size, callout);
        let x = frame.origin.x + origin.x * sx - 3.0;
        let y_top = frame.origin.y + frame.size.height - origin.y * sy;
        CGRect::new(CGPoint::new(x, y_top - height), CGSize::new(width, height))
    }

    /// The base image's pixel rect — the space every element coordinate and
    /// every frame clamp lives in.
    fn image_frame(&self) -> Option<Rect> {
        let shot = self.ivars().shot.borrow();
        let base = shot.as_ref()?.doc.base();
        Some(Rect::new(
            0.0,
            0.0,
            CGImage::width(Some(base)) as f64,
            CGImage::height(Some(base)) as f64,
        ))
    }

    /// Where the pending callout's text block — or its field editor — sits
    /// (in image pixels): `callout_anchor` against the pending arrow, with
    /// `block` already in image pixels.
    fn pending_callout_anchor(&self, block: Size) -> Option<Point> {
        let pending = self.ivars().pending_callout.borrow();
        let pending = pending.as_ref()?;
        let frame = self.image_frame()?;
        Some(callout_anchor(
            pending.base,
            pending.tip,
            block.width,
            block.height,
            frame,
        ))
    }

    /// Open the field editor for `target`: a transparent text field over
    /// the text block. Enter commits (via `doCommandBy`), a click elsewhere
    /// commits through `controlTextDidEndEditing`, Esc cancels.
    fn begin_text_edit(&self, target: TextTarget) {
        let ivars = self.ivars();
        // Never two field editors: an open one commits first.
        self.commit_text_editor();
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let (initial, frame, color, size_px) = match target {
            TextTarget::New {
                origin,
                callout,
                color,
            } => {
                let size = DEFAULT_TEXT_SIZE_PX;
                // A new callout opens its field exactly where the text will
                // land: beside the pending arrow's base, away from the tip.
                let anchor = if callout {
                    let (w_pt, h_pt) = self.text_editor_size("", size, true);
                    let (sx, sy) = self.view_scale();
                    self.pending_callout_anchor(Size::new(w_pt / sx, h_pt / sy))
                        .unwrap_or(origin)
                } else {
                    origin
                };
                (
                    String::new(),
                    self.text_editor_frame(anchor, "", size, true),
                    color,
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
                    ElementKind::Text { origin, text, color, size } => (
                        text.clone(),
                        self.text_editor_frame(*origin, text, *size, false),
                        *color,
                        *size,
                    ),
                    ElementKind::Callout { anchor: origin, text, color, size, .. } => (
                        text.clone(),
                        self.text_editor_frame(*origin, text, *size, true),
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
        // Shift+Enter may insert a line separator (U+2028) instead of a
        // newline; normalize it to the separator the renderer splits on.
        let text = field.stringValue().to_string().replace('\u{2028}', "\n");
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
            TextTarget::New {
                origin,
                callout,
                color,
            } => {
                if text.trim().is_empty() {
                    // An empty commit (and a cancelled input) drops a
                    // pending callout and adds nothing.
                    *ivars.pending_callout.borrow_mut() = None;
                    self.redraw();
                    return;
                }
                let size = DEFAULT_TEXT_SIZE_PX;
                let kind = if callout {
                    let (tip, base, width) = match ivars.pending_callout.borrow_mut().take() {
                        Some(pending) => (pending.tip, pending.base, pending.width),
                        None => (origin, origin, ivars.stroke_width.get()),
                    };
                    let base_image = shot.doc.base();
                    let frame = Rect::new(
                        0.0,
                        0.0,
                        CGImage::width(Some(base_image)) as f64,
                        CGImage::height(Some(base_image)) as f64,
                    );
                    let (w, h) = text::measure_block(&text, size)
                        .map(|m| (m.width, m.height))
                        .unwrap_or((size * 4.0, size * 1.25));
                    ElementKind::Callout {
                        tip,
                        anchor: callout_anchor(base, tip, w, h, frame),
                        text,
                        color,
                        size,
                        width,
                    }
                } else {
                    ElementKind::Text {
                        origin,
                        text,
                        color,
                        size,
                    }
                };
                let id = shot.doc.next_element_id();
                // Same rule as drawn shapes: the new element stays
                // unselected until the user clicks it.
                if let Err(err) = shot.apply(Command::AddElement(Element { id, kind })) {
                    eprintln!("editor: failed to add the text element: {err}");
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

    /// Keep a callout field editor in step with its growing text: a new
    /// callout re-anchors its block beside the base, away from the tip, so
    /// the text never runs over the arrow; a re-edit keeps its authored
    /// anchor and only grows the field. Other editors keep their frame.
    fn refit_callout_editor(&self) {
        let ivars = self.ivars();
        let Some(field) = ivars.text_editor.borrow().clone() else {
            return;
        };
        // Shift+Enter leaves a line separator (U+2028) until the commit
        // normalizes it; measure it as the newline it will become.
        let text = field.stringValue().to_string().replace('\u{2028}', "\n");
        let (anchor, size) = match ivars.text_target.get() {
            Some(TextTarget::New { callout: true, .. }) => {
                let size = DEFAULT_TEXT_SIZE_PX;
                let (w_pt, h_pt) = self.text_editor_size(&text, size, true);
                let (sx, sy) = self.view_scale();
                let Some(anchor) = self.pending_callout_anchor(Size::new(w_pt / sx, h_pt / sy))
                else {
                    return;
                };
                (anchor, size)
            }
            Some(TextTarget::Existing { id }) => {
                let shot = ivars.shot.borrow();
                let Some(el) = shot.as_ref().and_then(|shot| shot.doc.element(id)) else {
                    return;
                };
                match &el.kind {
                    ElementKind::Callout { anchor, size, .. } => (*anchor, *size),
                    _ => return,
                }
            }
            _ => return,
        };
        field.setFrame(self.text_editor_frame(anchor, &text, size, true));
    }

    /// Drop the open field editor without committing (a new shot arrived)
    /// and drop a pending callout with it.
    fn discard_text_editor(&self) {
        let ivars = self.ivars();
        // Take every slot before removing the field: removeFromSuperview
        // ends editing and re-enters commit_text_editor synchronously, and
        // a borrow held across the removal would panic that re-entry.
        let field = ivars.text_editor.borrow_mut().take();
        ivars.text_target.take();
        *ivars.pending_callout.borrow_mut() = None;
        if let Some(field) = field {
            // Ends editing, which re-enters commit with both slots empty.
            field.removeFromSuperview();
        }
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

    /// The eight frame handles (image pixels), corners first for hit
    /// priority.
    fn frame_spot_points(&self) -> Vec<(FrameSpot, Point)> {
        match self.current_frame_rect() {
            Some(r) => spot_points(r),
            None => Vec::new(),
        }
    }

    /// The frame spot under an image-space cursor, if any. The tolerance is
    /// the view-space handle radius converted through the view scale.
    fn frame_spot_at(&self, p: Point) -> Option<(FrameSpot, Point)> {
        let (sx, sy) = self.view_scale();
        let tol_x = (HANDLE_RADIUS_PT + 3.0) / sx.max(0.001);
        let tol_y = (HANDLE_RADIUS_PT + 3.0) / sy.max(0.001);
        self.current_frame_rect().and_then(|r| spot_at(r, p, tol_x, tol_y))
    }

    /// The view-space cursor rect around a frame spot.
    fn spot_cursor_rect(&self, point: Point) -> CGRect {
        let pad = HANDLE_RADIUS_PT + 3.0;
        let v = self.pixel_to_view(point);
        CGRect::new(
            CGPoint::new(v.x - pad, v.y - pad),
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
        ok.setFocusRingType(NSFocusRingType::None);
        ok.setRefusesFirstResponder(true);
        // The default-button weight: Enter applies the frame, the button is
        // the frame mode's confirmation.
        ok.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize())));
        // An accent plate instead of the default bezel; the button keeps
        // the dark appearance so its title renders white on the accent in
        // the light window.
        ok.setBordered(false);
        ok.setWantsLayer(true);
        if let Some(dark) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }) {
            ok.setAppearance(Some(&dark));
        }
        if let Some(layer) = ok.layer() {
            layer.setCornerRadius(6.0);
            layer.setBackgroundColor(Some(&NSColor::controlAccentColor().CGColor()));
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
        let frame = self.frame_rect();
        let (sx, sy) = self.view_scale();
        let bottom_y = frame.origin.y + frame.size.height - rect.max_y() * sy;
        let top_y = frame.origin.y + frame.size.height - rect.y * sy;
        let below = bottom_y - FRAME_OK_SIZE.height - 6.0;
        let strip_y = if below >= 2.0 { below } else { top_y + 6.0 };
        // A frame hugging the canvas top would otherwise push the strip
        // above the canvas (into the toolbar): keep it on the canvas.
        let strip_y = strip_y.clamp(
            2.0,
            (bounds.size.height - FRAME_OK_SIZE.height - 2.0).max(2.0),
        );
        if let Some(ok) = ivars.frame_ok.borrow().as_ref() {
            let x = (frame.origin.x + rect.max_x() * sx - FRAME_OK_SIZE.width)
                .clamp(0.0, (bounds.size.width - FRAME_OK_SIZE.width).max(0.0));
            ok.setFrame(CGRect::new(
                CGPoint::new(x, strip_y),
                CGSize::new(FRAME_OK_SIZE.width, FRAME_OK_SIZE.height),
            ));
        }
        if let Some(label) = ivars.frame_label.borrow().as_ref() {
            let text = format!("{} × {}", rect.width.round(), rect.height.round());
            label.setStringValue(&NSString::from_str(&text));
            let label_w = 160.0_f64.min(bounds.size.width.max(0.0));
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
        let r = clamp_crop_rect(edit.rect, iw as f64, ih as f64);
        let (w, h) = (r.width.round(), r.height.round());
        if w < 1.0 || h < 1.0 {
            *ivars.frame_edit.borrow_mut() = Some(edit);
            self.layout_frame_chrome();
            self.redraw();
            return;
        }
        let cmd = if edit.resize {
            Command::Resize(Size::new(w, h))
        } else {
            Command::Crop(Rect::new(r.x, r.y, w, h))
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
        self.redraw();
        self.refit_window();
    }

    /// Cancel the frame mode (Esc): drop the frame, deselect the tool.
    fn cancel_frame_edit(&self) {
        self.exit_frame_edit();
        self.ivars().tool.set(None);
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
        // Display scale: 1/n for an @nx capture (the size it had on
        // screen), then fitted down when even that exceeds the screen.
        // fit_size never upscales, so 1x shots pass through unchanged.
        let density = self.ivars().density.get();
        let content_size = NSScreen::mainScreen(mtm)
            .map(|screen| screen.visibleFrame().size)
            .map(|visible| {
                fit_size(
                    iw as f64 / density,
                    ih as f64 / density,
                    visible.width - 2.0 * CARD_GAP,
                    visible.height - TOOLBAR_HEIGHT - CARD_GAP,
                )
            })
            .unwrap_or_else(|| CGSize::new(iw as f64 / density, ih as f64 / density));
        // The window wraps the frame: the toolbar strip on top, the card
        // touching it from below, the card gap on the other three sides.
        // The width never drops below the toolbar's minimum.
        let width = (content_size.width + 2.0 * CARD_GAP)
            .max(self.toolbar().map(|toolbar| toolbar.min_width()).unwrap_or(0.0));
        let height = TOOLBAR_HEIGHT + content_size.height + CARD_GAP;
        self.ivars().frame_size.set(content_size);
        if self.ivars().placed.get() {
            // Keep the window's top-left corner where it was: AppKit's frame
            // origin is the bottom-left one, so the same corner moves the
            // origin by the height delta.
            let old = window.frame();
            let frame = CGRect::new(
                CGPoint::new(old.origin.x, old.origin.y + old.size.height - height),
                CGSize::new(width, height),
            );
            window.setFrame_display(frame, false);
        } else {
            // The first placement centers the window; every later refit
            // anchors it instead.
            window.setContentSize(CGSize::new(width, height));
            window.center();
            self.ivars().placed.set(true);
        }
        self.redraw();
    }

    /// Show (or replace) the shot in the window: file-name label, density,
    /// selection reset, full refit.
    fn display_shot(&self, shot: OpenShot) {
        let file_name = shot.file_name();
        // Any in-flight interaction belongs to the outgoing shot.
        self.discard_text_editor();
        self.exit_frame_edit();
        *self.ivars().shot.borrow_mut() = Some(shot);
        self.ivars().density.set(name_density(&file_name));
        self.ivars().selection.set(None);
        self.refit_window();
        // refit ends in a redraw, and the redraw pushes the toolbar state:
        // the file label, the tint, the swatch and the lights' alignment
        // (which must follow the refit — the titlebar relayouts on resize)
        // all ride on that single push.
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

    /// The toolbar strip, if the window is fully assembled.
    fn toolbar(&self) -> Option<Retained<ToolbarView>> {
        self.ivars().toolbar.borrow().clone()
    }

    /// The toolbar reported a user action: apply the toggle rule for the
    /// tool buttons, run the leafing, and let the follow-up work happen on
    /// the window side.
    fn handle_toolbar_event(&self, event: ToolbarEvent) {
        match event {
            ToolbarEvent::ToolToggled(tool) => {
                // Clicking the active tool again deselects it; the redraw
                // at the end of each path repaints the highlight.
                let next = if self.ivars().tool.get() == Some(tool) {
                    None
                } else {
                    Some(tool)
                };
                self.ivars().tool.set(next);
                // The crop/resize tools bring up their adjustable frame;
                // anything else (including deselecting the frame tool)
                // tears it down.
                match next {
                    Some(Tool::Crop) => self.enter_frame_edit(false),
                    Some(Tool::Resize) => self.enter_frame_edit(true),
                    _ => self.exit_frame_edit(),
                }
            }
            ToolbarEvent::PagePrevious => self.page(-1),
            ToolbarEvent::PageNext => self.page(1),
            ToolbarEvent::WidthChanged(value) => {
                self.ivars().stroke_width.set(value);
                // With a stroke-bearing element selected, the slider
                // restyles that element live (one undo step per drag, not
                // per tick).
                if let Some(id) = self.ivars().selection.get() {
                    let strokeable = self
                        .ivars()
                        .shot
                        .borrow()
                        .as_ref()
                        .and_then(|shot| shot.doc.element(id))
                        .is_some_and(|el| stroke_width_of(&el.kind).is_some());
                    if strokeable {
                        let result = self
                            .ivars()
                            .shot
                            .borrow_mut()
                            .as_mut()
                            .map(|shot| shot.apply_width(id, value));
                        if let Some(Err(err)) = result {
                            eprintln!("editor: failed to restyle the element width: {err}");
                        }
                    }
                }
                self.redraw();
            }
            ToolbarEvent::PresetSelected(color) => {
                self.ivars().color.set(color);
                // A picked preset recolors the selected element (blur,
                // which has no color, is skipped so undo never holds a
                // no-op step).
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
                self.redraw();
            }
            ToolbarEvent::ArrowParamChanged { index, value } => {
                if let Some(spec) = ARROW_PARAM_SPECS.get(index) {
                    let mut params = render::arrow_params();
                    (spec.set)(&mut params, value);
                    render::set_arrow_params(params);
                }
                // Instant feedback: the canvas re-renders every element
                // from the live params on each draw. No command is
                // involved, so nothing enters the undo journal or the
                // session files.
                self.redraw();
            }
            ToolbarEvent::ArrowParamReset(index) => {
                if let Some(spec) = ARROW_PARAM_SPECS.get(index) {
                    let defaults = render::ArrowParams::DEFAULT;
                    let value = (spec.get)(defaults);
                    let mut params = render::arrow_params();
                    (spec.set)(&mut params, value);
                    render::set_arrow_params(params);
                }
                self.redraw();
            }
            ToolbarEvent::ArrowParamsReset => {
                render::set_arrow_params(render::ArrowParams::DEFAULT);
                self.redraw();
            }
        }
    }

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

impl EditorWindow {
    pub fn new(mtm: MainThreadMarker, save_dir: std::path::PathBuf) -> EditorWindow {
        let view = EditorView::new(mtm);
        view.set_save_dir(save_dir);
        let window = unsafe {
            KeyableWindow::init_with_content_rect(
                KeyableWindow::alloc(mtm),
                CGRect::new(
                    CGPoint::ZERO,
                    CGSize::new(INITIAL_SIZE.width, INITIAL_SIZE.height + TOOLBAR_HEIGHT),
                ),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::FullSizeContentView
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // Electron-style hidden inset: a titled window with a transparent,
        // hidden titlebar — the system traffic lights draw over the toolbar
        // column, which stays the only chrome of the window.
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        // The toolbar strip is the window's titlebar for the system, and it
        // must not drag the window: with movability on, macOS replays any
        // press in the strip that a control does not claim (the width
        // slider's tracking loop aside) as a window drag, and a press
        // landing in the slider before its `mouseDown:` runs is stolen
        // outright. The window still moves — the panel and the viewing-mode
        // canvas drag it through their own mouse handlers.
        window.setMovable(false);
        window.setHasShadow(true);
        // Miniaturizable/Resizable light up the yellow and green buttons —
        // without the bits AppKit draws them disabled. FullScreen stays out:
        // with it the window loses the system corner rounding, and green
        // zooms natively without it (change editor-system-buttons).
        // The light appearance everywhere: the popover and every system
        // control resolve against it.
        if let Some(aqua) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }) {
            window.setAppearance(Some(&aqua));
        }
        // The flat light gray both the toolbar strip and the card margins
        // read as. The window paints it — the canvas stays clear of the
        // window background, so the system corner rounding survives.
        let window_bg = NSColor::colorWithSRGBRed_green_blue_alpha(
            WINDOW_BG[0],
            WINDOW_BG[1],
            WINDOW_BG[2],
            WINDOW_BG[3],
        );
        window.setBackgroundColor(Some(&window_bg));
        // Toolbar hover styling rides on mouse-moved events.
        window.setAcceptsMouseMovedEvents(true);
        // The window lives in the app controller for the whole process;
        // closing only hides it, the next shot reuses it (no second window).
        // SAFETY: plain setter on the freshly created window.
        unsafe {
            window.setReleasedWhenClosed(false);
        }

        // Layout: the toolbar strip across the top, the canvas filling
        // everything below it. The canvas paints nothing outside the card,
        // so the window background — the flat light gray — shows around
        // it and under the transparent strip.
        let content = window.contentView().expect("content view");
        let size = content.bounds().size;

        // The strip builds and owns its row, the tracking area and the
        // hover pass; the window mounts it and reads only its geometry.
        // Its events come back through the single callback; the swatch and
        // the slider keep their canvas handlers until they move behind the
        // same seam.
        let weak_view: Weak<EditorView> = Weak::new(view.as_ref());
        let toolbar = ToolbarView::new(mtm, size, move |event| {
                if let Some(view) = weak_view.load() {
                    view.handle_toolbar_event(event);
                }
            },
        );
        // The window cannot shrink below the bar: the leading pad for the
        // traffic lights, the row's fitting width, the trailing pad.
        window.setContentMinSize(CGSize::new(toolbar.min_width(), 0.0));
        toolbar.setFrame(CGRect::new(
            CGPoint::new(0.0, size.height - TOOLBAR_HEIGHT),
            CGSize::new(size.width, TOOLBAR_HEIGHT),
        ));
        content.addSubview(&toolbar);
        view.setFrame(CGRect::new(
            CGPoint::ZERO,
            CGSize::new(size.width, size.height - TOOLBAR_HEIGHT),
        ));
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview(&view);
        // Keys land on the canvas; the toolbar buttons refuse first
        // responder, so the window never opens with a focused button.
        window.setInitialFirstResponder(Some(&view));

        // The arrow debug tuner overlays the canvas' top-left corner under
        // MINISNAP_ARROW_DEBUG — a tuning instrument, not shipped UI.
        if std::env::var_os("MINISNAP_ARROW_DEBUG").is_some_and(|v| !v.is_empty()) {
            let tuner = arrow_tuner_panel(mtm, &toolbar);
            let fit = tuner.frame().size;
            tuner.setFrame(CGRect::new(
                CGPoint::new(8.0, size.height - TOOLBAR_HEIGHT - 8.0 - fit.height),
                fit,
            ));
            tuner.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
            content.addSubview(&tuner);
        }

        *view.ivars().toolbar.borrow_mut() = Some(toolbar.clone());
        view.redraw();

        EditorWindow { window, view }
    }

    /// Show (or replace) the shot in this window and bring it to the front.
    pub fn show_shot(&self, shot: OpenShot) {
        self.view.display_shot(shot);
        self.window.makeKeyAndOrderFront(None);
    }
}

/// The light checkerboard image editors draw behind transparent areas,
/// as a cached pattern color: a 2x-scaled 2×2-cell tile stays crisp on
/// retina screens.
fn checker_color() -> Retained<NSColor> {
    let cell = (CHECKER_CELL_PT * 2.0) as usize;
    let tile_pt = CGSize::new(CHECKER_CELL_PT * 2.0, CHECKER_CELL_PT * 2.0);
    let Some(ctx) = render::bitmap_context(cell * 2, cell * 2) else {
        return NSColor::lightGrayColor();
    };
    CGContext::set_rgb_fill_color(Some(&ctx), 1.0, 1.0, 1.0, 1.0);
    CGContext::fill_rect(
        Some(&ctx),
        CGRect::new(CGPoint::ZERO, CGSize::new((cell * 2) as f64, (cell * 2) as f64)),
    );
    CGContext::set_rgb_fill_color(Some(&ctx), 0.8, 0.8, 0.8, 1.0);
    CGContext::fill_rect(Some(&ctx), CGRect::new(CGPoint::ZERO, CGSize::new(cell as f64, cell as f64)));
    CGContext::fill_rect(Some(&ctx), CGRect::new(CGPoint::new(cell as f64, cell as f64), CGSize::new(cell as f64, cell as f64)));
    let Some(image) = CGBitmapContextCreateImage(Some(&ctx)) else {
        return NSColor::lightGrayColor();
    };
    // Designated initializers on fresh allocations.
    let tile = NSImage::initWithCGImage_size(NSImage::alloc(), &image, tile_pt);
    NSColor::colorWithPatternImage(&tile)
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
