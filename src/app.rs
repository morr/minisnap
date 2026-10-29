//! Application bootstrap: process life, activation policy, run loop,
//! tray icon and the main menu.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSImage, NSMenu,
    NSMenuItem, NSStatusBar, NSStatusItem,
};
use objc2_foundation::NSString;

/// Placeholder status-bar icon: a monochrome template SF Symbol until the
/// brand icon appears.
const STATUS_ITEM_SYMBOL: &str = "camera.viewfinder";

/// AppKit square status item length (`NSStatusItemSquareLength`).
const STATUS_ITEM_SQUARE_LENGTH: f64 = -2.0;

struct AppIvars {
    // NSStatusBar keeps only a weak reference to its items; holding the
    // retained copy here keeps the tray icon alive for the process lifetime.
    #[allow(dead_code)]
    status_item: Retained<NSStatusItem>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppIvars]
    struct AppController;

    unsafe impl NSObjectProtocol for AppController {}

    impl AppController {
        #[unsafe(method(captureFrozenArea:))]
        fn capture_frozen_area(&self, _sender: Option<&AnyObject>) {
            eprintln!("capture frozen area: not implemented yet");
        }

        #[unsafe(method(openPreferences:))]
        fn open_preferences(&self, _sender: Option<&AnyObject>) {
            eprintln!("preferences: not implemented yet");
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let mtm = MainThreadMarker::from(self);
            NSApplication::sharedApplication(mtm).terminate(None);
        }
    }

    unsafe impl NSApplicationDelegate for AppController {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            eprintln!("minisnap launched");
        }
    }
);

impl AppController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let status_item = NSStatusBar::systemStatusBar()
            .statusItemWithLength(STATUS_ITEM_SQUARE_LENGTH);
        let button = status_item.button(mtm).expect("status item button");
        let icon = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(STATUS_ITEM_SYMBOL),
            Some(&NSString::from_str("minisnap")),
        )
        .expect("SF Symbol image");
        icon.setTemplate(true);
        button.setImage(Some(&icon));

        let this = Self::alloc(mtm).set_ivars(AppIvars {
            status_item: status_item.clone(),
        });
        // SAFETY: `init` on NSObject is the designated initializer of the root
        // class and has no superclass invariants beyond zeroed ivars.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        let menu = NSMenu::new(mtm);
        let capture_item = unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Capture Frozen Area"),
                Some(sel!(captureFrozenArea:)),
                &NSString::from_str(""),
            );
            item.setTarget(Some(&this));
            item
        };
        menu.addItem(&capture_item);

        let prefs_item = unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Preferences"),
                Some(sel!(openPreferences:)),
                &NSString::from_str(""),
            );
            item.setTarget(Some(&this));
            item
        };
        menu.addItem(&prefs_item);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let quit_item = unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Quit"),
                Some(sel!(quit:)),
                &NSString::from_str(""),
            );
            item.setTarget(Some(&this));
            item
        };
        menu.addItem(&quit_item);

        status_item.setMenu(Some(&menu));
        this
    }
}

/// Boot the NSApplication as a menu-bar agent (no Dock icon, no windows),
/// install the tray icon with its menu and hand control to the run loop.
pub fn run() {
    let mtm = MainThreadMarker::new().expect("minisnap must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let controller = AppController::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    // NSMenuItem targets and the app delegate hold weak references, so the
    // controller must outlive everything: leak the single process-wide copy.
    std::mem::forget(controller);

    app.run();
}
