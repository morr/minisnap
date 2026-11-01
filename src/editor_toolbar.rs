//! The editor toolbar and window chrome: the strip above the canvas with
//! the leafing pair, the file-name label, the color swatch, the stroke
//! width slider and the tool buttons. `ToolbarView` is the strip — its own
//! objc2 view with its own ivars (the precedent is `WidthSliderView`): it
//! hosts the controls, drags the window on free-area clicks, runs the
//! hover pass over its buttons and reports the user's intents as
//! `ToolbarEvent`s through the single callback. The window talks to it
//! only through this interface.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    define_class, extern_methods, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker,
    MainThreadOnly, Message,
};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSAppearance, NSAppearanceCustomization,
    NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSBox, NSBoxType, NSButton, NSColor, NSControl, NSEvent,
    NSEventMask, NSEventTrackingRunLoopMode, NSEventType, NSFocusRingType, NSFont,
    NSFontWeightRegular, NSGraphicsContext, NSImage, NSImageSymbolConfiguration,
    NSLayoutConstraintOrientation, NSLineBreakMode, NSPasteboard, NSPasteboardTypeString,
    NSPopover, NSPopoverDelegate, NSSlider, NSStackView, NSTextAlignment, NSTextField,
    NSTitlePosition, NSTrackingArea, NSTrackingAreaOptions, NSUserInterfaceLayoutOrientation,
    NSView, NSViewController, NSWindowButton,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreateImage, CGContext};
use objc2_foundation::{NSDate, NSString, NSNotification, NSRectEdge};
use objc2_quartz_core::CALayer;

use crate::editor::render;
use crate::editor::{Point, Rgba};
use crate::plates::{
    animate_plate_changes, center_toolbar_layers, disc, plate, DISC_LAYER_NAME, HOVER_BG,
    PLATE_LAYER_NAME, SWATCH_DISC, SWATCH_RING, SWATCH_RING_HOVER, TOOLBAR_PLATE_SIZE,
};

/// Toolbar strip height, in points: the reference's roomier air above
/// the buttons and the shorter one between them and the card below.
pub const TOOLBAR_HEIGHT: f64 = 36.0;

/// Default stroke width for new elements, in image pixels.
pub const DEFAULT_STROKE_WIDTH: f64 = 4.0;

/// Range of the stroke-width slider (and of authored widths), in image
/// pixels.
pub const STROKE_MIN: f64 = 1.0;
pub const STROKE_MAX: f64 = 12.0;

/// Width of the stroke-width slider in the toolbar, in points.
const STROKE_SLIDER_WIDTH_PT: f64 = 90.0;

/// Height of the stroke-width slider in the toolbar, in points. Reported
/// as the view's intrinsic content size: the toolbar stack sizes arranged
/// views by autolayout, so an explicit frame would be overwritten.
pub(crate) const SLIDER_HEIGHT: f64 = 24.0;

/// Knob radius of the custom stroke-width slider, in points.
const SLIDER_KNOB_RADIUS: f64 = 6.5;

/// Height of the custom stroke-width slider's track, in points.
const SLIDER_TRACK_HEIGHT: f64 = 3.0;

/// The part of the slider track ahead of the knob's reach: a faint black
/// line reading as recessed on the light column (the dark theme's white
/// one blended into the light bar).
const TRACK_UNFILLED: f64 = 0.15;

/// Alpha of the slider knob's soft shadow.
const KNOB_SHADOW: f64 = 0.35;

/// Air above the toolbar buttons, in points: the top padding of the
/// reference strip.
const BAR_TOP_PAD: f64 = 9.0;

/// Leading padding of the toolbar row, in points: the traffic lights'
/// hosting view ends at 70 pt (the zoom button's right frame edge), plus
/// the 15 pt gap the user asked for between the system buttons and the
/// leafing arrows.
const BAR_LEADING_PAD: f64 = 85.0;

/// Vertical distance from the window's top edge to the toolbar icon row's
/// axis, in points: the panel pads the row down from its top edge, and the
/// row centers on the nominal button half-height.
pub const TOOLBAR_ROW_AXIS_FROM_TOP: f64 = BAR_TOP_PAD + TOOLBAR_BUTTON_SIZE.height / 2.0;

/// Trailing padding of the toolbar row, in points.
const BAR_TRAILING_PAD: f64 = 12.0;

/// Toolbar tags of the ◄/► leafing buttons.
const PAGE_PREV_TAG: isize = -2;
const PAGE_NEXT_TAG: isize = -3;

/// Toolbar tag of the color-swatch button: the current color lives on the
/// disc sublayer, so the button's own layer carries the shared hover plate.
const COLOR_BUTTON_TAG: isize = -1;

/// Toolbar button side, in points: the layout cell of a toolbar button
/// (constraints the stack partially ignores; the button frame lands
/// near it either way).
const TOOLBAR_BUTTON_SIZE: CGSize = CGSize::new(22.0, 22.0);

/// Spacing between adjacent toolbar controls, in points: the uniform
/// step of the reference toolbar (all gaps equal, the leafing pair
/// included), measured off the zcode screenshot, then widened by 4 pt
/// on the user's request.
const BAR_SPACING: f64 = 10.0;

/// Spacing around the zone dividers and between the tool groups, in
/// points: the wider gap that separates the toolbar's zones.
const BAR_ZONE_SPACING: f64 = 12.0;

/// Fixed width of the file-name label block, in points: the widest
/// typical shot name measured at the label's 13 pt system font
/// (`MiniSnap_2026-11-01_15-46-36@2x.png`, 251 pt) plus 10% — enough
/// even for an `@10x` density marker (262 pt). One width for every
/// shot, so leafing never shifts the neighboring controls.
const FILE_LABEL_WIDTH: f64 = 276.0;

/// Tint of an inactive toolbar symbol: near-black (#262626), readable on
/// the light column whatever the surrounding colors are.
const ICON_TINT: [f64; 4] = [38.0 / 255.0, 38.0 / 255.0, 38.0 / 255.0, 1.0];

/// Wash behind the active tool's button: the system accent at this alpha.
const ACTIVE_WASH: f64 = 0.25;

/// Color presets offered by the popover; magenta is the default.
pub(crate) const COLOR_PRESETS: [Rgba; 9] = [
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

/// Side of a palette preset swatch in the color popover, in points: the
/// minimum pointer target a pointer-driven control is expected to meet.
const PALETTE_SWATCH_SIZE: f64 = 24.0;

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

/// Everything the user did to the toolbar, reported to the window through
/// the single callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToolbarEvent {
    /// A tool button was clicked; the window owns the toggle rule (the
    /// active tool clicked again deselects) and the frame-mode follow-up.
    ToolToggled(Tool),
    /// The ◄ leafing button: the previous shot of the folder.
    PagePrevious,
    /// The ► leafing button: the next shot of the folder.
    PageNext,
    /// The stroke-width slider moved to `value` (every drag step; the
    /// window collapses a drag into one undo step).
    WidthChanged(f64),
    /// A palette preset swatch was clicked; the popover closed itself.
    PresetSelected(Rgba),
    /// An arrow-tuner slider moved: `index` into `ARROW_PARAM_SPECS`.
    ArrowParamChanged { index: usize, value: f64 },
    /// One tuner row went back to its default.
    ArrowParamReset(usize),
    /// The whole tuner went back to the shipped defaults.
    ArrowParamsReset,
}

/// Ivars of the custom stroke-width slider.
pub(crate) struct WidthSliderIvars {
    value: Cell<f64>,
    target: RefCell<Option<Retained<AnyObject>>>,
    action: Cell<Option<Sel>>,
}

define_class!(
    /// The custom-drawn stroke-width slider: a thin track, an accent fill
    /// and a white round knob. The contract mirrors the `NSSlider` it
    /// replaced — `doubleValue` is what the toolbar's width handler reads
    /// and `setDoubleValue:` is what syncs the knob with the selection —
    /// but it draws itself and reports every drag step (the width edits
    /// collapse into one undo step anyway).
    ///
    /// The superclass is `NSControl`, not `NSView`: the slider lives in the
    /// window's titlebar strip (`FullSizeContentView`), and macOS 26
    /// intercepts mouse-downs there for window dragging, delivering them
    /// only to controls. A plain `NSView` never sees the press at all.
    #[unsafe(super(NSControl))]
    #[thread_kind = MainThreadOnly]
    #[ivars = WidthSliderIvars]
    pub(crate) struct WidthSliderView;

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
                // strip view owns both ends of the pair.
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
            // Track the drag here, in one call: a `mouseDown:` that
            // returns early hands the remaining drag events back to the
            // window, which replays the titlebar-strip press as a window
            // drag — the knob freezes and the editor flies away.
            if let Some(window) = self.window() {
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
                        NSEventType::LeftMouseDragged => self.drag_to(&next),
                        NSEventType::LeftMouseUp => {
                            self.drag_to(&next);
                            break;
                        }
                        _ => {}
                    }
                }
                // Drop the mouse events that piled up behind the tracking
                // loop instead of replaying them into the content view.
                window.discardEventsMatchingMask_beforeEvent(
                    NSEventMask::LeftMouseUp
                        | NSEventMask::LeftMouseDragged
                        | NSEventMask::MouseMoved,
                    None,
                );
            }
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
        pub(crate) fn set_double_value(&self, value: f64);
    );

    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
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
    /// target/action pair.
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

/// A snapshot of everything the toolbar displays, pushed as one value.
/// The window computes it from the document state (the pure reduction is
/// `toolbar_state`) and the strip repaints from it — it never pokes at
/// individual controls from outside.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolbarState {
    /// The active annotation tool (None: viewing mode).
    pub tool: Option<Tool>,
    /// The width the stroke-width slider shows: the selected element's
    /// width when it is one the slider styles, else the width for new
    /// elements.
    pub stroke_width: f64,
    /// The color the swatch disc shows (and new elements draw with).
    pub color: Rgba,
    /// Whether the journal can undo (a knob for the day undo gets a
    /// button).
    pub can_undo: bool,
    /// Whether the journal can redo.
    pub can_redo: bool,
    /// The open shot's file name in the bar.
    pub file_label: String,
}

/// The window-side sink for toolbar events.
type OnEvent = Box<dyn Fn(ToolbarEvent)>;

/// Ivars of the toolbar strip view.
pub(crate) struct ToolbarIvars {
    /// Toolbar buttons (tag → button): the leafing pair and the tools.
    tool_buttons: RefCell<Vec<(isize, Retained<NSButton>)>>,
    /// The color-swatch toolbar button (its bezel shows the current color).
    color_button: RefCell<Option<Retained<NSButton>>>,
    /// The file-name label (the window title is hidden, the bar carries
    /// the name instead).
    file_label: RefCell<Option<Retained<NSTextField>>>,
    /// The stroke-width slider the strip hosts.
    width_slider: RefCell<Option<Retained<WidthSliderView>>>,
    /// The open color popover, if any.
    color_popover: RefCell<Option<Retained<NSPopover>>>,
    /// Debug tuner rows: each arrow-param slider with its value field
    /// (position matches ARROW_PARAM_SPECS; empty without the tuner).
    arrow_param_rows: RefCell<Vec<(Retained<NSSlider>, Retained<NSTextField>)>>,
    /// The smallest content width that fits the whole toolbar.
    min_width: Cell<f64>,
    /// The active tool as the highlight pass last painted it: the hover
    /// pass skips the active tool's button (its accent reads as a
    /// highlight already).
    active_tool: Cell<Option<Tool>>,
    /// The window-side sink for everything the user does to the toolbar.
    on_event: RefCell<Option<OnEvent>>,
}

define_class!(
    /// The toolbar strip of the editor window: a light column carrying the
    /// controls. For the system it is part of the window's titlebar
    /// (`FullSizeContentView`), so a free-area click drags the window —
    /// the controls consume their own clicks and never reach this handler.
    /// Hover styling rides the tracking area added at construction.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ToolbarIvars]
    pub(crate) struct ToolbarView;

    unsafe impl NSObjectProtocol for ToolbarView {}

    impl ToolbarView {
        #[unsafe(method_id(initWithFrame:))]
        unsafe fn __init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self> {
            let this = this.set_ivars(ToolbarIvars {
                tool_buttons: RefCell::new(Vec::new()),
                color_button: RefCell::new(None),
                file_label: RefCell::new(None),
                width_slider: RefCell::new(None),
                color_popover: RefCell::new(None),
                arrow_param_rows: RefCell::new(Vec::new()),
                min_width: Cell::new(0.0),
                active_tool: Cell::new(None),
                on_event: RefCell::new(None),
            });
            // SAFETY: NSView's designated initializer with plain ivars set.
            unsafe { msg_send![super(this), initWithFrame: frame_rect] }
        }

        // A click on the strip's free area drags the window; the controls
        // consume their own clicks, so they never reach this handler.
        #[unsafe(method(mouseDown:))]
        fn __mouse_down(&self, event: &NSEvent) {
            if let Some(window) = self.window() {
                window.performWindowDragWithEvent(event);
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

        #[unsafe(method(pageClicked:))]
        fn __page_clicked(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is the clicked NSButton; `tag` is the
            // NSView accessor we stamped on the button.
            let tag: isize = unsafe { msg_send![sender, tag] };
            let event = if tag == PAGE_PREV_TAG {
                ToolbarEvent::PagePrevious
            } else {
                ToolbarEvent::PageNext
            };
            self.emit(event);
        }

        #[unsafe(method(toolClicked:))]
        fn __tool_clicked(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is the clicked NSButton; `tag` is the
            // NSView accessor we stamped on the button.
            let tag: isize = unsafe { msg_send![sender, tag] };
            if let Some(tool) = Tool::from_tag(tag) {
                self.emit(ToolbarEvent::ToolToggled(tool));
            }
        }

        // The swatch click is pure toolbar UI: open or close the popover.
        // No event — the document learns nothing from it.
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
            // A picked preset closes the popover; the recolor itself is the
            // window's call (the document is not the toolbar's business).
            let popover = self.ivars().color_popover.borrow_mut().take();
            if let Some(popover) = popover {
                // SAFETY: plain close on a live popover.
                unsafe { popover.performClose(None) };
            }
            self.emit(ToolbarEvent::PresetSelected(color));
        }

        #[unsafe(method(widthSliderChanged:))]
        fn __width_slider_changed(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is the WidthSliderView; `doubleValue` is
            // its NSControl accessor.
            let value: f64 = unsafe { msg_send![sender, doubleValue] };
            self.emit(ToolbarEvent::WidthChanged(value));
        }

        #[unsafe(method(arrowParamChanged:))]
        fn __arrow_param_changed(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is a tuner slider; `tag` and `doubleValue`
            // are its accessors.
            let tag: isize = unsafe { msg_send![sender, tag] };
            let value: f64 = unsafe { msg_send![sender, doubleValue] };
            if let Some((_, field)) = self.ivars().arrow_param_rows.borrow().get(tag as usize) {
                field.setStringValue(&NSString::from_str(&format!("{value:.2}")));
            }
            self.emit(ToolbarEvent::ArrowParamChanged {
                index: tag as usize,
                value,
            });
        }

        #[unsafe(method(arrowTunerReset:))]
        fn __arrow_tuner_reset(&self, _sender: Option<&AnyObject>) {
            let defaults = render::ArrowParams::DEFAULT;
            let rows = self.ivars().arrow_param_rows.borrow();
            for (spec, (slider, field)) in ARROW_PARAM_SPECS.iter().zip(rows.iter()) {
                let value = (spec.get)(defaults);
                slider.setDoubleValue(value);
                field.setStringValue(&NSString::from_str(&format!("{value:.2}")));
            }
            self.emit(ToolbarEvent::ArrowParamsReset);
        }

        #[unsafe(method(arrowParamReset:))]
        fn __arrow_param_reset(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is a per-row reset button; `tag` is its
            // accessor.
            let tag: isize = unsafe { msg_send![sender, tag] };
            // Only this knob returns to its shipped default; the rest of
            // the tuning session stays.
            let defaults = render::ArrowParams::DEFAULT;
            let Some(spec) = ARROW_PARAM_SPECS.get(tag as usize) else {
                return;
            };
            let value = (spec.get)(defaults);
            if let Some((slider, field)) = self.ivars().arrow_param_rows.borrow().get(tag as usize)
            {
                slider.setDoubleValue(value);
                field.setStringValue(&NSString::from_str(&format!("{value:.2}")));
            }
            self.emit(ToolbarEvent::ArrowParamReset(tag as usize));
        }

        #[unsafe(method(arrowPromptCopy:))]
        fn __arrow_prompt_copy(&self, _sender: Option<&AnyObject>) {
            // The tuner's whole point: carry the tuned values out of the
            // app and into the coding agent that bakes them in as the new
            // defaults. Pure clipboard work — the document learns nothing.
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

    // SAFETY: only the did-close callback is implemented; the strip view
    // outlives the popover it anchors.
    unsafe impl NSPopoverDelegate for ToolbarView {
        #[unsafe(method(popoverDidClose:))]
        fn __popover_did_close(&self, _notification: &NSNotification) {
            // Forget the closed popover so the next click on the swatch
            // button opens a fresh one.
            *self.ivars().color_popover.borrow_mut() = None;
        }
    }
);

impl ToolbarView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    /// Build the whole strip: the row of controls, the tracking area, the
    /// initial tint. `content_size` is the window content size the strip
    /// is laid out for (the stack frame is built from it and stretches
    /// with the window through the autoresizing mask).
    pub fn new(
        mtm: MainThreadMarker,
        content_size: CGSize,
        on_event: impl Fn(ToolbarEvent) + 'static,
    ) -> Retained<Self> {
        // SAFETY: designated initializer with plain ivars set.
        let view = unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::ZERO) };
        // The strip takes its size before the row moves in: a frame change
        // with the stack already inside would run the stack's autoresizing
        // mask over a zero-sized origin and smear the row off the strip.
        view.setFrame(CGRect::new(
            CGPoint::ZERO,
            CGSize::new(content_size.width, TOOLBAR_HEIGHT),
        ));
        *view.ivars().on_event.borrow_mut() = Some(Box::new(on_event));

        // Hover styling: one tracking area over the strip, owned by the
        // strip itself — it holds the button list and the active tool.
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
        view.addTrackingArea(&tracking);

        let toolbar = NSStackView::new(mtm);
        toolbar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        toolbar.setSpacing(BAR_SPACING);
        let mut tool_buttons = Vec::new();
        // The ◄/► leafing pair rides right of the traffic lights' leading
        // pad, tight together like the back/forward pair of the reference.
        for (tag, symbol, title) in [
            (PAGE_PREV_TAG, "chevron.left", "Previous shot"),
            (PAGE_NEXT_TAG, "chevron.right", "Next shot"),
        ] {
            let button = symbol_button(mtm, symbol, title, tag);
            // SAFETY: target is this strip view, alive for the process.
            unsafe {
                button.setTarget(Some(&view));
                button.setAction(Some(sel!(pageClicked:)));
            }
            toolbar.addArrangedSubview(&button);
            tool_buttons.push((tag, button));
        }
        // The open shot's file name closes the leafing zone: plain
        // secondary text, not a control — a drag on it moves the window.
        let file_label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        file_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        file_label.setAlignment(NSTextAlignment::Center);
        file_label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        // The block rides a fixed width — one for every shot, so the
        // row holds still across leafing; the height stays on the
        // intrinsicContentSize, like the slider's.
        file_label.setTranslatesAutoresizingMaskIntoConstraints(false);
        file_label
            .widthAnchor()
            .constraintEqualToConstant(FILE_LABEL_WIDTH)
            .setActive(true);
        // The fixed width must beat the text's own size for names longer
        // than the block: they truncate in the middle instead of fighting
        // the constraint.
        file_label.setContentCompressionResistancePriority_forOrientation(
            250.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        // Zones: leafing | file name | color + width | tools, separated
        // by 1 pt dividers with a wider gap around them.
        let leafing_sep = bar_separator(mtm);
        toolbar.addArrangedSubview(&leafing_sep);
        toolbar.setCustomSpacing_afterView(BAR_ZONE_SPACING, &leafing_sep);
        toolbar.addArrangedSubview(&file_label);
        toolbar.setCustomSpacing_afterView(BAR_ZONE_SPACING, &file_label);
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
        // so the swatch reads as one more button and hovers with the same
        // plate as they do. The bright outline keeps dark presets (black,
        // blue) distinguishable from the panel at boundary contrast; it
        // stays at rest on hover, since the shared plate already marks it.
        let disc = CALayer::new();
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
        disc.setName(Some(&NSString::from_str(DISC_LAYER_NAME)));
        // The hover plate — the same named sublayer the icon buttons carry,
        // so the swatch highlights with an equal square container; the
        // disc rides on top of it.
        let swatch_plate = CALayer::new();
        swatch_plate.setCornerRadius(6.0);
        swatch_plate.setName(Some(&NSString::from_str(PLATE_LAYER_NAME)));
        if let Some(layer) = color_button.layer() {
            layer.addSublayer(&disc);
            layer.addSublayer(&swatch_plate);
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
        // SAFETY: target is this strip view, alive for the process.
        unsafe {
            color_button.setTarget(Some(&view));
            color_button.setAction(Some(sel!(colorClicked:)));
        }
        // The slider sizes itself through its intrinsicContentSize — the
        // toolbar stack reads it for layout, but only for views with the
        // autoresizing mask off; every neighbor clears it too.
        let slider = WidthSliderView::new(mtm);
        slider.setTranslatesAutoresizingMaskIntoConstraints(false);
        // SAFETY: target is this strip view, alive for the process; the
        // action reads `doubleValue` off the sender.
        unsafe {
            slider.set_target(Some(&view));
            slider.set_action(Some(sel!(widthSliderChanged:)));
        }
        *view.ivars().width_slider.borrow_mut() = Some(slider.clone());
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
            // SAFETY: target is this strip view, alive for the process.
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

        // The strip stretches with the window: its frame is built from the
        // initial size, and a fixed-width stack squeezes the row — the
        // slider, the only view without required size constraints, absorbs
        // the deficit and vanishes.
        toolbar.setFrame(CGRect::new(
            CGPoint::new(
                BAR_LEADING_PAD,
                TOOLBAR_HEIGHT - BAR_TOP_PAD - TOOLBAR_BUTTON_SIZE.height / 2.0
                    - SLIDER_HEIGHT / 2.0,
            ),
            CGSize::new(content_size.width - BAR_LEADING_PAD - BAR_TRAILING_PAD, SLIDER_HEIGHT),
        ));
        toolbar.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        view.addSubview(&toolbar);
        // The strip itself stretches with the window and hugs the top.
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin,
        );

        // The window cannot shrink below the bar: the leading pad for the
        // traffic lights, the row's fitting width, the trailing pad.
        let min_width = toolbar.fittingSize().width + BAR_LEADING_PAD + BAR_TRAILING_PAD;
        view.ivars().min_width.set(min_width);
        *view.ivars().tool_buttons.borrow_mut() = tool_buttons;
        *view.ivars().color_button.borrow_mut() = Some(color_button);
        *view.ivars().file_label.borrow_mut() = Some(file_label);
        view.update_tool_highlight(None);
        view
    }

    /// Report a user action to the window.
    fn emit(&self, event: ToolbarEvent) {
        if let Some(on_event) = self.ivars().on_event.borrow().as_ref() {
            on_event(event);
        }
    }

    /// The smallest content width that fits the toolbar.
    pub fn min_width(&self) -> f64 {
        self.ivars().min_width.get()
    }

    /// The single input: repaint the whole strip from the snapshot. The
    /// traffic-lights alignment runs here too — it must follow every
    /// window relayout, and the window pushes a fresh state after each
    /// refit, so the pass rides along.
    pub fn set_state(&self, state: ToolbarState) {
        if let Some(label) = self.ivars().file_label.borrow().as_ref() {
            label.setStringValue(&NSString::from_str(&state.file_label));
        }
        self.update_tool_highlight(state.tool);
        self.show_color(state.color);
        // The slider skips an equal value, so a push never re-enters the
        // width handler as a phantom drag.
        if let Some(slider) = self.ivars().width_slider.borrow().as_ref() {
            slider.set_double_value(state.stroke_width);
        }
        self.align_traffic_lights();
    }

    /// Show the color on the toolbar swatch circle.
    fn show_color(&self, color: [f64; 4]) {
        let button = self.ivars().color_button.borrow().clone();
        let Some(button) = button.as_ref() else {
            return;
        };
        let Some(disc) = disc(button) else {
            return;
        };
        // Re-center the disc and the plate on the button's settled bounds
        // (the stack may have re-laid the row since the last pass).
        center_toolbar_layers(button);
        let [r, g, b, a] = color;
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        disc.setBackgroundColor(Some(&color.CGColor()));
    }

    /// Accent-wash the active tool's button, white-tint the others.
    fn update_tool_highlight(&self, active: Option<Tool>) {
        self.ivars().active_tool.set(active);
        let accent: Retained<NSColor> = NSColor::controlAccentColor();
        let icon = NSColor::colorWithSRGBRed_green_blue_alpha(
            ICON_TINT[0],
            ICON_TINT[1],
            ICON_TINT[2],
            ICON_TINT[3],
        );
        animate_plate_changes(|| {
            for (tag, button) in self.ivars().tool_buttons.borrow().iter() {
                if let Some(plate) = plate(button) {
                    let bg: Option<Retained<NSColor>> = if Some(*tag) == active.map(|tool| tool.tag())
                    {
                        Some(accent.colorWithAlphaComponent(ACTIVE_WASH))
                    } else {
                        None
                    };
                    plate.setBackgroundColor(bg.map(|color| color.CGColor()).as_deref());
                }
                button.setContentTintColor(if Some(*tag) == active.map(|tool| tool.tag()) {
                    Some(&accent)
                } else {
                    Some(&icon)
                });
                center_toolbar_layers(button);
            }
        });
    }

    /// Slide the system traffic lights' hosting view vertically so their
    /// centers share the toolbar icon row's axis (`TOOLBAR_ROW_AXIS_FROM_TOP`
    /// from the top edge; the titlebar parks them higher). The shift rides
    /// the container's frame converted through content coordinates, which
    /// absorbs the titlebar's flipped geometry; the titlebar is never
    /// re-laid by us, so the offset holds until the next relayout — hence
    /// the call on every state push (the window pushes after each refit).
    /// Before the window is first shown the buttons have no superview and
    /// the pass is a no-op.
    fn align_traffic_lights(&self) {
        let Some(window) = self.window() else {
            return;
        };
        let Some(close) = window.standardWindowButton(NSWindowButton::CloseButton) else {
            return;
        };
        let Some(content) = window.contentView() else {
            return;
        };
        // SAFETY: plain view-tree reads, no external invariants.
        let Some(host) = (unsafe { close.superview() }) else {
            return;
        };
        let in_content = host.convertRect_toView(host.bounds(), Some(&content));
        let axis_y = content.bounds().size.height - TOOLBAR_ROW_AXIS_FROM_TOP;
        let center_y = in_content.origin.y + in_content.size.height / 2.0;
        let dy = axis_y - center_y;
        if dy == 0.0 {
            return;
        }
        // SAFETY: plain view-tree reads, no external invariants.
        let Some(superview) = (unsafe { host.superview() }) else {
            return;
        };
        let target = CGRect::new(
            CGPoint::new(in_content.origin.x, in_content.origin.y + dy),
            in_content.size,
        );
        let target_in_superview = content.convertRect_toView(target, Some(&superview));
        host.setFrame(target_in_superview);
    }

    /// Restyle the controls under the cursor. Two surfaces are served
    /// here, one owner: the toolbar strip's buttons get a faint hover wash
    /// (the active tool's accent button is skipped), the palette presets
    /// in the open popover get a darker ring. The strip lives in the
    /// editor window and the palette in the popover's own window, so the
    /// event's window decides which surface is under the cursor; `None`
    /// (the cursor left both) clears both.
    fn hover_at(&self, event: Option<&NSEvent>) {
        let (toolbar_hit, preset_hit) = match event {
            None => (None, None),
            Some(event) => {
                let point = event.locationInWindow();
                // Compared by window number: NSEvent::window needs a
                // MainThreadMarker, the number needs nothing.
                let strip_number = self.window().map(|window| window.windowNumber());
                let in_editor_window = strip_number == Some(event.windowNumber());
                if in_editor_window {
                    (self.panel_button_at(point), None)
                } else {
                    (None, self.palette_button_at(point))
                }
            }
        };
        let active = self.ivars().active_tool.get().map(|tool| tool.tag());
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
                if Some(*tag) == active {
                    continue;
                }
                if let Some(plate) = plate(button) {
                    let bg: Option<&NSColor> = if Some(*tag) == toolbar_hit {
                        Some(&hover)
                    } else {
                        None
                    };
                    plate.setBackgroundColor(bg.map(|color| color.CGColor()).as_deref());
                }
                center_toolbar_layers(button);
            }
            // The swatch rides the same hover plate as the icon buttons, so
            // every toolbar control highlights with an equal square tile;
            // the outline itself stays at rest (it only keeps light presets
            // visible against the light column).
            if let Some(button) = self.ivars().color_button.borrow().as_ref() {
                if let Some(plate) = plate(button) {
                    let bg: Option<&NSColor> = if toolbar_hit == Some(COLOR_BUTTON_TAG) {
                        Some(&hover)
                    } else {
                        None
                    };
                    plate.setBackgroundColor(bg.map(|color| color.CGColor()).as_deref());
                }
                center_toolbar_layers(button);
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
        hit.downcast_ref::<NSButton>().map(|button| button.tag())
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
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // SAFETY: plain init on a freshly allocated popover.
        let popover: Retained<NSPopover> = unsafe { msg_send![NSPopover::alloc(mtm), init] };
        // Closes only where the code closes it (a preset pick or a second
        // click on the swatch button), so the toggle stays deterministic.
        popover.setBehavior(objc2_app_kit::NSPopoverBehavior::ApplicationDefined);
        // The delegate is this strip view, alive for the process.
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
            // SAFETY: target is this strip view, alive for the process.
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
        // Swatch hover rides the same owner as the strip tracking: one
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
                Some(self),
                None,
            )
        };
        content.addTrackingArea(&tracking);
        popover.showRelativeToRect_ofView_preferredEdge(
            anchor_button.bounds(),
            anchor_button,
            NSRectEdge::MinY,
        );
        *self.ivars().color_popover.borrow_mut() = Some(popover);
    }

    /// Tag of the strip button under a window-space point. The buttons are
    /// matched by frame, not by `hitTest`: the system-hosted buttons of
    /// recent macOS skip the classic hit-testing path (hover silently
    /// found nothing), while their frames are plain geometry. The frame
    /// inflates to the plate size, so the hover trigger area covers the
    /// whole visible container, not just the narrower button cell.
    fn panel_button_at(&self, point: CGPoint) -> Option<isize> {
        let inside = |rect: CGRect| {
            point.x >= rect.origin.x
                && point.x <= rect.origin.x + rect.size.width
                && point.y >= rect.origin.y
                && point.y <= rect.origin.y + rect.size.height
        };
        let hit_rect = |button: &NSButton| {
            let frame = button.convertRect_toView(button.bounds(), None);
            let cx = frame.origin.x + frame.size.width / 2.0;
            let cy = frame.origin.y + frame.size.height / 2.0;
            CGRect::new(
                CGPoint::new(
                    cx - TOOLBAR_PLATE_SIZE / 2.0,
                    cy - TOOLBAR_PLATE_SIZE / 2.0,
                ),
                CGSize::new(
                    TOOLBAR_PLATE_SIZE,
                    TOOLBAR_PLATE_SIZE,
                ),
            )
        };
        for (tag, button) in self.ivars().tool_buttons.borrow().iter() {
            if inside(hit_rect(button)) {
                return Some(*tag);
            }
        }
        if let Some(button) = self.ivars().color_button.borrow().as_ref() {
            if inside(hit_rect(button)) {
                return Some(COLOR_BUTTON_TAG);
            }
        }
        None
    }
}

/// An SF Symbol toolbar button with an empty title, tagged with the tool.
pub(crate) fn symbol_button(
    mtm: MainThreadMarker,
    symbol: &str,
    title: &str,
    tag: isize,
) -> Retained<NSButton> {
    icon_button(mtm, symbol_image(symbol, title), title, tag)
}

/// The SF Symbol image at the shared icon size; the accessibility
/// description is the human-readable button name, not the symbol name.
pub(crate) fn symbol_image(symbol: &str, name: &str) -> Retained<NSImage> {
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
/// wash and the active-tool accent light the plate sublayer. Toolbar
/// buttons never take first responder: no focus ring, keys stay with the
/// canvas. `title` becomes the tooltip and the accessibility label — an
/// icon-only button has no visible text for either to fall back on.
pub(crate) fn icon_button(
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
        // The hover/active plate is its own fixed-size sublayer rather than
        // the button layer's background: the toolbar stack lays arranged
        // views out by their fitting size and pays no mind to the size
        // constraints below — the button comes out taller than wide (the
        // glyph's intrinsic height), and a layer background would read as
        // a rectangle. The plate is deliberately larger than the cell: the
        // container grows around the glyph, which stays centered at its
        // own size. Named, so the centering pass never touches the
        // system's private button sublayers.
        let plate = CALayer::new();
        plate.setCornerRadius(6.0);
        plate.setName(Some(&NSString::from_str(PLATE_LAYER_NAME)));
        layer.addSublayer(&plate);
    }
    // The cell size rides on constraints: they hold the width (the stack
    // honors it) and pin the nominal size for accessibility; the height
    // the stack actually gives is the glyph's fitting size, which is why
    // the plate is a separate centered sublayer instead of a background.
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
pub(crate) struct ArrowParamSpec {
    pub(crate) name: &'static str,
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) get: fn(render::ArrowParams) -> f64,
    pub(crate) set: fn(&mut render::ArrowParams, f64),
}

/// The tuner rows, top to bottom — the main constants of the arrow
/// construction (`render::ArrowParams`). `half_head_w` stays above the
/// body half-width (1.1 stroke widths), so even a dragged-low slider
/// cannot invert the wing into the body.
pub(crate) const ARROW_PARAM_SPECS: &[ArrowParamSpec] = &[
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
pub(crate) fn arrow_tuner_panel(mtm: MainThreadMarker, view: &ToolbarView) -> Retained<NSBox> {
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
        // SAFETY: target is this strip view, alive for the process.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
