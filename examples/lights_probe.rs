//! editor-system-buttons task 1.1 harness: a standalone window with the
//! editor's subclass mechanism (traffic-light intercepts on the window,
//! style mask without `FullScreen`) to verify with real mouse clicks.
//! Run: cargo run --example lights_probe — click the lights, watch stderr.

use objc2::define_class;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{extern_methods, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSScreen, NSWindow,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSString;

define_class!(
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    struct ProbeWindow;

    unsafe impl NSObjectProtocol for ProbeWindow {}

    impl ProbeWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn __can_become_key_window(&self) -> bool {
            true
        }

        #[unsafe(method(miniaturize:))]
        fn __miniaturize(&self, sender: Option<&AnyObject>) {
            eprintln!("PROBE: miniaturize: override fired -> orderOut:");
            self.orderOut(sender);
        }

        #[unsafe(method(performMiniaturize:))]
        fn __perform_miniaturize(&self, sender: Option<&AnyObject>) {
            eprintln!("PROBE: performMiniaturize: override fired -> orderOut:");
            self.orderOut(sender);
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

fn main() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    // Center on the second screen when there is one: the shared dev machine
    // runs parallel sessions whose windows live on the first screen.
    let screens = NSScreen::screens(mtm);
    let screen = screens.objectAtIndex(screens.len() - 1);
    let frame = screen.frame();
    let (w, h) = (700.0, 500.0);
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
    window.setTitle(&NSString::from_str("lights-probe"));
    // Mirror the editor chrome: the lights draw straight on the window
    // background — no Liquid Glass capsule around them.
    window.setTitlebarAppearsTransparent(true);
    window.setTitleVisibility(objc2_app_kit::NSWindowTitleVisibility::Hidden);
    // Parallel sessions drive fullscreen windows and switch Spaces on this
    // machine — the probe must stay visible on the current space of its
    // screen, like the editor does.
    window.setCollectionBehavior(objc2_app_kit::NSWindowCollectionBehavior::CanJoinAllSpaces);
    window.setBackgroundColor(Some(&objc2_app_kit::NSColor::colorWithSRGBRed_green_blue_alpha(
        0.92, 0.92, 0.93, 1.0,
    )));
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    eprintln!(
        "PROBE: up pid={} screen_frame=({:?}) window=({x},{y}) {w}x{h}",
        std::process::id(),
        (frame.origin.x, frame.origin.y, frame.size.width, frame.size.height),
    );
    window.makeKeyAndOrderFront(None);
    window.setFrameOrigin(CGPoint::new(x, y));
    // Inactive windows draw monochrome traffic lights; try both activation
    // calls — on recent macOS a background app may still be denied, the
    // harness handles the gray case by clicking the window body first.
    app.activate();
    app.activateIgnoringOtherApps(true);
    for i in 0..screens.len() {
        let f = screens.objectAtIndex(i).frame();
        eprintln!(
            "PROBE: screen[{i}] frame=({},{},{}x{})",
            f.origin.x, f.origin.y, f.size.width, f.size.height
        );
    }
    let wf = window.frame();
    eprintln!(
        "PROBE: window.frame=({},{},{}x{})",
        wf.origin.x, wf.origin.y, wf.size.width, wf.size.height
    );
    app.run();
}
