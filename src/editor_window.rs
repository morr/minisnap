//! The editor window: one reusable `NSWindow` showing the open shot — the
//! base frame plus annotations, drawn with CoreGraphics into a custom
//! `NSView`, with an annotation toolbar above the canvas. Selection markers
//! are drawn on top in the canvas and never become part of a saved
//! composite.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    define_class, extern_methods, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker,
    MainThreadOnly,
};
use objc2_app_kit::{
    NSAppearance, NSApplication, NSAutoresizingMaskOptions, NSBackingStoreType, NSButton,
    NSColor, NSControl, NSControlTextEditingDelegate, NSCursor, NSEvent, NSEventModifierFlags,
    NSFocusRingType, NSFont, NSFontWeightRegular, NSGraphicsContext, NSImage,
    NSImageSymbolConfiguration, NSLineBreakMode, NSPopover, NSPopoverDelegate, NSScreen,
    NSStackView, NSTextField, NSTextFieldDelegate, NSTextView,
    NSTrackingArea, NSTrackingAreaOptions, NSUserInterfaceLayoutOrientation, NSView,
    NSViewController, NSVisualEffectMaterial, NSVisualEffectView, NSWindow, NSWindowStyleMask,
};
use objc2_app_kit::NSAppearanceCustomization;
use objc2_app_kit::NSAppearanceNameDarkAqua;
use objc2_app_kit::NSLayoutConstraintOrientation;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreateImage, CGImage, CGContext};
use objc2_foundation::{NSString, NSNotification, NSRectEdge};

use crate::editor::hit_test::{Hit, text_bounds, top_hit};
use crate::editor::text;
use crate::editor::{
    render, set_endpoint, shift_element, Command, Element, ElementKind, Endpoint, OpenShot, Point,
    Rect, Rgba, Size,
};

/// The dark backdrop the frame sits on.
const BACKDROP: [f64; 4] = [0.055, 0.055, 0.065, 1.0];

/// Corner radius of the borderless editor window (and of the floating
/// toolbar panel), in points.
const WINDOW_RADIUS: f64 = 12.0;

/// Selection marker radius, in view points (constant on screen).
const MARKER_RADIUS_PT: f64 = 4.5;

/// Floating toolbar panel height, in points.
const TOOLBAR_HEIGHT: f64 = 44.0;

/// Margin between the floating toolbar panel and the window edges, in
/// points.
const TOOLBAR_MARGIN: f64 = 10.0;

/// The bottom strip the toolbar reserves: the panel plus its margin. The
/// canvas fills everything above it.
const TOOLBAR_RESERVED: f64 = TOOLBAR_HEIGHT + TOOLBAR_MARGIN;

/// Initial canvas size before the first shot arrives.
const INITIAL_SIZE: CGSize = CGSize::new(480.0, 320.0);

/// Default stroke width for new elements, in image pixels.
const DEFAULT_STROKE_WIDTH: f64 = 4.0;

/// Range of the stroke-width slider (and of authored widths), in image
/// pixels.
const STROKE_MIN: f64 = 1.0;
const STROKE_MAX: f64 = 12.0;

/// Width of the stroke-width slider in the toolbar, in points.
const STROKE_SLIDER_WIDTH_PT: f64 = 90.0;

/// Knob radius of the custom stroke-width slider, in points.
const SLIDER_KNOB_RADIUS: f64 = 6.5;

/// Height of the custom stroke-width slider's track, in points.
const SLIDER_TRACK_HEIGHT: f64 = 3.0;

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

/// Toolbar tag of the color-swatch button: its layer background is the
/// current color, so hover styling skips it.
const COLOR_BUTTON_TAG: isize = -1;

/// Toolbar button side, in points.
const TOOLBAR_BUTTON_SIZE: CGSize = CGSize::new(26.0, 26.0);

/// Tint of an inactive toolbar symbol: near-white, readable on the dark
/// HUD panel regardless of the system appearance.
const ICON_TINT: [f64; 4] = [1.0, 1.0, 1.0, 0.9];

/// Hover background of a toolbar button: a faint white wash.
const HOVER_BG: [f64; 4] = [1.0, 1.0, 1.0, 0.10];

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

/// A callout whose arrow is drawn but whose text is still being entered:
/// the arrow previews on the canvas and the element is committed (or
/// dropped) together with the text.
struct PendingCallout {
    /// The arrow's start (the text block goes beside it).
    base: Point,
    tip: Point,
    color: Rgba,
    size: f64,
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

define_class!(
    /// The editor's borderless window. Borderless `NSWindow`s refuse key
    /// status by default; the canvas lives on keyboard commands (Esc, ⌘Q,
    /// ⌘C, ⌘Z, ←/→), so the window must accept key.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    pub struct KeyableWindow;

    unsafe impl NSObjectProtocol for KeyableWindow {}

    impl KeyableWindow {
        #[unsafe(method(canBecomeKey))]
        fn __can_become_key(&self) -> bool {
            true
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

define_class!(
    /// The floating toolbar panel: a vibrancy pill that drags the window
    /// when a click lands on its free surface. Controls consume their own
    /// clicks, so they never reach this handler.
    #[unsafe(super(NSVisualEffectView))]
    #[thread_kind = MainThreadOnly]
    struct ToolbarPanel;

    unsafe impl NSObjectProtocol for ToolbarPanel {}

    impl ToolbarPanel {
        #[unsafe(method(mouseDown:))]
        fn __mouse_down(&self, event: &NSEvent) {
            if let Some(window) = self.window() {
                window.performWindowDragWithEvent(event);
            }
        }
    }
);

impl ToolbarPanel {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: designated initializer with no ivars.
        unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::ZERO) }
    }
}

/// Ivars of the custom stroke-width slider.
struct WidthSliderIvars {
    value: Cell<f64>,
    target: RefCell<Option<Retained<AnyObject>>>,
    action: Cell<Option<Sel>>,
}

define_class!(
    /// The custom-drawn stroke-width slider: a thin track, an accent fill
    /// and a white round knob. The contract mirrors the `NSSlider` it
    /// replaced — `doubleValue` is what the editor's `widthChanged:` reads
    /// and `setDoubleValue:` is what syncs the knob with the selection —
    /// but it draws itself and reports every drag step (the width edits
    /// collapse into one undo step anyway).
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = WidthSliderIvars]
    struct WidthSliderView;

    unsafe impl NSObjectProtocol for WidthSliderView {}

    impl WidthSliderView {
        #[unsafe(method_id(initWithFrame:))]
        unsafe fn __init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self> {
            let this = this.set_ivars(WidthSliderIvars {
                value: Cell::new(DEFAULT_STROKE_WIDTH),
                target: RefCell::new(None),
                action: Cell::new(None),
            });
            // SAFETY: NSView's designated initializer with plain ivars set.
            unsafe { msg_send![super(this), initWithFrame: frame_rect] }
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn __accepts_first_responder(&self) -> bool {
            // Keys stay with the canvas, like every other toolbar control.
            false
        }

        #[unsafe(method(doubleValue))]
        fn __double_value(&self) -> f64 {
            self.ivars().value.get()
        }

        #[unsafe(method(setDoubleValue:))]
        fn __set_double_value(&self, value: f64) {
            self.set_value(value.clamp(STROKE_MIN, STROKE_MAX));
        }

        #[unsafe(method(setTarget:))]
        unsafe fn __set_target(&self, target: Option<&AnyObject>) {
            *self.ivars().target.borrow_mut() = target.and_then(|target| {
                // SAFETY: the target outlives the slider's use of it — the
                // editor view owns both ends of the pair.
                unsafe { Retained::retain((target as *const AnyObject).cast_mut()) }
            });
        }

        #[unsafe(method(setAction:))]
        unsafe fn __set_action(&self, action: Option<Sel>) {
            self.ivars().action.set(action);
        }

        #[unsafe(method(drawRect:))]
        fn __draw_rect(&self, _dirty_rect: CGRect) {
            self.draw();
        }

        #[unsafe(method(mouseDown:))]
        fn __mouse_down(&self, event: &NSEvent) {
            self.drag_to(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn __mouse_dragged(&self, event: &NSEvent) {
            self.drag_to(event);
        }

        #[unsafe(method(mouseUp:))]
        fn __mouse_up(&self, event: &NSEvent) {
            self.drag_to(event);
        }
    }
);

impl WidthSliderView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;

        #[unsafe(method(setTarget:))]
        unsafe fn set_target(&self, target: Option<&AnyObject>);

        #[unsafe(method(setAction:))]
        unsafe fn set_action(&self, action: Option<Sel>);

        #[unsafe(method(setDoubleValue:))]
        fn set_double_value(&self, value: f64);
    );

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: designated initializer with plain ivars set.
        unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::ZERO) }
    }

    /// Center X of the knob for the current value, in view points.
    fn knob_x(&self, width: f64) -> f64 {
        let frac = ((self.ivars().value.get() - STROKE_MIN) / (STROKE_MAX - STROKE_MIN))
            .clamp(0.0, 1.0);
        SLIDER_KNOB_RADIUS + (width - 2.0 * SLIDER_KNOB_RADIUS) * frac
    }

    /// Store the value, redraw, and report the change through the
    /// target/action pair (the `widthChanged:` editor handler).
    fn set_value(&self, value: f64) {
        let old = self.ivars().value.get();
        self.ivars().value.set(value);
        if value == old {
            return;
        }
        self.setNeedsDisplayInRect(self.bounds());
        self.send_action();
    }

    /// Track the drag (or a plain click) to the event's X position.
    fn drag_to(&self, event: &NSEvent) {
        let bounds = self.bounds();
        if bounds.size.width < 2.0 * SLIDER_KNOB_RADIUS {
            return;
        }
        let local = self.convertPoint_fromView(event.locationInWindow(), None);
        let frac = ((local.x - SLIDER_KNOB_RADIUS)
            / (bounds.size.width - 2.0 * SLIDER_KNOB_RADIUS))
        .clamp(0.0, 1.0);
        self.set_value(STROKE_MIN + (STROKE_MAX - STROKE_MIN) * frac);
    }

    /// Fire the target/action pair like a control would.
    fn send_action(&self) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let action = self.ivars().action.get();
        let target = self.ivars().target.borrow().clone();
        if let Some(action) = action {
            // SAFETY: main-thread target/action pair; the action takes the
            // slider as its sender.
            unsafe {
                NSApplication::sharedApplication(mtm).sendAction_to_from(
                    action,
                    target.as_deref(),
                    Some(self),
                );
            }
        }
    }

    /// The track, the accent fill up to the knob, and the white knob.
    fn draw(&self) {
        let Some(context) = NSGraphicsContext::currentContext() else {
            return;
        };
        let cg = context.CGContext();
        let bounds = self.bounds();
        let (w, h) = (bounds.size.width, bounds.size.height);
        if w < 2.0 * SLIDER_KNOB_RADIUS {
            return;
        }
        let knob_x = self.knob_x(w);
        let track_y = h / 2.0 - SLIDER_TRACK_HEIGHT / 2.0;
        // The recessed track behind the fill.
        CGContext::set_rgb_fill_color(Some(&cg), 1.0, 1.0, 1.0, 0.22);
        CGContext::fill_rect(
            Some(&cg),
            CGRect::new(
                CGPoint::new(SLIDER_KNOB_RADIUS, track_y),
                CGSize::new(w - 2.0 * SLIDER_KNOB_RADIUS, SLIDER_TRACK_HEIGHT),
            ),
        );
        // The accent fill from the left end to the knob center.
        let filled = knob_x - SLIDER_KNOB_RADIUS;
        if filled > 0.0 {
            let accent: Retained<NSColor> = NSColor::controlAccentColor();
            let accent_cg = accent.CGColor();
            CGContext::set_fill_color_with_color(Some(&cg), Some(&accent_cg));
            CGContext::fill_rect(
                Some(&cg),
                CGRect::new(
                    CGPoint::new(SLIDER_KNOB_RADIUS, track_y),
                    CGSize::new(filled, SLIDER_TRACK_HEIGHT),
                ),
            );
        }
        // The knob: a white disc with a soft shadow.
        let knob = CGRect::new(
            CGPoint::new(knob_x - SLIDER_KNOB_RADIUS, h / 2.0 - SLIDER_KNOB_RADIUS),
            CGSize::new(2.0 * SLIDER_KNOB_RADIUS, 2.0 * SLIDER_KNOB_RADIUS),
        );
        let shadow = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.35);
        let shadow_cg = shadow.CGColor();
        CGContext::set_shadow_with_color(
            Some(&cg),
            CGSize::new(0.0, 1.0),
            2.0,
            Some(&shadow_cg),
        );
        CGContext::set_rgb_fill_color(Some(&cg), 1.0, 1.0, 1.0, 1.0);
        CGContext::fill_ellipse_in_rect(Some(&cg), knob);
        CGContext::set_shadow_with_color(Some(&cg), CGSize::ZERO, 0.0, None);
    }
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
    /// The file-name label in the toolbar (the borderless window has no
    /// system title).
    file_label: RefCell<Option<Retained<NSTextField>>>,
    /// Toolbar buttons (tool tag → button) for the active highlight.
    tool_buttons: RefCell<Vec<(isize, Retained<NSButton>)>>,
    /// The color-swatch toolbar button (its bezel shows the current color).
    color_button: RefCell<Option<Retained<NSButton>>>,
    /// The open color popover, if any.
    color_popover: RefCell<Option<Retained<NSPopover>>>,
    /// The stroke-width slider (its knob mirrors the selection's width).
    width_slider: RefCell<Option<Retained<WidthSliderView>>>,
    /// The floating toolbar panel (hover hit-testing target).
    toolbar_panel: RefCell<Option<Retained<ToolbarPanel>>>,
    /// The smallest content width that fits the whole toolbar.
    min_width: Cell<f64>,
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
                pending_callout: RefCell::new(None),
                frame_label: RefCell::new(None),
                frame_ok: RefCell::new(None),
                save_dir: RefCell::new(std::path::PathBuf::new()),
                file_label: RefCell::new(None),
                tool_buttons: RefCell::new(Vec::new()),
                color_button: RefCell::new(None),
                color_popover: RefCell::new(None),
                width_slider: RefCell::new(None),
                toolbar_panel: RefCell::new(None),
                min_width: Cell::new(0.0),
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

        #[unsafe(method(mouseEntered:))]
        fn __mouse_entered(&self, event: &NSEvent) {
            self.hover_at(Some(event));
        }

        #[unsafe(method(mouseExited:))]
        fn __mouse_exited(&self, _event: &NSEvent) {
            self.hover_at(None);
        }

        #[unsafe(method(mouseMoved:))]
        fn __mouse_moved(&self, event: &NSEvent) {
            self.hover_at(Some(event));
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
            let popover = self.ivars().color_popover.borrow_mut().take();
            if let Some(popover) = popover {
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
            // With a stroke-bearing element selected, the slider restyles
            // that element live (one undo step per drag, not per tick).
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
                },
            };
            render::draw_elements(&cg, base, ih as f64, std::slice::from_ref(&pending_el));
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
        // Handle dots: white fill with the accent ring. A dot sitting on
        // the window edge is pressed inward just enough to stay fully
        // visible; its hit zone stays at the true spot.
        for (_, point) in self.frame_spot_points() {
            let r = HANDLE_RADIUS_PT;
            let cx = (point.x * sx).clamp(r, (bounds.size.width - r).max(r));
            let cy = (bounds.size.height - point.y * sy).clamp(r, (bounds.size.height - r).max(r));
            let marker = CGRect::new(
                CGPoint::new(cx - r, cy - r),
                CGSize::new(2.0 * r, 2.0 * r),
            );
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
            // tip; the text follows in the second phase, at mouse-up.
            Some(Tool::Callout) => Some(ElementKind::Callout {
                tip: point,
                anchor: point,
                text: String::new(),
                color: ivars.color.get(),
                size: DEFAULT_TEXT_SIZE_PX,
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
                if !Self::draft_is_real(&kind) {
                    // Plain click: the draft is discarded and the selection
                    // was already cleared at mouse-down.
                    self.redraw();
                    return;
                }
                // A callout splits into two phases: the drawn arrow stays
                // up as a pending preview while the text is entered at its
                // base; the element lands (or is dropped) with the text.
                if let ElementKind::Callout { anchor, tip, color, size, text: _ } = kind {
                    *ivars.pending_callout.borrow_mut() = Some(PendingCallout {
                        base: anchor,
                        tip,
                        color,
                        size,
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
            ElementKind::Callout { anchor, tip, .. } => {
                point_dist(*anchor, *tip) >= MIN_DRAG_PX
            }
            _ => false,
        }
    }

    /// Schedule a full redraw of the canvas.
    fn redraw(&self) {
        // The redraw follows every state change a selection can ride on
        // (mouse-up commits, undo/redo), so it doubles as the hook that
        // keeps the slider handle pointing at the selection's width.
        self.sync_width_slider();
        self.setNeedsDisplayInRect(self.bounds());
    }

    /// Point the stroke-width slider — and the default width for new
    /// elements — at the selected element's width, when the element is one
    /// the slider styles. Selections of other kinds leave both as they are.
    fn sync_width_slider(&self) {
        let Some(slider) = self.ivars().width_slider.borrow().clone() else {
            return;
        };
        let width = self.ivars().selection.get().and_then(|id| {
            self.ivars()
                .shot
                .borrow()
                .as_ref()
                .and_then(|shot| shot.doc.element(id))
                .and_then(|el| stroke_width_of(&el.kind))
        });
        let Some(width) = width else { return };
        let clamped = width.clamp(STROKE_MIN, STROKE_MAX);
        slider.set_double_value(clamped);
        self.ivars().stroke_width.set(clamped);
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
        let (width, height) = if text.is_empty() {
            (TEXT_EDITOR_WIDTH_PT, font_pt * 1.6 + 6.0)
        } else {
            match text::measure_block(text, size) {
                Some(m) => (
                    (m.width * sx).max(TEXT_EDITOR_WIDTH_PT) + 12.0,
                    m.height * sy + 6.0,
                ),
                None => (TEXT_EDITOR_WIDTH_PT, font_pt * 1.6 + 6.0),
            }
        };
        let x = origin.x * sx - 3.0;
        let y_top = bounds.size.height - origin.y * sy;
        CGRect::new(CGPoint::new(x, y_top - height), CGSize::new(width, height))
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
                    let (tip, base) = match ivars.pending_callout.borrow_mut().take() {
                        Some(pending) => (pending.tip, pending.base),
                        None => (origin, origin),
                    };
                    let base_image = shot.doc.base();
                    let frame = Rect::new(
                        0.0,
                        0.0,
                        CGImage::width(Some(base_image)) as f64,
                        CGImage::height(Some(base_image)) as f64,
                    );
                    ElementKind::Callout {
                        tip,
                        anchor: callout_anchor(base, &text, size, frame),
                        text,
                        color,
                        size,
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
    /// priority. There is no center handle: for crop, drags that miss the
    /// handles move the whole frame (`FrameSpot::Center`); resize has no
    /// move at all.
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
        ok.setFocusRingType(NSFocusRingType::None);
        ok.setRefusesFirstResponder(true);
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
        // A frame hugging the canvas top would otherwise push the strip
        // above the canvas (into the toolbar): keep it on the canvas.
        let strip_y = strip_y.clamp(
            2.0,
            (bounds.size.height - FRAME_OK_SIZE.height - 2.0).max(2.0),
        );
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
                fit_size(
                    iw as f64,
                    ih as f64,
                    visible.width,
                    visible.height - TOOLBAR_RESERVED,
                )
            })
            .unwrap_or_else(|| CGSize::new(iw as f64, ih as f64));
        // The refit never shrinks below the toolbar's minimum width.
        let width = content_size.width.max(self.ivars().min_width.get());
        window.setContentSize(CGSize::new(
            width,
            content_size.height + TOOLBAR_RESERVED,
        ));
        window.center();
        self.redraw();
    }

    /// Show (or replace) the shot in the window: file-name label, size,
    /// full redraw.
    fn display_shot(&self, shot: OpenShot) {
        let file_name = shot.file_name();
        // Any in-flight interaction belongs to the outgoing shot.
        self.discard_text_editor();
        self.exit_frame_edit();
        *self.ivars().shot.borrow_mut() = Some(shot);
        self.ivars().selection.set(None);
        if let Some(label) = self.ivars().file_label.borrow().as_ref() {
            label.setStringValue(&NSString::from_str(&file_name));
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

    /// Restyle the toolbar button under the cursor: a faint wash on the
    /// hovered button, nothing on the others. The active tool's button and
    /// the color swatch own their backgrounds and are skipped.
    fn hover_at(&self, event: Option<&NSEvent>) {
        let hovered = event.and_then(|event| {
            let panel = self.ivars().toolbar_panel.borrow().clone()?;
            let point = panel.convertPoint_fromView(event.locationInWindow(), None);
            let hit = panel.hitTest(point)?;
            hit.downcast_ref::<NSButton>()
                .map(|button| button.tag() as isize)
        });
        let active = self.ivars().tool.get().map(|tool| tool.tag());
        // The active tool's accent background reads as a highlight already.
        let hovered = if hovered.is_some() && hovered == active {
            None
        } else {
            hovered
        };
        let hover = NSColor::colorWithSRGBRed_green_blue_alpha(
            HOVER_BG[0],
            HOVER_BG[1],
            HOVER_BG[2],
            HOVER_BG[3],
        );
        for (tag, button) in self.ivars().tool_buttons.borrow().iter() {
            if Some(*tag) == active || *tag == COLOR_BUTTON_TAG {
                continue;
            }
            if let Some(layer) = button.layer() {
                let bg: Option<&NSColor> = if Some(*tag) == hovered {
                    Some(&hover)
                } else {
                    None
                };
                layer.setBackgroundColor(bg.map(|color| color.CGColor()).as_deref());
            }
        }
    }

    /// Accent-wash the active tool's button, white-tint the others.
    fn update_tool_highlight(&self) {
        let active = self.ivars().tool.get().map(|tool| tool.tag());
        let accent: Retained<NSColor> = NSColor::controlAccentColor();
        let icon = NSColor::colorWithSRGBRed_green_blue_alpha(
            ICON_TINT[0],
            ICON_TINT[1],
            ICON_TINT[2],
            ICON_TINT[3],
        );
        for (tag, button) in self.ivars().tool_buttons.borrow().iter() {
            if Some(*tag) == active {
                let bg = accent.colorWithAlphaComponent(0.25);
                if let Some(layer) = button.layer() {
                    layer.setBackgroundColor(Some(&bg.CGColor()));
                }
                button.setContentTintColor(Some(&accent));
            } else {
                if let Some(layer) = button.layer() {
                    layer.setBackgroundColor(None);
                }
                button.setContentTintColor(Some(&icon));
            }
        }
    }

    /// Show the current color on the toolbar swatch circle.
    fn update_color_swatch(&self) {
        let button = self.ivars().color_button.borrow().clone();
        let Some(button) = button.as_ref() else {
            return;
        };
        let Some(layer) = button.layer() else {
            return;
        };
        let [r, g, b, a] = self.ivars().color.get();
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        layer.setBackgroundColor(Some(&color.CGColor()));
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
        // The delegate is this canvas view, alive for the process.
        popover.setDelegate(Some(ProtocolObject::from_ref(self)));

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
            button.setFocusRingType(NSFocusRingType::None);
            button.setRefusesFirstResponder(true);
            if let Some(layer) = button.layer() {
                layer.setBackgroundColor(Some(&ns_color.CGColor()));
                layer.setCornerRadius(10.0);
                // A visible outline: light presets (white) would blend into
                // the popover background.
                layer.setBorderWidth(1.0);
                layer.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
            }
            button.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(20.0, 20.0)));
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
        popover.showRelativeToRect_ofView_preferredEdge(
            anchor_button.bounds(),
            &anchor_button,
            NSRectEdge::MinY,
        );
        *self.ivars().color_popover.borrow_mut() = Some(popover);
    }
}

/// Distance between two points.
fn point_dist(a: Point, b: Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// The stroke width of an element the width slider styles; None for kinds
/// without one (text, callout, blur).
fn stroke_width_of(kind: &ElementKind) -> Option<f64> {
    match kind {
        ElementKind::Arrow { width, .. }
        | ElementKind::Rect { width, .. }
        | ElementKind::Line { width, .. }
        | ElementKind::Pen { width, .. } => Some(*width),
        _ => None,
    }
}

/// Top-left of a new callout's text block: above the base, clamped so the
/// whole block stays inside the frame (in image pixels).
fn callout_anchor(base: Point, text: &str, size: f64, frame: Rect) -> Point {
    let (w, h) = text::measure_block(text, size)
        .map(|m| (m.width, m.height))
        .unwrap_or((size * 4.0, size * 1.25));
    let margin = 4.0;
    let max_x = (frame.max_x() - w - margin).max(margin);
    let max_y = (frame.max_y() - h - margin).max(margin);
    Point::new(
        (base.x - w / 2.0).clamp(margin, max_x),
        (base.y - h - 6.0).clamp(margin, max_y),
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
    icon_button(mtm, symbol_image(symbol), tag)
}

/// The SF Symbol image at the shared icon size.
fn symbol_image(symbol: &str) -> Retained<NSImage> {
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
    image
        .imageWithSymbolConfiguration(&config)
        .expect("configured symbol")
}

/// A toolbar button showing `image` as a template (tinted like the SF
/// Symbols), with an empty title and the given tag. Bezel-less: the hover
/// wash and the active-tool accent are layer backgrounds. Toolbar buttons
/// never take first responder: no focus ring, keys stay with the canvas.
fn icon_button(mtm: MainThreadMarker, image: Retained<NSImage>, tag: isize) -> Retained<NSButton> {
    image.setTemplate(true);
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
    button.setFocusRingType(NSFocusRingType::None);
    button.setRefusesFirstResponder(true);
    button.setBordered(false);
    button.setWantsLayer(true);
    if let Some(layer) = button.layer() {
        layer.setCornerRadius(6.0);
    }
    button.setFrame(CGRect::new(CGPoint::ZERO, TOOLBAR_BUTTON_SIZE));
    button
}

/// The custom callout icon: a text block plus an arrow — the two shapes
/// the tool draws (the SF Symbols set has no such glyph).
fn callout_icon() -> Retained<NSImage> {
    // A 2x bitmap shown at 15 pt; the context's Y-up space puts the text
    // bars on top and the arrow at the bottom-right.
    let ctx = render::bitmap_context(30, 30).expect("icon context");
    CGContext::set_rgb_fill_color(Some(&ctx), 0.0, 0.0, 0.0, 1.0);
    for (y, len) in [(24.0, 12.0), (19.0, 8.0), (14.0, 10.0)] {
        CGContext::fill_rect(
            Some(&ctx),
            CGRect::new(CGPoint::new(4.0, y), CGSize::new(len, 2.2)),
        );
    }
    render::draw_arrow(
        &ctx,
        Point::new(15.0, 3.0),
        Point::new(26.0, 13.0),
        [0.0, 0.0, 0.0, 1.0],
        2.6,
    );
    let image = CGBitmapContextCreateImage(Some(&ctx)).expect("icon image");
    NSImage::initWithCGImage_size(NSImage::alloc(), &image, CGSize::new(15.0, 15.0))
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
                    CGSize::new(INITIAL_SIZE.width, INITIAL_SIZE.height + TOOLBAR_RESERVED),
                ),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // Borderless chrome: rounded corners come from the content view's
        // layer clip, the system shadow follows the rounded alpha mask, and
        // the window background stays transparent under it.
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        // The dark HUD panel, the tints and the label all resolve against
        // the dark appearance.
        if let Some(dark) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }) {
            window.setAppearance(Some(&dark));
        }
        // Toolbar hover styling rides on mouse-moved events.
        window.setAcceptsMouseMovedEvents(true);
        // The window lives in the app controller for the whole process;
        // closing only hides it, the next shot reuses it (no second window).
        // SAFETY: plain setter on the freshly created window.
        unsafe {
            window.setReleasedWhenClosed(false);
        }

        // The floating toolbar panel at the bottom, Monosnap-style: a
        // rounded vibrancy pill carrying the ◄/► leafing buttons, the file
        // name (the borderless window has no system title), the color
        // swatch with its preset popover, the stroke-width slider and the
        // tools in Monosnap order.
        let panel = ToolbarPanel::new(mtm);
        panel.setMaterial(NSVisualEffectMaterial::HUDWindow);
        panel.setWantsLayer(true);
        if let Some(layer) = panel.layer() {
            layer.setCornerRadius(WINDOW_RADIUS);
            layer.setMasksToBounds(true);
        }
        // Hover styling: one tracking area over the panel, owned by the
        // canvas view — it holds the button list and the active tool.
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                CGRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(&*view),
                None,
            )
        };
        panel.addTrackingArea(&tracking);
        let toolbar = NSStackView::new(mtm);
        toolbar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        toolbar.setSpacing(6.0);
        let mut tool_buttons = Vec::new();
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
            tool_buttons.push((tag, button));
        }
        let file_label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        file_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        file_label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        // Long file names truncate instead of stretching the window's
        // minimum width.
        file_label.setContentCompressionResistancePriority_forOrientation(
            250.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        toolbar.addArrangedSubview(&file_label);
        toolbar.setCustomSpacing_afterView(14.0, &file_label);
        // The color swatch: a circle showing the current color (no symbol);
        // the popover with the presets opens on click.
        // SAFETY: class convenience constructor, plain setters afterwards.
        let color_button = unsafe {
            NSButton::buttonWithTitle_target_action(&NSString::from_str(""), None, None, mtm)
        };
        color_button.setTag(COLOR_BUTTON_TAG);
        color_button.setFocusRingType(NSFocusRingType::None);
        color_button.setRefusesFirstResponder(true);
        color_button.setBordered(false);
        color_button.setWantsLayer(true);
        if let Some(layer) = color_button.layer() {
            layer.setCornerRadius(10.0);
            // A hairline outline keeps light presets visible on the panel.
            layer.setBorderWidth(1.0);
            layer.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
        }
        color_button.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(20.0, 20.0)));
        // SAFETY: target is this canvas view, alive for the process.
        unsafe {
            color_button.setTarget(Some(&view));
            color_button.setAction(Some(sel!(colorClicked:)));
        }
        toolbar.addArrangedSubview(&color_button);
        // The custom stroke-width slider: track, accent fill, round knob.
        let slider = WidthSliderView::new(mtm);
        // SAFETY: target is this canvas view, alive for the process; the
        // action reads `doubleValue` off the sender.
        unsafe {
            slider.set_target(Some(&*view));
            slider.set_action(Some(sel!(widthChanged:)));
        }
        // The width control needs room to drag precisely.
        slider.setFrame(CGRect::new(
            CGPoint::ZERO,
            CGSize::new(STROKE_SLIDER_WIDTH_PT, 24.0),
        ));
        toolbar.addArrangedSubview(&slider);
        for tool in Tool::all() {
            let button = match tool {
                Tool::Callout => icon_button(mtm, callout_icon(), tool.tag()),
                other => symbol_button(mtm, other.symbol(), other.tag()),
            };
            // SAFETY: target is this canvas view, alive for the process.
            unsafe {
                button.setTarget(Some(&view));
                button.setAction(Some(sel!(toolClicked:)));
            }
            toolbar.addArrangedSubview(&button);
            tool_buttons.push((tool.tag(), button));
        }

        // Layout: the editing toolbar pinned to the bottom, the canvas
        // filling everything above it. The flexible max-Y margin keeps the
        // bottom bar down when the window grows; the canvas grows into the
        // freed space.
        let content = window.contentView().expect("content view");
        let size = content.bounds().size;
        // The container clips its subviews to the rounded window shape.
        content.setWantsLayer(true);
        if let Some(layer) = content.layer() {
            layer.setCornerRadius(WINDOW_RADIUS);
            layer.setMasksToBounds(true);
        }
        // The window cannot shrink below the floating panel: the stack's
        // fitting width plus the stack insets and the panel's window
        // margins.
        let min_width = toolbar.fittingSize().width + 24.0 + 2.0 * TOOLBAR_MARGIN;
        window.setContentMinSize(CGSize::new(min_width, 0.0));
        view.ivars().min_width.set(min_width);
        panel.setFrame(CGRect::new(
            CGPoint::new(TOOLBAR_MARGIN, TOOLBAR_MARGIN),
            CGSize::new(size.width - 2.0 * TOOLBAR_MARGIN, TOOLBAR_HEIGHT),
        ));
        panel.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        content.addSubview(&panel);
        toolbar.setFrame(CGRect::new(
            CGPoint::new(12.0, 4.0),
            CGSize::new(size.width - 2.0 * TOOLBAR_MARGIN - 24.0, TOOLBAR_HEIGHT - 8.0),
        ));
        toolbar.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        panel.addSubview(&toolbar);
        let canvas_height = size.height - TOOLBAR_RESERVED;
        view.setFrame(CGRect::new(
            CGPoint::new(0.0, TOOLBAR_RESERVED),
            CGSize::new(size.width, canvas_height),
        ));
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview(&view);
        // Keys land on the canvas; the toolbar buttons refuse first
        // responder, so the window never opens with a focused button.
        window.setInitialFirstResponder(Some(&view));

        *view.ivars().tool_buttons.borrow_mut() = tool_buttons;
        *view.ivars().color_button.borrow_mut() = Some(color_button);
        *view.ivars().file_label.borrow_mut() = Some(file_label);
        *view.ivars().width_slider.borrow_mut() = Some(slider);
        *view.ivars().toolbar_panel.borrow_mut() = Some(panel);
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
