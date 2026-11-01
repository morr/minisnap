//! Slider-drag bug probe: a faithful replica of the editor toolbar (panel,
//! stack, leaf button, file label, swatch, the custom `WidthSliderView`)
//! with logging on every mouse handler, to verify with a real CGEvent drag
//! which view claims the drag over the slider and whether the window moves.
//! Run: cargo run --example slider_probe — then
//! `swift /tmp/sendclick.swift X Y --drag X2 Y2` at the printed slider
//! center; watch stderr.

use objc2::define_class;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObjectProtocol, Sel};
use objc2::{
    extern_methods, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use std::cell::RefCell;
use std::ptr::NonNull;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton, NSColor, NSEvent,
    NSEventMask, NSEventType, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNotification, NSString};

type WindowBase = objc2_app_kit::NSWindow;

// --- constants copied verbatim from src/editor_window.rs ---
const TOOLBAR_HEIGHT: f64 = 36.0;
const BAR_TOP_PAD: f64 = 9.0;
const BAR_LEADING_PAD: f64 = 70.0;
const BAR_TRAILING_PAD: f64 = 12.0;
const BAR_SPACING: f64 = 6.0;
const BAR_ZONE_SPACING: f64 = 12.0;
const STROKE_SLIDER_WIDTH_PT: f64 = 90.0;
const SLIDER_HEIGHT: f64 = 24.0;
const SLIDER_KNOB_RADIUS: f64 = 6.5;
const SLIDER_TRACK_HEIGHT: f64 = 3.0;
const TRACK_UNFILLED: f64 = 0.15;
const KNOB_SHADOW: f64 = 0.35;
const TOOLBAR_BUTTON_SIZE: CGSize = CGSize::new(22.0, 22.0);
const FILE_LABEL_WIDTH: f64 = 276.0;
const DEFAULT_STROKE_WIDTH: f64 = 4.0;
const STROKE_MIN: f64 = 1.0;
const STROKE_MAX: f64 = 12.0;
const INITIAL_SIZE: CGSize = CGSize::new(480.0, 320.0);
const WINDOW_BG: [f64; 4] = [242.0 / 255.0, 242.0 / 255.0, 242.0 / 255.0, 1.0];

// --- WidthSliderView, copied verbatim from src/editor_window.rs ---
struct WidthSliderIvars {
    value: std::cell::Cell<f64>,
    target: std::cell::RefCell<Option<Retained<AnyObject>>>,
    action: std::cell::Cell<Option<Sel>>,
}

define_class!(
    // TEST B: NSControl superclass — controls receive mouse events in the
    // full-size-content-view titlebar region where plain NSViews get
    // swallowed by the system titlebar-drag interception.
    #[unsafe(super(objc2_app_kit::NSControl))]
    #[thread_kind = MainThreadOnly]
    #[ivars = WidthSliderIvars]
    struct WidthSliderView;

    unsafe impl NSObjectProtocol for WidthSliderView {}

    impl WidthSliderView {
        #[unsafe(method_id(initWithFrame:))]
        unsafe fn __init_with_frame(
            this: Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self> {
            let this = this.set_ivars(WidthSliderIvars {
                value: std::cell::Cell::new(DEFAULT_STROKE_WIDTH),
                target: std::cell::RefCell::new(None),
                action: std::cell::Cell::new(None),
            });
            // SAFETY: NSView's designated initializer with plain ivars set.
            unsafe { msg_send![super(this), initWithFrame: frame_rect] }
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn __accepts_first_responder(&self) -> bool {
            false
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn __mouse_down_can_move_window(&self) -> bool {
            eprintln!("PROBE: slider mouseDownCanMoveWindow queried");
            false
        }

        #[unsafe(method(intrinsicContentSize))]
        fn __intrinsic_content_size(&self) -> CGSize {
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
                // SAFETY: probe lifetime, outlives the run loop.
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
            eprintln!("PROBE: slider mouseDown at {:?}", event.locationInWindow());
            self.drag_to(event);
            // Tracking loop: hold the event stream until mouse-up, like
            // NSButton does — returning without one lets the window take
            // the remaining drag events over (the reported bug).
            if let Some(window) = self.window() {
                loop {
                    let Some(next) = window.nextEventMatchingMask_untilDate_inMode_dequeue(
                        NSEventMask::LeftMouseUp | NSEventMask::LeftMouseDragged,
                        Some(&objc2_foundation::NSDate::distantFuture()),
                        unsafe { objc2_app_kit::NSEventTrackingRunLoopMode },
                        true,
                    ) else {
                        break;
                    };
                    match next.r#type() {
                        NSEventType::LeftMouseDragged => {
                            eprintln!(
                                "PROBE: slider tracked dragged at {:?}",
                                next.locationInWindow()
                            );
                            self.drag_to(&next);
                        }
                        NSEventType::LeftMouseUp => {
                            eprintln!(
                                "PROBE: slider tracked mouseUp at {:?}",
                                next.locationInWindow()
                            );
                            self.drag_to(&next);
                            break;
                        }
                        _ => {}
                    }
                }
                // Discard any queued mouse events after the loop: the
                // window otherwise replays the intercepted titlebar press
                // as a one-shot window jump after mouse-up.
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
            eprintln!(
                "PROBE: slider mouseDragged at {:?}",
                event.locationInWindow()
            );
            self.drag_to(event);
        }

        #[unsafe(method(mouseUp:))]
        fn __mouse_up(&self, event: &NSEvent) {
            eprintln!("PROBE: slider mouseUp at {:?}", event.locationInWindow());
            self.drag_to(event);
        }
    }
);

impl WidthSliderView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: Allocated<Self>,
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

    fn set_value(&self, value: f64) {
        let old = self.ivars().value.get();
        self.ivars().value.set(value);
        if value == old {
            return;
        }
        self.setNeedsDisplayInRect(self.bounds());
        self.send_action();
    }

    fn drag_to(&self, event: &NSEvent) {
        let bounds = self.bounds();
        if bounds.size.width < 2.0 * SLIDER_KNOB_RADIUS {
            eprintln!("PROBE: slider drag_to skipped, width={}", bounds.size.width);
            return;
        }
        let local = self.convertPoint_fromView(event.locationInWindow(), None);
        let frac = ((local.x - SLIDER_KNOB_RADIUS)
            / (bounds.size.width - 2.0 * SLIDER_KNOB_RADIUS))
        .clamp(0.0, 1.0);
        eprintln!("PROBE: slider drag_to value={}", STROKE_MIN + (STROKE_MAX - STROKE_MIN) * frac);
        self.set_value(STROKE_MIN + (STROKE_MAX - STROKE_MIN) * frac);
    }

    fn send_action(&self) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let action = self.ivars().action.get();
        let target = self.ivars().target.borrow().clone();
        if let Some(action) = action {
            // SAFETY: main-thread target/action pair.
            unsafe {
                NSApplication::sharedApplication(mtm).sendAction_to_from(
                    action,
                    target.as_deref(),
                    Some(self),
                );
            }
        }
    }

    fn draw(&self) {
        let Some(context) = objc2_app_kit::NSGraphicsContext::currentContext() else {
            return;
        };
        let cg = context.CGContext();
        let bounds = self.bounds();
        let (w, h) = (bounds.size.width, bounds.size.height);
        if w < 2.0 * SLIDER_KNOB_RADIUS {
            return;
        }
        let frac = ((self.ivars().value.get() - STROKE_MIN) / (STROKE_MAX - STROKE_MIN))
            .clamp(0.0, 1.0);
        let knob_x = SLIDER_KNOB_RADIUS + (w - 2.0 * SLIDER_KNOB_RADIUS) * frac;
        let track_y = h / 2.0 - SLIDER_TRACK_HEIGHT / 2.0;
        objc2_core_graphics::CGContext::set_rgb_fill_color(Some(&cg), 0.0, 0.0, 0.0, TRACK_UNFILLED);
        objc2_core_graphics::CGContext::fill_rect(
            Some(&cg),
            CGRect::new(
                CGPoint::new(SLIDER_KNOB_RADIUS, track_y),
                CGSize::new(w - 2.0 * SLIDER_KNOB_RADIUS, SLIDER_TRACK_HEIGHT),
            ),
        );
        let filled = knob_x - SLIDER_KNOB_RADIUS;
        if filled > 0.0 {
            let accent: Retained<NSColor> = NSColor::controlAccentColor();
            let accent_cg = accent.CGColor();
            objc2_core_graphics::CGContext::set_fill_color_with_color(Some(&cg), Some(&accent_cg));
            objc2_core_graphics::CGContext::fill_rect(
                Some(&cg),
                CGRect::new(
                    CGPoint::new(SLIDER_KNOB_RADIUS, track_y),
                    CGSize::new(filled, SLIDER_TRACK_HEIGHT),
                ),
            );
        }
        let knob = CGRect::new(
            CGPoint::new(knob_x - SLIDER_KNOB_RADIUS, h / 2.0 - SLIDER_KNOB_RADIUS),
            CGSize::new(2.0 * SLIDER_KNOB_RADIUS, 2.0 * SLIDER_KNOB_RADIUS),
        );
        let shadow = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, KNOB_SHADOW);
        let shadow_cg = shadow.CGColor();
        objc2_core_graphics::CGContext::set_shadow_with_color(
            Some(&cg),
            CGSize::new(0.0, 1.0),
            2.0,
            Some(&shadow_cg),
        );
        objc2_core_graphics::CGContext::set_rgb_fill_color(Some(&cg), 1.0, 1.0, 1.0, 1.0);
        objc2_core_graphics::CGContext::fill_ellipse_in_rect(Some(&cg), knob);
        objc2_core_graphics::CGContext::set_shadow_with_color(
            Some(&cg),
            CGSize::ZERO,
            0.0,
            None,
        );
    }
}

// The window performs a drag only when the panel explicitly requests one:
// system-initiated calls (the titlebar-region interception that steals the
// slider's drag and jumps the window after mouse-up) are ignored.
static PANEL_DRAG_ALLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// --- ToolbarPanel analog with logging ---
define_class!(
    // v9: back to plain NSView — does the panel dispatch without NSControl
    // once movable=NO is set?
    #[unsafe(super(objc2_app_kit::NSView))]
    #[thread_kind = MainThreadOnly]
    struct ProbePanel;

    unsafe impl NSObjectProtocol for ProbePanel {}

    impl ProbePanel {
        #[unsafe(method(mouseDown:))]
        fn __mouse_down(&self, event: &NSEvent) {
            eprintln!("PROBE: panel mouseDown at {:?}", event.locationInWindow());
            if let Some(window) = self.window() {
                eprintln!("PROBE: panel starts WINDOW DRAG");
                PANEL_DRAG_ALLOWED.store(true, std::sync::atomic::Ordering::SeqCst);
                window.performWindowDragWithEvent(event);
                PANEL_DRAG_ALLOWED.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn __panel_can_move(&self) -> bool {
            eprintln!("PROBE: panel mouseDownCanMoveWindow queried");
            false
        }
    }
);

impl ProbePanel {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: designated initializer with no ivars.
        unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::ZERO) }
    }
}

// --- stack analog: NSStackView subclass with logging ---
define_class!(
    #[unsafe(super(objc2_app_kit::NSStackView))]
    #[thread_kind = MainThreadOnly]
    struct ProbeStack;

    unsafe impl NSObjectProtocol for ProbeStack {}

    impl ProbeStack {
        #[unsafe(method(mouseDown:))]
        fn __mouse_down(&self, event: &NSEvent) {
            eprintln!("PROBE: stack mouseDown at {:?}", event.locationInWindow());
            // SAFETY: NSResponder default — forward up the chain.
            unsafe {
                let next = self.nextResponder();
                if let Some(next) = next {
                    let _: () = unsafe { msg_send![&*next, mouseDown: event] };
                }
            }
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn __stack_can_move(&self) -> bool {
            eprintln!("PROBE: stack mouseDownCanMoveWindow queried");
            false
        }
    }
);

impl ProbeStack {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: designated initializer with no ivars.
        unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::ZERO) }
    }
}

// --- Canvas analog with logging ---
define_class!(
    #[unsafe(super(objc2_app_kit::NSView))]
    #[thread_kind = MainThreadOnly]
    struct ProbeCanvas;

    unsafe impl NSObjectProtocol for ProbeCanvas {}

    impl ProbeCanvas {
        #[unsafe(method(mouseDown:))]
        fn __mouse_down(&self, event: &NSEvent) {
            eprintln!(
                "PROBE: canvas mouseDown at {:?} (manual window drag)",
                event.locationInWindow()
            );
            // Manual window drag: with movable=NO the system rejects
            // performWindowDragWithEvent outside the titlebar strip, so the
            // canvas tracks the drag itself and moves the window by the
            // global mouse delta.
            let Some(window) = self.window() else {
                return;
            };
            let frame = window.frame();
            let start = objc2_app_kit::NSEvent::mouseLocation();
            loop {
                let Some(next) = window.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::LeftMouseUp | NSEventMask::LeftMouseDragged,
                    Some(&objc2_foundation::NSDate::distantFuture()),
                    unsafe { objc2_app_kit::NSEventTrackingRunLoopMode },
                    true,
                ) else {
                    break;
                };
                match next.r#type() {
                    NSEventType::LeftMouseDragged => {
                        let now = objc2_app_kit::NSEvent::mouseLocation();
                        window.setFrameOrigin(CGPoint::new(
                            frame.origin.x + (now.x - start.x),
                            frame.origin.y + (now.y - start.y),
                        ));
                    }
                    NSEventType::LeftMouseUp => break,
                    _ => {}
                }
            }
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn __canvas_can_move(&self) -> bool {
            eprintln!("PROBE: canvas mouseDownCanMoveWindow queried");
            false
        }

        #[unsafe(method(widthChanged:))]
        fn __width_changed(&self, sender: Option<&AnyObject>) {
            let value: f64 = match sender {
                Some(s) => unsafe { msg_send![s, doubleValue] },
                None => f64::NAN,
            };
            eprintln!("PROBE: widthChanged fired, value={value}");
        }

        #[unsafe(method(buttonClicked:))]
        fn __button_clicked(&self, _sender: Option<&AnyObject>) {
            eprintln!("PROBE: leaf button CLICKED (action fired)");
        }
    }
);

impl ProbeCanvas {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: designated initializer with no ivars.
        unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::ZERO) }
    }
}

// --- probe window: logs every left-mouse event and the hit-test verdict
// BEFORE dispatch, exactly what NSWindow.sendEvent decides on ---
define_class!(
    #[unsafe(super(WindowBase))]
    #[thread_kind = MainThreadOnly]
    struct ProbeWindow;

    unsafe impl NSObjectProtocol for ProbeWindow {}

    impl ProbeWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn __can_become_key_window(&self) -> bool {
            true
        }

        #[unsafe(method(performWindowDragWithEvent:))]
        unsafe fn __perform_window_drag(&self, event: &NSEvent) {
            if !PANEL_DRAG_ALLOWED.load(std::sync::atomic::Ordering::SeqCst) {
                eprintln!(
                    "PROBE: system performWindowDragWithEvent IGNORED at {:?}",
                    event.locationInWindow()
                );
                return;
            }
            eprintln!(
                "PROBE: panel-initiated performWindowDragWithEvent at {:?}",
                event.locationInWindow()
            );
            // SAFETY: pass-through to the superclass implementation.
            unsafe { msg_send![super(self), performWindowDragWithEvent: event] }
        }

        #[unsafe(method(sendEvent:))]
        unsafe fn __send_event(&self, event: &NSEvent) {
            let t = event.r#type();
            if matches!(
                t,
                NSEventType::LeftMouseDown
                    | NSEventType::LeftMouseDragged
                    | NSEventType::LeftMouseUp
            ) {
                let loc = event.locationInWindow();
                let hit = self
                    .contentView()
                    .and_then(|content| content.hitTest(loc));
                let name = hit
                    .map(|v| String::from_utf8_lossy(v.class().name().to_bytes()).into_owned())
                    .unwrap_or_else(|| "<nil>".into());
                eprintln!("PROBE: sendEvent {t:?} at {loc:?} hit-test -> {name}");
            }
            // SAFETY: pass-through to the superclass implementation.
            unsafe { msg_send![super(self), sendEvent: event] }
        }
    }
);

impl ProbeWindow {
    extern_methods!(
        #[unsafe(method(initWithContentRect:styleMask:backing:defer:))]
        unsafe fn init_with_content_rect(
            this: Allocated<Self>,
            content_rect: CGRect,
            style: NSWindowStyleMask,
            backing: NSBackingStoreType,
            defer: bool,
        ) -> Retained<Self>;
    );
}

fn class_name(v: Option<Retained<objc2_app_kit::NSView>>) -> String {
    v.map(|v| String::from_utf8_lossy(v.class().name().to_bytes()).into_owned())
        .unwrap_or_else(|| "<nil>".into())
}

fn main() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let screens = objc2_app_kit::NSScreen::screens(mtm);
    let screen = screens.objectAtIndex(screens.len() - 1);
    let frame = screen.frame();
    let (w, h) = (INITIAL_SIZE.width, INITIAL_SIZE.height + TOOLBAR_HEIGHT);
    let x = frame.origin.x + (frame.size.width - w) / 2.0;
    let y = frame.origin.y + (frame.size.height - h) / 2.0;

    let window = unsafe {
        ProbeWindow::init_with_content_rect(
            ProbeWindow::alloc(mtm),
            CGRect::new(CGPoint::new(x, y), CGSize::new(w, h)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::FullSizeContentView
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setTitlebarAppearsTransparent(true);
    window.setTitleVisibility(objc2_app_kit::NSWindowTitleVisibility::Hidden);
    // v13: movability OFF for good. The system titlebar-drag interception
    // in the full-size-content-view strip stops jumping the window after
    // the slider's tracking loop; the panel and the canvas still drag the
    // window through their explicit performWindowDragWithEvent calls.
    window.setMovable(false);
    window.setCollectionBehavior(
        objc2_app_kit::NSWindowCollectionBehavior::CanJoinAllSpaces,
    );
    window.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
        WINDOW_BG[0], WINDOW_BG[1], WINDOW_BG[2], WINDOW_BG[3],
    )));
    unsafe {
        window.setReleasedWhenClosed(false);
    }

    // --- the toolbar tree, same construction order as EditorWindow::new ---
    let canvas = ProbeCanvas::new(mtm);
    let panel = ProbePanel::new(mtm);
    let toolbar = ProbeStack::new(mtm);
    toolbar.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Horizontal);
    toolbar.setSpacing(BAR_SPACING);

    let leaf = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str("L"), None, None, mtm)
    };
    leaf.setBordered(false);
    leaf.setTranslatesAutoresizingMaskIntoConstraints(false);
    unsafe {
        leaf.setTarget(Some(&canvas));
        leaf.setAction(Some(sel!(buttonClicked:)));
        leaf.widthAnchor()
            .constraintEqualToConstant(TOOLBAR_BUTTON_SIZE.width)
            .setActive(true);
        leaf.heightAnchor()
            .constraintEqualToConstant(TOOLBAR_BUTTON_SIZE.height)
            .setActive(true);
    }
    toolbar.addArrangedSubview(&leaf);

    let file_label =
        objc2_app_kit::NSTextField::labelWithString(&NSString::from_str("shot.png"), mtm);
    file_label.setTranslatesAutoresizingMaskIntoConstraints(false);
    unsafe {
        file_label
            .widthAnchor()
            .constraintEqualToConstant(FILE_LABEL_WIDTH)
            .setActive(true);
    }
    toolbar.addArrangedSubview(&file_label);

    let swatch = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str(""), None, None, mtm)
    };
    swatch.setBordered(false);
    swatch.setTranslatesAutoresizingMaskIntoConstraints(false);
    unsafe {
        swatch
            .widthAnchor()
            .constraintEqualToConstant(TOOLBAR_BUTTON_SIZE.width)
            .setActive(true);
        swatch
            .heightAnchor()
            .constraintEqualToConstant(TOOLBAR_BUTTON_SIZE.height)
            .setActive(true);
    }
    toolbar.addArrangedSubview(&swatch);

    let slider = WidthSliderView::new(mtm);
    slider.setTranslatesAutoresizingMaskIntoConstraints(false);
    unsafe {
        slider.set_target(Some(&*canvas));
        slider.set_action(Some(sel!(widthChanged:)));
    }
    toolbar.addArrangedSubview(&slider);
    toolbar.setCustomSpacing_afterView(BAR_ZONE_SPACING, &slider);

    let content = window.contentView().expect("content view");
    let size = content.bounds().size;
    panel.setFrame(CGRect::new(
        CGPoint::new(0.0, size.height - TOOLBAR_HEIGHT),
        CGSize::new(size.width, TOOLBAR_HEIGHT),
    ));
    panel.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewMinYMargin,
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
    toolbar.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewMinYMargin,
    );
    panel.addSubview(&toolbar);
    canvas.setFrame(CGRect::new(
        CGPoint::ZERO,
        CGSize::new(size.width, size.height - TOOLBAR_HEIGHT),
    ));
    canvas.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    content.addSubview(&canvas);

    extern "C" {
        fn objc2_app_kit_width_changed();
    }

    window.makeKeyAndOrderFront(None);
    window.setFrameOrigin(CGPoint::new(x, y));
    app.activate();
    app.activateIgnoringOtherApps(true);

    // Delayed diagnostics: after layout has run, dump the geometry and the
    // hit-test verdict over the slider's center, then keep ticking to catch
    // window moves.
    let win = window.clone();
    let block: block2::RcBlock<dyn Fn(NonNull<objc2_foundation::NSTimer>)> =
        block2::RcBlock::new(move |_timer: NonNull<objc2_foundation::NSTimer>| {
            static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let panel_f = panel.frame();
            let stack_f = toolbar.frame();
            let slider_f = slider.frame();
            let leaf_f = leaf.frame();
            eprintln!(
                "PROBE: frames panel={panel_f:?} stack={stack_f:?} slider={slider_f:?} leaf={leaf_f:?}"
            );
            let slider_bounds = slider.bounds();
            let center = CGPoint::new(
                slider_bounds.size.width / 2.0,
                slider_bounds.size.height / 2.0,
            );
            let in_window = slider.convertPoint_toView(center, None);
            let content = win.contentView().expect("content view");
            let hit = content.hitTest(in_window);
            eprintln!(
                "PROBE: slider center in window = {in_window:?}, contentView.hitTest -> {}",
                class_name(hit)
            );
            let wf = win.frame();
            eprintln!("PROBE: window frame=({},{},{}x{}) ready for clicks",
                wf.origin.x, wf.origin.y, wf.size.width, wf.size.height);
            // Screen coords (CGEvent space): flip Y from the AppKit screen space.
            let screens = objc2_app_kit::NSScreen::screens(MainThreadMarker::new().unwrap());
            let main_h = screens.objectAtIndex(0).frame().size.height;
            let gx = wf.origin.x + in_window.x;
            let gy = main_h - (wf.origin.y + in_window.y);
            eprintln!("PROBE: CGEvent click point = {gx:.0},{gy:.0}");
        });
    });
    unsafe {
        objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_repeats_block(
            0.5,
            true,
            &block,
        );
    }

    // Log window moves: didMoveNotification arrives on drags.
    let center = unsafe {
        objc2_foundation::NSNotificationCenter::defaultCenter()
    };
    let win_for_note = window.clone();
    let note_block = block2::RcBlock::new(move |_note: NonNull<NSNotification>| {
        let wf = win_for_note.frame();
        eprintln!(
            "PROBE: WINDOW MOVED to ({},{})",
            wf.origin.x, wf.origin.y
        );
    });
    unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(&NSString::from_str("NSWindowDidMoveNotification")),
            Some(&window),
            None,
            &note_block,
        );
    }

    app.run();
}

