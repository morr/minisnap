//! The editor window: one reusable `NSWindow` showing the open shot — the
//! base frame plus annotations, drawn with CoreGraphics into a custom
//! `NSView`, with an annotation toolbar above the canvas. Selection markers
//! are drawn on top in the canvas and never become part of a saved
//! composite.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{
    define_class, extern_methods, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSColor, NSEvent, NSGraphicsContext,
    NSImage, NSImageSymbolConfiguration, NSPopover, NSScreen, NSStackView,
    NSUserInterfaceLayoutOrientation, NSView, NSViewController, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGImage, CGContext};
use objc2_foundation::{NSString, NSRectEdge};

use crate::editor::{render, ElementKind, OpenShot, Point, Rect, Rgba};

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
        Point::new(
            local.x * iw as f64 / bounds.size.width,
            (bounds.size.height - local.y) * ih as f64 / bounds.size.height,
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
        // pixel units (`draw_elements` applies its own Y flip).
        CGContext::save_g_state(Some(&cg));
        CGContext::scale_ctm(Some(&cg), sx, sy);
        CGContext::draw_image(
            Some(&cg),
            CGRect::new(CGPoint::ZERO, CGSize::new(iw as f64, ih as f64)),
            Some(base),
        );
        render::draw_elements(&cg, base, ih as f64, shot.doc.elements());
        CGContext::restore_g_state(Some(&cg));

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
        eprintln!("editor: mouse down at {point:?}");
    }

    fn handle_mouse_dragged(&self, event: &NSEvent) {
        let point = self.pixel_point(event);
        eprintln!("editor: mouse dragged to {point:?}");
    }

    fn handle_mouse_up(&self, event: &NSEvent) {
        let point = self.pixel_point(event);
        eprintln!("editor: mouse up at {point:?}");
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
