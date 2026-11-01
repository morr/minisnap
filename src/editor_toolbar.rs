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
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{define_class, extern_methods, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSAutoresizingMaskOptions, NSButton, NSColor, NSEvent, NSFocusRingType,
    NSFontWeightRegular, NSImage, NSImageSymbolConfiguration, NSLayoutConstraintOrientation,
    NSLineBreakMode, NSStackView, NSTextAlignment, NSTextField, NSTrackingArea,
    NSTrackingAreaOptions, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreateImage, CGContext};
use objc2_foundation::NSString;
use objc2_quartz_core::CALayer;

use crate::editor::render;
use crate::editor::Point;
use crate::editor_window::{WidthSliderView, SLIDER_HEIGHT};
use crate::plates::{
    center_toolbar_layers, disc, plate, animate_plate_changes, DISC_LAYER_NAME, HOVER_BG,
    PLATE_LAYER_NAME, SWATCH_DISC, SWATCH_RING, TOOLBAR_PLATE_SIZE,
};

/// Toolbar strip height, in points: the reference's roomier air above
/// the buttons and the shorter one between them and the card below.
pub const TOOLBAR_HEIGHT: f64 = 36.0;

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
    /// with the window through the autoresizing mask). The swatch button
    /// and the width slider keep their window-side handlers for now:
    /// `transitional_target` is the editor view they target until the
    /// popover and the slider move in behind the same event seam.
    pub fn new(
        mtm: MainThreadMarker,
        content_size: CGSize,
        transitional_target: &AnyObject,
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
        // SAFETY: the target is the editor view; its popover handler owns
        // the swatch until the palette moves behind the event seam.
        unsafe {
            color_button.setTarget(Some(transitional_target));
            color_button.setAction(Some(sel!(colorClicked:)));
        }
        // The slider sizes itself through its intrinsicContentSize — the
        // toolbar stack reads it for layout, but only for views with the
        // autoresizing mask off; every neighbor clears it too.
        let slider = WidthSliderView::new(mtm);
        slider.setTranslatesAutoresizingMaskIntoConstraints(false);
        // SAFETY: the target is the editor view; its width handler owns
        // the slider until it moves behind the event seam; the action
        // reads `doubleValue` off the sender.
        unsafe {
            slider.set_target(Some(transitional_target));
            slider.set_action(Some(sel!(widthChanged:)));
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

    /// The stroke-width slider the strip hosts. Transitional: the window
    /// still syncs the slider's knob with the selection until the slider
    /// moves behind `set_state`.
    pub fn width_slider(&self) -> Retained<WidthSliderView> {
        self.ivars().width_slider.borrow().clone().expect("slider built")
    }

    /// Show the open shot's file name in the bar.
    pub fn set_file_label(&self, name: &str) {
        if let Some(label) = self.ivars().file_label.borrow().as_ref() {
            label.setStringValue(&NSString::from_str(name));
        }
    }

    /// Show the color on the toolbar swatch circle.
    pub fn show_color(&self, color: [f64; 4]) {
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
    pub fn update_tool_highlight(&self, active: Option<Tool>) {
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

    /// Restyle the controls under the cursor: a faint wash on the hovered
    /// toolbar button — the icon buttons and the color swatch alike, all
    /// equal-width tiles — nothing on the others. The active tool's button
    /// owns its accent background and is skipped. `None` (the cursor left
    /// the strip) clears the wash.
    fn hover_at(&self, event: Option<&NSEvent>) {
        let toolbar_hit = event.and_then(|event| self.panel_button_at(event.locationInWindow()));
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
            // every toolbar control highlights with an equal square tile.
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
        });
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
