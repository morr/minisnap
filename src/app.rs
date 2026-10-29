//! Application bootstrap: process life, activation policy, run loop,
//! tray icon and the main menu.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSImage, NSMenu,
    NSMenuItem, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer};

use crate::config::Config;
use crate::hotkeys::{HotkeyAction, HotkeyRegistry};

/// Placeholder status-bar icon: a monochrome template SF Symbol until the
/// brand icon appears.
const STATUS_ITEM_SYMBOL: &str = "camera.viewfinder";

/// AppKit square status item length (`NSStatusItemSquareLength`).
const STATUS_ITEM_SQUARE_LENGTH: f64 = -2.0;

/// How often the main-thread tick polls global-hotkey events, in seconds.
const TICK_INTERVAL: f64 = 0.05;

struct AppIvars {
    // NSStatusBar keeps only a weak reference to its items; holding the
    // retained copy here keeps the tray icon alive for the process lifetime.
    #[allow(dead_code)]
    status_item: Retained<NSStatusItem>,
    // Kept for re-reading the shortcuts from the preferences window.
    #[allow(dead_code)]
    capture_item: Retained<NSMenuItem>,
    // The capture action consults the config when the mode is wired up.
    #[allow(dead_code)]
    config: RefCell<Config>,
    hotkeys: RefCell<HotkeyRegistry>,
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

        #[unsafe(method(tick:))]
        fn tick(&self, _sender: Option<&AnyObject>) {
            for action in self.ivars().hotkeys.borrow().pending_actions() {
                match action {
                    HotkeyAction::Capture => {
                        eprintln!("hotkey: capture frozen area (mode not wired yet)");
                    }
                    HotkeyAction::OpenLatest => {
                        eprintln!("hotkey: open latest image (not wired yet)");
                    }
                }
            }
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
    fn new(mtm: MainThreadMarker, config: Config) -> Retained<Self> {
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

        let menu = NSMenu::new(mtm);
        let capture_item = unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Capture Frozen Area"),
                Some(sel!(captureFrozenArea:)),
                &NSString::from_str(""),
            );
            item
        };
        menu.addItem(&capture_item);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let prefs_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Preferences"),
                Some(sel!(openPreferences:)),
                &NSString::from_str(""),
            )
        };
        menu.addItem(&prefs_item);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let quit_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Quit"),
                Some(sel!(quit:)),
                &NSString::from_str(""),
            )
        };
        menu.addItem(&quit_item);

        status_item.setMenu(Some(&menu));
        let this = Self::alloc(mtm).set_ivars(AppIvars {
            status_item: status_item.clone(),
            capture_item: capture_item.clone(),
            config: RefCell::new(config.clone()),
            hotkeys: RefCell::new(HotkeyRegistry::new()),
        });
        // SAFETY: `init` on NSObject is the designated initializer of the root
        // class and has no superclass invariants beyond zeroed ivars.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        // Menu item targets are weak (assign) references: set them on the
        // final retained controller.
        for item in [&capture_item, &prefs_item, &quit_item] {
            unsafe { item.setTarget(Some(&this)) };
        }

        let mut hotkeys = HotkeyRegistry::new();
        hotkeys.apply(config.capture_hotkey, config.open_hotkey);
        this.ivars().hotkeys.replace(hotkeys);

        this.schedule_tick(mtm);
        this
    }

    fn schedule_tick(&self, _mtm: MainThreadMarker) {
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                TICK_INTERVAL,
                self,
                sel!(tick:),
                None,
                true,
            )
        };
        let run_loop = NSRunLoop::currentRunLoop();
        unsafe {
            run_loop.addTimer_forMode(&timer, &NSRunLoopCommonModes);
        }
        // The run loop retains the timer, which retains the target (self);
        // dropping our reference would fire nothing, so keep it alive until
        // the process ends.
        std::mem::forget(timer);
    }
}

/// Boot the NSApplication as a menu-bar agent (no Dock icon, no windows),
/// install the tray icon with its menu and hand control to the run loop.
pub fn run() {
    let mtm = MainThreadMarker::new().expect("minisnap must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let config = Config::load_or_create();
    eprintln!(
        "config: capture={} open={} save_dir={} retention={}",
        config.capture_hotkey,
        config.open_hotkey,
        config.save_dir.display(),
        config.retention
    );

    let controller = AppController::new(mtm, config);
    app.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    // NSMenuItem targets and the app delegate hold weak references, so the
    // controller must outlive everything: leak the single process-wide copy.
    std::mem::forget(controller);

    app.run();
}
