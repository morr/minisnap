//! The editor window: one reusable `NSWindow` showing the open shot — the
//! base frame plus annotations, drawn with CoreGraphics into a custom
//! `NSView`, with an annotation toolbar in the left column. Selection
//! markers are drawn on top in the canvas and never become part of a saved
//! composite.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    define_class, extern_methods, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker,
    MainThreadOnly, Message,
};
use objc2_app_kit::{
    NSAnimationContext, NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua,
    NSAppearanceNameDarkAqua, NSApplication, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSBezierPath, NSBox, NSBoxType, NSButton, NSColor, NSControl, NSControlTextEditingDelegate,
    NSCursor, NSEvent, NSEventModifierFlags, NSFocusRingType, NSFont, NSFontWeightRegular,
    NSGraphicsContext, NSImage, NSImageSymbolConfiguration, NSPasteboard, NSPasteboardTypeString,
    NSPopover, NSPopoverDelegate, NSScreen, NSSlider, NSStackView, NSTextAlignment, NSTextField,
    NSTextFieldDelegate, NSTextView, NSTitlePosition, NSTrackingArea, NSTrackingAreaOptions,
    NSUserInterfaceLayoutOrientation, NSView, NSViewController, NSWindow, NSWindowButton,
    NSWindowTitleVisibility, NSWindowStyleMask, NSWorkspace,
};
use objc2_app_kit::NSAccessibility;
use objc2_quartz_core::CALayer;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreateImage, CGImage, CGContext};
use objc2_foundation::{NSString, NSNotification, NSRectEdge};

use crate::editor::hit_test::{Hit, text_bounds, top_hit};
use crate::editor::text;
use crate::editor::{
    name_density, render, set_endpoint, shift_element, Command, Element, ElementKind, Endpoint,
    OpenShot, Point, Rect, Rgba, Size,
};

/// Checkerboard cell of the transparent-backdrop fill, in points: the
/// light checker image editors draw behind transparent areas.
const CHECKER_CELL_PT: f64 = 8.0;


/// Selection marker radius, in view points (constant on screen).
const MARKER_RADIUS_PT: f64 = 4.5;

/// Background of the whole window and the toolbar column: the flat light
/// gray (#F2F2F2) of the zcode chrome — no vibrancy anywhere.
const WINDOW_BG: [f64; 4] = [242.0 / 255.0, 242.0 / 255.0, 242.0 / 255.0, 1.0];

/// Toolbar strip height, in points: the reference's roomier air above
/// the buttons and the shorter one between them and the card below.
const TOOLBAR_HEIGHT: f64 = 36.0;

/// Air above the toolbar buttons, in points: the top padding of the
/// reference strip.
const BAR_TOP_PAD: f64 = 9.0;

/// Leading padding of the toolbar row, in points: the cluster of three
/// visible traffic lights ends at ~64 pt (the yellow button's right
/// edge), plus a gap so the controls next in the row never run under it.
/// The miniaturize and zoom buttons are hidden today; the pad is where
/// they come back (editor-system-buttons).
const BAR_LEADING_PAD: f64 = 70.0;

/// Trailing padding of the toolbar row, in points.
const BAR_TRAILING_PAD: f64 = 12.0;

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

/// Default stroke width for new elements, in image pixels.
const DEFAULT_STROKE_WIDTH: f64 = 4.0;

/// Range of the stroke-width slider (and of authored widths), in image
/// pixels.
const STROKE_MIN: f64 = 1.0;
const STROKE_MAX: f64 = 12.0;

/// Width of the stroke-width slider in the toolbar, in points.
const STROKE_SLIDER_WIDTH_PT: f64 = 90.0;

/// Height of the stroke-width slider in the toolbar, in points. Reported
/// as the view's intrinsic content size: the toolbar stack sizes arranged
/// views by autolayout, so an explicit frame would be overwritten.
const SLIDER_HEIGHT: f64 = 24.0;

/// Knob radius of the custom stroke-width slider, in points.
const SLIDER_KNOB_RADIUS: f64 = 6.5;

/// Height of the custom stroke-width slider's track, in points.
const SLIDER_TRACK_HEIGHT: f64 = 3.0;

/// Wash behind the active tool's button: the system accent at this alpha.
const ACTIVE_WASH: f64 = 0.25;

/// The part of the slider track ahead of the knob's reach: a faint black
/// line reading as recessed on the light column (the dark theme's white
/// one blended into the light bar).
const TRACK_UNFILLED: f64 = 0.15;

/// Alpha of the slider knob's soft shadow.
const KNOB_SHADOW: f64 = 0.35;

/// A drag shorter than this (in image pixels) is a plain click, not a
/// drawn element.
const MIN_DRAG_PX: f64 = 4.0;

/// Font size of new text and callout elements, in image pixels.
const DEFAULT_TEXT_SIZE_PX: f64 = 32.0;

/// Gap between a new callout's text block and the arrow base it sits beside,
/// in image pixels.
const CALLOUT_TEXT_OFFSET_PX: f64 = 16.0;

/// Minimum width of the callout field editor, in view points: only wide
/// enough for the caret, so an empty field never reaches over the arrow.
const CALLOUT_EDITOR_MIN_WIDTH_PT: f64 = 30.0;

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

/// Toolbar button side, in points: the hover tile of the reference
/// toolbar — measured off the zcode screenshot, calibrated by the 12 pt
/// traffic lights (tile 22×22, uniform 6 pt step). The glyph stays
/// smaller inside it, so the plate reads with air around the icon.
const TOOLBAR_BUTTON_SIZE: CGSize = CGSize::new(22.0, 22.0);

/// Spacing between adjacent toolbar controls, in points: the uniform
/// step of the reference toolbar (all gaps equal, the leafing pair
/// included), measured off the zcode screenshot.
const BAR_SPACING: f64 = 6.0;

/// Spacing around the zone dividers and between the tool groups, in
/// points: the wider gap that separates the toolbar's zones.
const BAR_ZONE_SPACING: f64 = 12.0;

/// Tint of an inactive toolbar symbol: near-black (#262626), readable on
/// the light column whatever the surrounding colors are.
const ICON_TINT: [f64; 4] = [38.0 / 255.0, 38.0 / 255.0, 38.0 / 255.0, 1.0];

/// Hover background of a toolbar button: a faint black wash, the light
/// counterpart of the dark theme's white wash.
const HOVER_BG: [f64; 4] = [0.0, 0.0, 0.0, 0.06];

/// Diameter of a color swatch's visible circle, in points: the disc sits
/// inset in the swatch button, at the scale of the icon glyphs, so the
/// swatch reads as one more toolbar button rather than a big dot.
const SWATCH_DISC: f64 = 18.0;

/// Side of a palette preset swatch in the color popover, in points: the
/// minimum pointer target a pointer-driven control is expected to meet.
const PALETTE_SWATCH_SIZE: f64 = 24.0;

/// Resting outline of a color swatch: dark enough to mark the lightest
/// presets (white, yellow) against the light column and popover at the 3:1
/// boundary contrast graphics are expected to reach, faint enough not to
/// read as a highlight.
const SWATCH_RING: [f64; 4] = [0.0, 0.0, 0.0, 0.15];

/// Hover outline of a color swatch: the ring darkens on hover instead of
/// the fill — the fill is the color itself.
const SWATCH_RING_HOVER: [f64; 4] = [0.0, 0.0, 0.0, 0.3];

/// Crossfade length of toolbar plate changes (hover, tool highlight), in
/// seconds. Short enough that feedback stays responsive; the database rule
/// is only that instant swaps are the anti-pattern.
const PLATE_TRANSITION: f64 = 0.15;

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
    /// The stroke the arrow was drawn with (the width slider's value).
    width: f64,
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

    /// Human-readable name for the button's tooltip and accessibility
    /// label: icon-only buttons carry no visible text, and the SF Symbol
    /// name is not a name a person can read.
    fn name(self) -> &'static str {
        match self {
            Tool::Arrow => "Arrow",
            Tool::Rect => "Rectangle",
            Tool::Line => "Line",
            Tool::Pen => "Freehand",
            Tool::Text => "Text",
            Tool::Callout => "Callout",
            Tool::Blur => "Blur",
            Tool::Crop => "Crop",
            Tool::Resize => "Resize",
        }
    }

    fn from_tag(tag: isize) -> Option<Tool> {
        Tool::all().into_iter().find(|tool| tool.tag() == tag)
    }
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
    /// The left toolbar column: an invisible surface that drags the window
    /// when a click lands on its free area — the canvas behind it paints
    /// the light gray both the column and the window read as. Controls
    /// consume their own clicks, so they never reach this handler.
    #[unsafe(super(NSView))]
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

        #[unsafe(method(intrinsicContentSize))]
        fn __intrinsic_content_size(&self) -> CGSize {
            // No intrinsic size means the toolbar stack collapses the
            // slider to zero width — the size must live on the view.
            CGSize::new(STROKE_SLIDER_WIDTH_PT, SLIDER_HEIGHT)
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
        // The recessed track behind the fill: black at a low alpha, so it
        // stays visible on the light column.
        CGContext::set_rgb_fill_color(Some(&cg), 0.0, 0.0, 0.0, TRACK_UNFILLED);
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
        let shadow = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, KNOB_SHADOW);
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
    /// Toolbar buttons (tool tag → button) for the active highlight.
    tool_buttons: RefCell<Vec<(isize, Retained<NSButton>)>>,
    /// The color-swatch toolbar button (its bezel shows the current color).
    color_button: RefCell<Option<Retained<NSButton>>>,
    /// The open color popover, if any.
    color_popover: RefCell<Option<Retained<NSPopover>>>,
    /// The stroke-width slider (its knob mirrors the selection's width).
    width_slider: RefCell<Option<Retained<WidthSliderView>>>,
    /// Debug tuner rows: each arrow-param slider with its value field
    /// (position matches ARROW_PARAM_SPECS; empty without the tuner).
    arrow_param_rows: RefCell<Vec<(Retained<NSSlider>, Retained<NSTextField>)>>,
    /// The floating toolbar panel (hover hit-testing target).
    toolbar_panel: RefCell<Option<Retained<ToolbarPanel>>>,
    /// The smallest content width that fits the whole toolbar.
    min_width: Cell<f64>,
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
                tool_buttons: RefCell::new(Vec::new()),
                color_button: RefCell::new(None),
                color_popover: RefCell::new(None),
                width_slider: RefCell::new(None),
                arrow_param_rows: RefCell::new(Vec::new()),
                toolbar_panel: RefCell::new(None),
                min_width: Cell::new(0.0),
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

        #[unsafe(method(arrowParamChanged:))]
        fn __arrow_param_changed(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is a tuner slider; `tag` and `doubleValue`
            // are its accessors.
            let tag: isize = unsafe { msg_send![sender, tag] };
            let value: f64 = unsafe { msg_send![sender, doubleValue] };
            let Some(spec) = ARROW_PARAM_SPECS.get(tag as usize) else {
                return;
            };
            let mut params = render::arrow_params();
            (spec.set)(&mut params, value);
            render::set_arrow_params(params);
            if let Some((_, field)) = self.ivars().arrow_param_rows.borrow().get(tag as usize) {
                field.setStringValue(&NSString::from_str(&format!("{value:.2}")));
            }
            // Instant feedback: the canvas re-renders every element from
            // the live params on each draw. No command is involved, so
            // nothing enters the undo journal or the session files.
            self.redraw();
        }

        #[unsafe(method(arrowTunerReset:))]
        fn __arrow_tuner_reset(&self, _sender: Option<&AnyObject>) {
            let defaults = render::ArrowParams::DEFAULT;
            render::set_arrow_params(defaults);
            let rows = self.ivars().arrow_param_rows.borrow();
            for (spec, (slider, field)) in ARROW_PARAM_SPECS.iter().zip(rows.iter()) {
                let value = (spec.get)(defaults);
                slider.setDoubleValue(value);
                field.setStringValue(&NSString::from_str(&format!("{value:.2}")));
            }
            self.redraw();
        }

        #[unsafe(method(arrowParamReset:))]
        fn __arrow_param_reset(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is a per-row reset button; `tag` is its
            // accessor.
            let tag: isize = unsafe { msg_send![sender, tag] };
            let Some(spec) = ARROW_PARAM_SPECS.get(tag as usize) else {
                return;
            };
            // Only this knob returns to its shipped default; the rest of
            // the tuning session stays.
            let defaults = render::ArrowParams::DEFAULT;
            let value = (spec.get)(defaults);
            let mut params = render::arrow_params();
            (spec.set)(&mut params, value);
            render::set_arrow_params(params);
            if let Some((slider, field)) = self.ivars().arrow_param_rows.borrow().get(tag as usize)
            {
                slider.setDoubleValue(value);
                field.setStringValue(&NSString::from_str(&format!("{value:.2}")));
            }
            self.redraw();
        }

        #[unsafe(method(arrowPromptCopy:))]
        fn __arrow_prompt_copy(&self, _sender: Option<&AnyObject>) {
            // The tuner's whole point: carry the tuned values out of the
            // app and into the coding agent that bakes them in as the new
            // defaults.
            let text = NSString::from_str(&arrow_params_prompt(render::arrow_params()));
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            // SAFETY: NSPasteboardTypeString is a plain extern NSString
            // constant.
            let ok = pasteboard.setString_forType(&text, unsafe { NSPasteboardTypeString });
            if !ok {
                eprintln!("editor: the pasteboard rejected the tuner prompt");
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
                        if let Some(window) = self.window() {
                            window.performWindowDragWithEvent(event);
                        }
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
        let width = (content_size.width + 2.0 * CARD_GAP).max(self.ivars().min_width.get());
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

    /// Show (or replace) the shot in the window: density, selection reset,
    /// full refit.
    fn display_shot(&self, shot: OpenShot) {
        let file_name = shot.file_name();
        // Any in-flight interaction belongs to the outgoing shot.
        self.discard_text_editor();
        self.exit_frame_edit();
        *self.ivars().shot.borrow_mut() = Some(shot);
        self.ivars().density.set(name_density(&file_name));
        self.ivars().selection.set(None);
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

    /// Restyle the controls under the cursor: a faint wash on the hovered
    /// toolbar button, a brighter outline on the hovered color swatch,
    /// nothing on the others. The active tool's button and the swatch fill
    /// own their backgrounds and are skipped. The toolbar panel lives in
    /// the editor window and the palette presets in the popover's own
    /// window, so the event's window decides which surface is under the
    /// cursor; `None` (the cursor left both) clears both.
    fn hover_at(&self, event: Option<&NSEvent>) {
        let (toolbar_hit, preset_hit) = match event {
            None => (None, None),
            Some(event) => {
                let point = event.locationInWindow();
                // Compared by window number: NSEvent::window needs a
                // MainThreadMarker, the number needs nothing.
                let editor_number = self.window().map(|window| window.windowNumber());
                let in_editor_window = editor_number == Some(event.windowNumber());
                if in_editor_window {
                    (self.panel_button_at(point), None)
                } else {
                    (None, self.palette_button_at(point))
                }
            }
        };
        let active = self.ivars().tool.get().map(|tool| tool.tag());
        // The active tool's accent background reads as a highlight already.
        let toolbar_hit = if toolbar_hit.is_some() && toolbar_hit == active {
            None
        } else {
            toolbar_hit
        };
        let hover = NSColor::colorWithSRGBRed_green_blue_alpha(
            HOVER_BG[0],
            HOVER_BG[1],
            HOVER_BG[2],
            HOVER_BG[3],
        );
        let ring = NSColor::colorWithSRGBRed_green_blue_alpha(
            SWATCH_RING[0],
            SWATCH_RING[1],
            SWATCH_RING[2],
            SWATCH_RING[3],
        );
        let ring_hover = NSColor::colorWithSRGBRed_green_blue_alpha(
            SWATCH_RING_HOVER[0],
            SWATCH_RING_HOVER[1],
            SWATCH_RING_HOVER[2],
            SWATCH_RING_HOVER[3],
        );
        animate_plate_changes(|| {
            for (tag, button) in self.ivars().tool_buttons.borrow().iter() {
                if Some(*tag) == active || *tag == COLOR_BUTTON_TAG {
                    continue;
                }
                if let Some(layer) = button.layer() {
                    let bg: Option<&NSColor> = if Some(*tag) == toolbar_hit {
                        Some(&hover)
                    } else {
                        None
                    };
                    layer.setBackgroundColor(bg.map(|color| color.CGColor()).as_deref());
                }
            }
            // The toolbar swatch shows the current color as its fill; its
            // hover feedback is the outline.
            if let Some(button) = self.ivars().color_button.borrow().as_ref() {
                if let Some(disc) = Self::swatch_disc(button) {
                    let hover_ring = if toolbar_hit == Some(COLOR_BUTTON_TAG) {
                        &ring_hover
                    } else {
                        &ring
                    };
                    disc.setBorderColor(Some(&hover_ring.CGColor()));
                }
            }
            for button in self.palette_swatch_buttons() {
                if let Some(layer) = button.layer() {
                    let hover_ring = if Some(button.tag()) == preset_hit {
                        &ring_hover
                    } else {
                        &ring
                    };
                    layer.setBorderColor(Some(&hover_ring.CGColor()));
                }
            }
        });
    }

    /// Tag of the toolbar-panel button under a window-space point. The
    /// buttons are matched by frame, not by `hitTest`: the system-hosted
    /// buttons of recent macOS skip the classic hit-testing path (hover
    /// silently found nothing), while their frames are plain geometry.
    fn panel_button_at(&self, point: CGPoint) -> Option<isize> {
        let inside = |rect: CGRect| {
            point.x >= rect.origin.x
                && point.x <= rect.origin.x + rect.size.width
                && point.y >= rect.origin.y
                && point.y <= rect.origin.y + rect.size.height
        };
        for (tag, button) in self.ivars().tool_buttons.borrow().iter() {
            if inside(button.convertRect_toView(button.bounds(), None)) {
                return Some(*tag);
            }
        }
        if let Some(button) = self.ivars().color_button.borrow().as_ref() {
            if inside(button.convertRect_toView(button.bounds(), None)) {
                return Some(COLOR_BUTTON_TAG);
            }
        }
        None
    }

    /// Tag of the palette preset under a window-space point, if the
    /// popover is open.
    fn palette_button_at(&self, point: CGPoint) -> Option<isize> {
        let popover = self.ivars().color_popover.borrow().clone()?;
        if !popover.isShown() {
            return None;
        }
        let controller = popover.contentViewController()?;
        let stack = controller.view();
        let hit = stack.hitTest(stack.convertPoint_fromView(point, None))?;
        hit.downcast_ref::<NSButton>().map(|button| button.tag() as isize)
    }

    /// The preset swatch buttons of the open popover.
    fn palette_swatch_buttons(&self) -> Vec<Retained<NSButton>> {
        let Some(popover) = self.ivars().color_popover.borrow().clone() else {
            return Vec::new();
        };
        if !popover.isShown() {
            return Vec::new();
        }
        let Some(controller) = popover.contentViewController() else {
            return Vec::new();
        };
        controller
            .view()
            .subviews()
            .iter()
            .filter_map(|view| view.downcast_ref::<NSButton>().map(|button| button.retain()))
            .collect()
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
        // Only the plate backgrounds crossfade; the symbol tint swap is
        // instant (not a layer property).
        animate_plate_changes(|| {
            for (tag, button) in self.ivars().tool_buttons.borrow().iter() {
                if Some(*tag) == active {
                    let bg = accent.colorWithAlphaComponent(ACTIVE_WASH);
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
        });
    }

    /// The swatch button's color disc: the one sublayer its layer carries.
    fn swatch_disc(button: &NSButton) -> Option<Retained<CALayer>> {
        let layer = button.layer()?;
        // SAFETY: plain layer-tree read, no external invariants.
        let layers = unsafe { layer.sublayers() }?;
        Some(layers.objectAtIndexedSubscript(0))
    }

    /// Show the current color on the toolbar swatch circle.
    fn update_color_swatch(&self) {
        let button = self.ivars().color_button.borrow().clone();
        let Some(button) = button.as_ref() else {
            return;
        };
        let Some(disc) = Self::swatch_disc(button) else {
            return;
        };
        let [r, g, b, a] = self.ivars().color.get();
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        disc.setBackgroundColor(Some(&color.CGColor()));
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
                layer.setCornerRadius(PALETTE_SWATCH_SIZE / 2.0);
                // The same bright resting outline as the toolbar swatch's
                // disc: dark presets stay visible on the popover background.
                layer.setBorderWidth(1.0);
                layer.setBorderColor(Some(
                    &NSColor::colorWithSRGBRed_green_blue_alpha(
                        SWATCH_RING[0],
                        SWATCH_RING[1],
                        SWATCH_RING[2],
                        SWATCH_RING[3],
                    )
                    .CGColor(),
                ));
            }
            // A borderless button with an empty title has no intrinsic
            // size: without constraints the popover's stack collapses the
            // preset to nothing, same as the toolbar swatch.
            button.setTranslatesAutoresizingMaskIntoConstraints(false);
            button.widthAnchor()
                .constraintEqualToConstant(PALETTE_SWATCH_SIZE)
                .setActive(true);
            button.heightAnchor()
                .constraintEqualToConstant(PALETTE_SWATCH_SIZE)
                .setActive(true);
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
        // Swatch hover rides the same owner as the toolbar tracking: one
        // tracking area over the stack, and `hover_at` restyles whichever
        // preset the cursor is over.
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                CGRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(&*self),
                None,
            )
        };
        content.addTrackingArea(&tracking);
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

/// Run toolbar plate changes inside an implicit-animation group: hover and
/// highlight crossfade instead of popping (instant state changes are a UX
/// anti-pattern). Only layer properties animate; a color tint applied to a
/// symbol stays instant. When the system asks to reduce motion the change
/// applies directly. Core Animation replaces a conflicting in-flight
/// transition, and the final state is whatever the block sets — nothing
/// depends on the animation finishing.
fn animate_plate_changes(change: impl FnOnce()) {
    let reduce_motion = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
    if reduce_motion {
        change();
        return;
    }
    NSAnimationContext::beginGrouping();
    let context = NSAnimationContext::currentContext();
    context.setDuration(PLATE_TRANSITION);
    context.setAllowsImplicitAnimation(true);
    change();
    NSAnimationContext::endGrouping();
}

/// The stroke width of an element the width slider styles; None for kinds
/// without one (text, callout, blur).
fn stroke_width_of(kind: &ElementKind) -> Option<f64> {
    match kind {
        ElementKind::Arrow { width, .. }
        | ElementKind::Rect { width, .. }
        | ElementKind::Line { width, .. }
        | ElementKind::Pen { width, .. }
        | ElementKind::Callout { width, .. } => Some(*width),
        _ => None,
    }
}

/// Top-left of a new callout's text block (in image pixels): beside the
/// arrow's base, on the side away from the tip, so the growing text never
/// runs over the pointer. Along the dominant axis the block clears the base
/// by `CALLOUT_TEXT_OFFSET_PX`; across it, the block is centered on the
/// base. The whole block is clamped inside the frame (4 px margin) — near an
/// edge the block shifts inward, even if that brings it over the arrow.
fn callout_anchor(base: Point, tip: Point, block_w: f64, block_h: f64, frame: Rect) -> Point {
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
fn symbol_button(
    mtm: MainThreadMarker,
    symbol: &str,
    title: &str,
    tag: isize,
) -> Retained<NSButton> {
    icon_button(mtm, symbol_image(symbol, title), title, tag)
}

/// The SF Symbol image at the shared icon size; the accessibility
/// description is the human-readable button name, not the symbol name.
fn symbol_image(symbol: &str, name: &str) -> Retained<NSImage> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        Some(&NSString::from_str(name)),
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

/// A vertical divider between the toolbar zones: a 1×18 pt view in the
/// system separator color, so it adapts to the light column. Not a button,
/// so the hover pass ignores it. The size rides on constraints: the
/// toolbar stack sizes arranged views by autolayout and would collapse a
/// frame-only view.
fn bar_separator(mtm: MainThreadMarker) -> Retained<NSView> {
    let view = NSView::initWithFrame(NSView::alloc(mtm), CGRect::ZERO);
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setBackgroundColor(Some(&NSColor::separatorColor().CGColor()));
    }
    view.widthAnchor()
        .constraintEqualToConstant(1.0)
        .setActive(true);
    view.heightAnchor()
        .constraintEqualToConstant(18.0)
        .setActive(true);
    view
}

/// A toolbar button showing `image` as a template (tinted like the SF
/// Symbols), with an empty title and the given tag. Bezel-less: the hover
/// wash and the active-tool accent are layer backgrounds. Toolbar buttons
/// never take first responder: no focus ring, keys stay with the canvas.
/// `title` becomes the tooltip and the accessibility label — an icon-only
/// button has no visible text for either to fall back on.
fn icon_button(
    mtm: MainThreadMarker,
    image: Retained<NSImage>,
    title: &str,
    tag: isize,
) -> Retained<NSButton> {
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
        layer.setCornerRadius(5.0);
    }
    // The 28×28 tile rides on constraints: the toolbar stack sizes
    // arranged views by autolayout and hugs a frame-only button down to
    // its glyph — the hover plate shrank with it.
    button.setTranslatesAutoresizingMaskIntoConstraints(false);
    button
        .widthAnchor()
        .constraintEqualToConstant(TOOLBAR_BUTTON_SIZE.width)
        .setActive(true);
    button
        .heightAnchor()
        .constraintEqualToConstant(TOOLBAR_BUTTON_SIZE.height)
        .setActive(true);
    let name = NSString::from_str(title);
    button.setToolTip(Some(&name));
    button.setAccessibilityLabel(Some(&name));
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

/// One knob of the arrow tuner: the `ArrowParams` field it reads and
/// writes, the slider's range, and the row label. Slider tags index this
/// table.
struct ArrowParamSpec {
    name: &'static str,
    min: f64,
    max: f64,
    get: fn(render::ArrowParams) -> f64,
    set: fn(&mut render::ArrowParams, f64),
}

/// The tuner rows, top to bottom — the main constants of the arrow
/// construction (`render::ArrowParams`). `half_head_w` stays above the
/// body half-width (1.1 stroke widths), so even a dragged-low slider
/// cannot invert the wing into the body.
const ARROW_PARAM_SPECS: &[ArrowParamSpec] = &[
    ArrowParamSpec {
        name: "head_len_w",
        min: 2.0,
        max: 8.0,
        get: |p| p.head_len_w,
        set: |p, v| p.head_len_w = v,
    },
    ArrowParamSpec {
        name: "half_head_w",
        min: 1.2,
        max: 4.0,
        get: |p| p.half_head_w,
        set: |p, v| p.half_head_w = v,
    },
    ArrowParamSpec {
        name: "notch_at",
        min: 0.4,
        max: 0.95,
        get: |p| p.notch_at,
        set: |p, v| p.notch_at = v,
    },
    ArrowParamSpec {
        name: "notch_bow",
        min: 0.0,
        max: 1.0,
        get: |p| p.notch_bow,
        set: |p, v| p.notch_bow = v,
    },
    ArrowParamSpec {
        name: "outer_bow",
        min: 0.0,
        max: 1.0,
        get: |p| p.outer_bow,
        set: |p, v| p.outer_bow = v,
    },
    ArrowParamSpec {
        name: "notch_round_w",
        min: 0.0,
        max: 0.8,
        get: |p| p.notch_round_w,
        set: |p, v| p.notch_round_w = v,
    },
    ArrowParamSpec {
        name: "light_toward_white",
        min: 0.0,
        max: 0.6,
        get: |p| p.light_toward_white,
        set: |p, v| p.light_toward_white = v,
    },
    ArrowParamSpec {
        name: "dark_shade",
        min: 0.3,
        max: 1.0,
        get: |p| p.dark_shade,
        set: |p, v| p.dark_shade = v,
    },
    ArrowParamSpec {
        name: "shadow_dx",
        min: -4.0,
        max: 4.0,
        get: |p| p.shadow_dx,
        set: |p, v| p.shadow_dx = v,
    },
    ArrowParamSpec {
        name: "shadow_dy",
        min: -4.0,
        max: 4.0,
        get: |p| p.shadow_dy,
        set: |p, v| p.shadow_dy = v,
    },
    ArrowParamSpec {
        name: "shadow_blur",
        min: 0.0,
        max: 8.0,
        get: |p| p.shadow_blur,
        set: |p, v| p.shadow_blur = v,
    },
    ArrowParamSpec {
        name: "shadow_alpha",
        min: 0.0,
        max: 1.0,
        get: |p| p.shadow_alpha,
        set: |p, v| p.shadow_alpha = v,
    },
];

/// The text the tuner's `prompt` button puts on the clipboard: the tuned
/// values plus a ready-to-paste instruction for the coding agent (ZCode)
/// to bake them in as the new defaults, backing the old ones up to /tmp.
fn arrow_params_prompt(params: render::ArrowParams) -> String {
    let mut out = String::from(
        "Примени настроенные в debug-панели значения параметров стрелки \
         как новые дефолты в minisnap.\n\n",
    );
    out.push_str("Значения (все 12 полей ArrowParams):\n");
    for spec in ARROW_PARAM_SPECS {
        out.push_str(&format!("{} = {:.2}\n", spec.name, (spec.get)(params)));
    }
    out.push_str("\nШаги:\n");
    out.push_str(
        "1. Забекапь прежние значения из ArrowParams::DEFAULT в \
         src/editor/render.rs в файл /tmp/minisnap-arrow-defaults-backup-<время>.txt — \
         те же 12 строк вида `имя = значение`.\n",
    );
    out.push_str(
        "2. Подставь значения из списка выше в ArrowParams::DEFAULT. Кроме \
         DEFAULT ничего в коде не меняй: живые параметры — рантайм-крутилки \
         тюнера, новыми дефолтами становятся только значения DEFAULT.\n",
    );
    out.push_str(
        "3. Прогони cargo test: стрелочные тесты должны остаться зелёными. \
         Если пиксельный тест стрелки падает из-за изменившегося силуэта — \
         сдвинь его контрольные точки осмысленно, не ослабляй пороги.\n",
    );
    out.push_str(
        "4. Закоммить по скиллу git: только src/editor/render.rs; сообщение \
         вроде `set the arrow defaults to the tuned look`.\n",
    );
    out
}

/// The arrow debug tuner: a translucent panel of sliders over the canvas'
/// top-left corner, editing the live arrow params (`render::ARROW_PARAMS`)
/// with instant redraw. Built only under `MINISNAP_ARROW_DEBUG` — a tuning
/// instrument, not shipped UI. Every view gets an explicit frame in rows
/// of fixed height: a stack-view panel sized from `fittingSize` proved
/// fragile and dropped its bottom row of buttons.
fn arrow_tuner_panel(mtm: MainThreadMarker, view: &EditorView) -> Retained<NSBox> {
    // Row geometry, in points: label | slider | this-row reset | value.
    const TOP: f64 = 10.0;
    const BOTTOM: f64 = 10.0;
    const INSET: f64 = 12.0;
    const ROW_H: f64 = 20.0;
    const ROW_GAP: f64 = 2.0;
    const LABEL_W: f64 = 104.0;
    const SLIDER_X: f64 = INSET + LABEL_W + 4.0;
    const SLIDER_W: f64 = 88.0;
    const RESET_X: f64 = SLIDER_X + SLIDER_W + 4.0;
    const RESET_W: f64 = 22.0;
    const VALUE_X: f64 = RESET_X + RESET_W + 4.0;
    const VALUE_W: f64 = 44.0;
    const ACTIONS_GAP: f64 = 8.0;
    const ACTION_H: f64 = 24.0;
    const ACTION_W: f64 = 64.0;
    const W: f64 = VALUE_X + VALUE_W + INSET;
    let n = ARROW_PARAM_SPECS.len() as f64;
    let h = TOP + n * ROW_H + (n - 1.0) * ROW_GAP + ACTIONS_GAP + ACTION_H + BOTTOM;

    let panel = NSBox::new(mtm);
    // A classic drawn box, not a layer-backed view: on this macOS every
    // control is SwiftUI-hosted, and inline-hosted controls (NSButton)
    // stop compositing inside a view with a hand-configured layer — they
    // went invisible. NSBox paints its fill the old way and leaves the
    // control hosting alone.
    // NSBoxCustom: the Primary style draws a system material and ignores
    // the fill color; Custom is the type where fillColor actually paints.
    panel.setBoxType(NSBoxType::Custom);
    panel.setTitlePosition(NSTitlePosition::NoTitle);
    panel.setBorderWidth(0.0);
    panel.setFillColor(&NSColor::blackColor().colorWithAlphaComponent(0.7));
    panel.setCornerRadius(10.0);
    panel.setContentViewMargins(objc2_foundation::NSSize::new(0.0, 0.0));
    panel.setFrameSize(CGSize::new(W, h));
    let content_area = panel.contentView().expect("box content view");
    // The panel's buttons draw in the dark look whatever the system
    // appearance is: on the dark fill their titles read white and the
    // template icons stay light.
    // SAFETY: NSAppearanceNameDarkAqua is a plain extern NSString
    // constant.
    let dark_appearance =
        unsafe { NSAppearance::appearanceNamed(NSAppearanceNameDarkAqua) };
    let font = NSFont::monospacedSystemFontOfSize_weight(
        10.0,
        // SAFETY: NSFontWeightRegular is a plain extern CGFloat constant.
        unsafe { NSFontWeightRegular },
    );
    let params = render::arrow_params();
    let mut rows = Vec::new();
    for (tag, spec) in ARROW_PARAM_SPECS.iter().enumerate() {
        let y = h - TOP - ROW_H - tag as f64 * (ROW_H + ROW_GAP);
        let label = NSTextField::labelWithString(&NSString::from_str(spec.name), mtm);
        label.setFont(Some(&font));
        label.setTextColor(Some(&NSColor::whiteColor()));
        label.setFrame(CGRect::new(
            CGPoint::new(INSET, y + 3.0),
            CGSize::new(LABEL_W, 14.0),
        ));
        let value = (spec.get)(params);
        // SAFETY: class convenience constructor for the slider.
        let slider = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                value,
                spec.min,
                spec.max,
                Some(view),
                Some(sel!(arrowParamChanged:)),
                mtm,
            )
        };
        slider.setTag(tag as isize);
        // Sliders report on every tick of a drag, not only on release.
        slider.setContinuous(true);
        slider.setFocusRingType(NSFocusRingType::None);
        slider.setRefusesFirstResponder(true);
        slider.setFrame(CGRect::new(
            CGPoint::new(SLIDER_X, y + 1.0),
            CGSize::new(SLIDER_W, 18.0),
        ));
        // Reset for this one knob, tagged with the row.
        let reset_name = format!("reset {}", spec.name);
        let reset = icon_button(
            mtm,
            symbol_image("arrow.counterclockwise", &reset_name),
            &reset_name,
            tag as isize,
        );
        // SAFETY: target is this canvas view, alive for the process.
        unsafe {
            reset.setTarget(Some(view));
            reset.setAction(Some(sel!(arrowParamReset:)));
        }
        reset.setToolTip(Some(&NSString::from_str(&format!(
            "reset {} to its default",
            spec.name
        ))));
        if let Some(appearance) = &dark_appearance {
            reset.setAppearance(Some(appearance));
        }
        reset.setFrame(CGRect::new(
            CGPoint::new(RESET_X, y + 1.0),
            CGSize::new(RESET_W, 18.0),
        ));
        let field =
            NSTextField::labelWithString(&NSString::from_str(&format!("{value:.2}")), mtm);
        field.setFont(Some(&font));
        field.setTextColor(Some(&NSColor::whiteColor()));
        field.setAlignment(NSTextAlignment::Right);
        field.setFrame(CGRect::new(
            CGPoint::new(VALUE_X, y + 3.0),
            CGSize::new(VALUE_W, 14.0),
        ));
        content_area.addSubview(&label);
        content_area.addSubview(&slider);
        content_area.addSubview(&reset);
        content_area.addSubview(&field);
        rows.push((slider, field));
    }
    // Bottom actions: reset everything to the shipped defaults, or copy
    // the tuned values as a prompt that makes the coding agent bake them
    // in as the new defaults (the old ones backed up to /tmp).
    let action_button = |title: &str, action: objc2::runtime::Sel, x: f64| {
        // SAFETY: class convenience constructor, no external invariants.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(view),
                Some(action),
                mtm,
            )
        };
        button.setFont(Some(&font));
        button.setFocusRingType(NSFocusRingType::None);
        button.setRefusesFirstResponder(true);
        if let Some(appearance) = &dark_appearance {
            button.setAppearance(Some(appearance));
        }
        button.setFrame(CGRect::new(
            CGPoint::new(x, BOTTOM),
            CGSize::new(ACTION_W, ACTION_H),
        ));
        button
    };
    let reset = action_button("reset", sel!(arrowTunerReset:), INSET);
    reset.setToolTip(Some(&NSString::from_str(
        "reset all the arrow params to the shipped defaults",
    )));
    let prompt = action_button("prompt", sel!(arrowPromptCopy:), INSET + ACTION_W + 8.0);
    prompt.setToolTip(Some(&NSString::from_str(
        "copy the tuned params and a prompt that bakes them in as the \
         new defaults",
    )));
    content_area.addSubview(&reset);
    content_area.addSubview(&prompt);
    *view.ivars().arrow_param_rows.borrow_mut() = rows;
    panel
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
                    | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // Electron-style hidden inset: a titled window with a transparent,
        // hidden titlebar — the system traffic lights draw over the toolbar
        // column, which stays the only chrome of the window.
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setHasShadow(true);
        // An accessory app has no dock icon: a miniaturized window would be
        // unreachable, and zoom is meaningless — the window is refit to
        // every shot anyway. Hide both standard buttons; close stays.
        // Bringing the three lights alive is editor-system-buttons.
        for role in [NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton] {
            if let Some(button) = window.standardWindowButton(role) {
                button.setHidden(true);
            }
        }
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

        // The toolbar column along the left edge: an invisible surface
        // carrying the controls; the canvas behind it paints the light
        // gray the column reads as.
        let panel = ToolbarPanel::new(mtm);
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
        toolbar.setSpacing(BAR_SPACING);
        let mut tool_buttons = Vec::new();
        // The ◄/► leafing pair rides right of the traffic lights' leading
        // pad, tight together like the back/forward pair of the reference.
        let mut leaf_buttons = Vec::new();
        for (tag, symbol, title) in [
            (PAGE_PREV_TAG, "chevron.left", "Previous shot"),
            (PAGE_NEXT_TAG, "chevron.right", "Next shot"),
        ] {
            let button = symbol_button(mtm, symbol, title, tag);
            // SAFETY: target is this canvas view, alive for the process.
            unsafe {
                button.setTarget(Some(&view));
                button.setAction(Some(sel!(pageClicked:)));
            }
            toolbar.addArrangedSubview(&button);
            leaf_buttons.push(button.clone());
            tool_buttons.push((tag, button));
        }
        // Zones: leafing | color + width | tools, separated by 1 pt
        // dividers with a wider gap around them.
        toolbar.setCustomSpacing_afterView(BAR_ZONE_SPACING, &leaf_buttons[1]);
        let leading_sep = bar_separator(mtm);
        toolbar.addArrangedSubview(&leading_sep);
        toolbar.setCustomSpacing_afterView(BAR_ZONE_SPACING, &leading_sep);
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
        // The visible disc is an inset sublayer at the scale of the icon
        // glyphs; the button itself keeps the toolbar buttons' target size,
        // so the swatch reads as one more button. The bright outline keeps
        // dark presets (black, blue) distinguishable from the panel at
        // boundary contrast; on hover the outline brightens instead of the
        // fill (the fill is the color itself).
        let disc = CALayer::new();
        let inset = (TOOLBAR_BUTTON_SIZE.width - SWATCH_DISC) / 2.0;
        disc.setFrame(CGRect::new(
            CGPoint::new(inset, inset),
            CGSize::new(SWATCH_DISC, SWATCH_DISC),
        ));
        disc.setCornerRadius(SWATCH_DISC / 2.0);
        disc.setBorderWidth(1.0);
        disc.setBorderColor(Some(
            &NSColor::colorWithSRGBRed_green_blue_alpha(
                SWATCH_RING[0],
                SWATCH_RING[1],
                SWATCH_RING[2],
                SWATCH_RING[3],
            )
            .CGColor(),
        ));
        if let Some(layer) = color_button.layer() {
            layer.addSublayer(&disc);
        }
        // The swatch has no intrinsic size (borderless button, empty
        // title): its size must ride on constraints, or the toolbar
        // stack collapses it to nothing.
        color_button.setTranslatesAutoresizingMaskIntoConstraints(false);
        color_button
            .widthAnchor()
            .constraintEqualToConstant(TOOLBAR_BUTTON_SIZE.width)
            .setActive(true);
        color_button
            .heightAnchor()
            .constraintEqualToConstant(TOOLBAR_BUTTON_SIZE.height)
            .setActive(true);
        let name = NSString::from_str("Color");
        color_button.setToolTip(Some(&name));
        color_button.setAccessibilityLabel(Some(&name));
        // SAFETY: target is this canvas view, alive for the process.
        unsafe {
            color_button.setTarget(Some(&view));
            color_button.setAction(Some(sel!(colorClicked:)));
        }
        // The slider sizes itself through its intrinsicContentSize — the
        // toolbar stack reads it for layout.
        let slider = WidthSliderView::new(mtm);
        // SAFETY: target is this canvas view, alive for the process; the
        // action reads `doubleValue` off the sender.
        unsafe {
            slider.set_target(Some(&*view));
            slider.set_action(Some(sel!(widthChanged:)));
        }
        toolbar.addArrangedSubview(&color_button);
        toolbar.addArrangedSubview(&slider);
        toolbar.setCustomSpacing_afterView(BAR_ZONE_SPACING, &slider);
        let trailing_sep = bar_separator(mtm);
        toolbar.addArrangedSubview(&trailing_sep);
        toolbar.setCustomSpacing_afterView(BAR_ZONE_SPACING, &trailing_sep);
        for tool in Tool::all() {
            let button = match tool {
                Tool::Callout => icon_button(mtm, callout_icon(), tool.name(), tool.tag()),
                other => symbol_button(mtm, other.symbol(), other.name(), other.tag()),
            };
            // SAFETY: target is this canvas view, alive for the process.
            unsafe {
                button.setTarget(Some(&view));
                button.setAction(Some(sel!(toolClicked:)));
            }
            toolbar.addArrangedSubview(&button);
            // Wider gaps between the tool groups: drawing | annotating |
            // transforming (Pen and Blur end their groups, Resize ends
            // the tools zone).
            if matches!(tool, Tool::Pen | Tool::Blur | Tool::Resize) {
                toolbar.setCustomSpacing_afterView(BAR_ZONE_SPACING, &button);
            }
            tool_buttons.push((tool.tag(), button));
        }

        // Layout: the toolbar strip across the top, the canvas filling
        // everything below it. The canvas paints nothing outside the card,
        // so the window background — the flat light gray — shows around
        // it and under the transparent strip.
        let content = window.contentView().expect("content view");
        let size = content.bounds().size;
        // The window cannot shrink below the bar: the leading pad for the
        // traffic lights, the row's fitting width, the trailing pad.
        let min_width = toolbar.fittingSize().width + BAR_LEADING_PAD + BAR_TRAILING_PAD;
        window.setContentMinSize(CGSize::new(min_width, 0.0));
        view.ivars().min_width.set(min_width);
        panel.setFrame(CGRect::new(
            CGPoint::new(0.0, size.height - TOOLBAR_HEIGHT),
            CGSize::new(size.width, TOOLBAR_HEIGHT),
        ));
        panel.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&panel);
        toolbar.setFrame(CGRect::new(
            CGPoint::new(
                BAR_LEADING_PAD,
                TOOLBAR_HEIGHT - BAR_TOP_PAD - TOOLBAR_BUTTON_SIZE.height / 2.0
                    - SLIDER_HEIGHT / 2.0,
            ),
            CGSize::new(size.width - BAR_LEADING_PAD - BAR_TRAILING_PAD, SLIDER_HEIGHT),
        ));
        toolbar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        panel.addSubview(&toolbar);
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
            let tuner = arrow_tuner_panel(mtm, &view);
            let fit = tuner.frame().size;
            tuner.setFrame(CGRect::new(
                CGPoint::new(8.0, size.height - TOOLBAR_HEIGHT - 8.0 - fit.height),
                fit,
            ));
            tuner.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
            content.addSubview(&tuner);
        }

        *view.ivars().tool_buttons.borrow_mut() = tool_buttons;
        *view.ivars().color_button.borrow_mut() = Some(color_button);
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

    #[test]
    fn arrow_prompt_carries_every_param_and_the_steps() {
        let params = render::ArrowParams::DEFAULT;
        let prompt = arrow_params_prompt(params);
        for spec in ARROW_PARAM_SPECS {
            let line = format!("{} = {:.2}\n", spec.name, (spec.get)(params));
            assert!(prompt.contains(&line), "prompt lacks {line:?}");
        }
        assert!(prompt.contains("ArrowParams::DEFAULT"));
        assert!(prompt.contains("/tmp"));
        assert!(prompt.contains("cargo test"));
    }

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
