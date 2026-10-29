//! Application bootstrap: process life, activation policy, run loop,
//! tray icon and the main menu.

use std::cell::RefCell;
use std::sync::mpsc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSEvent, NSFontWeightSemibold,
    NSImage, NSImageSymbolConfiguration, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{NSNotificationCenter, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer};
use objc2_app_kit::NSApplicationDidChangeScreenParametersNotification;

use crate::config::Config;
use crate::hotkeys::{self, HotkeyAction, HotkeyRegistry};
use crate::overlay::{Overlay, OverlayOutcome};
use global_hotkey::hotkey::HotKey;

/// Placeholder status-bar icon: a monochrome template SF Symbol until the
/// brand icon appears.
const STATUS_ITEM_SYMBOL: &str = "camera.viewfinder";

/// AppKit square status item length (`NSStatusItemSquareLength`).
const STATUS_ITEM_SQUARE_LENGTH: f64 = -2.0;

/// How often the main-thread tick polls global-hotkey events, in seconds.
const TICK_INTERVAL: f64 = 0.05;

/// Owned data produced on background threads, dispatched on the main thread
/// by the tick. The controller itself is main-thread-only, so jobs carry
/// data instead of closures over it.
enum MainJob {
    CaptureReady {
        result: Result<crate::capture::T0Capture, String>,
        screen_frame: CGRect,
        scale: f64,
    },
}

struct AppIvars {
    // NSStatusBar keeps only a weak reference to its items; holding the
    // retained copy here keeps the tray icon alive for the process lifetime.
    #[allow(dead_code)]
    status_item: Retained<NSStatusItem>,
    // Kept for re-reading the shortcuts from the preferences window.
    #[allow(dead_code)]
    capture_item: Retained<NSMenuItem>,
    #[allow(dead_code)]
    open_latest_item: Retained<NSMenuItem>,
    // The capture action consults the config when the mode is wired up.
    #[allow(dead_code)]
    config: RefCell<Config>,
    hotkeys: RefCell<HotkeyRegistry>,
    jobs_tx: mpsc::Sender<MainJob>,
    jobs_rx: RefCell<mpsc::Receiver<MainJob>>,
    overlay: RefCell<Overlay>,
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

        #[unsafe(method(openLatestImage:))]
        fn open_latest_image(&self, _sender: Option<&AnyObject>) {
            eprintln!("open latest image: not implemented yet");
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

        #[unsafe(method(screenParamsChanged:))]
        fn screen_params_changed(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            // Display configuration changed (monitor plugged/unplugged):
            // the frozen frame no longer matches reality — cancel the mode.
            if self.ivars().overlay.borrow().is_active() {
                eprintln!("screens changed: cancelling capture mode");
                self.ivars().overlay.borrow().cancel();
            }
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _sender: Option<&AnyObject>) {
            loop {
                let next = self.ivars().jobs_rx.borrow_mut().try_recv().ok();
                match next {
                    Some(MainJob::CaptureReady {
                        result,
                        screen_frame,
                        scale,
                    }) => self.capture_ready(result, screen_frame, scale),
                    None => break,
                }
            }
            for action in self.ivars().hotkeys.borrow().pending_actions() {
                match action {
                    HotkeyAction::Capture => self.start_capture(),
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
        let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(STATUS_ITEM_SYMBOL),
            Some(&NSString::from_str("minisnap")),
        )
        .expect("SF Symbol image");
        // A bare symbol image renders blank on the new translucent menu bar;
        // an explicit size/weight configuration is required for it to show.
        let symbol_config = NSImageSymbolConfiguration::configurationWithPointSize_weight(
            16.0,
            unsafe { NSFontWeightSemibold },
        );
        let icon = symbol
            .imageWithSymbolConfiguration(&symbol_config)
            .expect("configured SF Symbol");
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

        let open_latest_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Open Latest Image"),
                Some(sel!(openLatestImage:)),
                &NSString::from_str(""),
            )
        };
        menu.addItem(&open_latest_item);

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
        let (jobs_tx, jobs_rx) = mpsc::channel::<MainJob>();
        let overlay = Overlay::new(mtm);
        let this = Self::alloc(mtm).set_ivars(AppIvars {
            status_item: status_item.clone(),
            capture_item: capture_item.clone(),
            open_latest_item: open_latest_item.clone(),
            config: RefCell::new(config.clone()),
            hotkeys: RefCell::new(HotkeyRegistry::new()),
            jobs_tx,
            jobs_rx: RefCell::new(jobs_rx),
            overlay: RefCell::new(overlay),
        });
        // SAFETY: `init` on NSObject is the designated initializer of the root
        // class and has no superclass invariants beyond zeroed ivars.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        // Menu item targets are weak (assign) references: set them on the
        // final retained controller.
        for item in [&capture_item, &open_latest_item, &prefs_item, &quit_item] {
            unsafe { item.setTarget(Some(&this)) };
        }

        // Menu shortcuts come from the same config as the registrations —
        // one source of truth.
        let (capture_hk, open_hk) = (config.capture_hotkey, config.open_hotkey);
        Self::apply_menu_hotkey(&capture_item, &capture_hk);
        Self::apply_menu_hotkey(&open_latest_item, &open_hk);

        let mut hotkeys = HotkeyRegistry::new();
        hotkeys.apply(config.capture_hotkey, config.open_hotkey);
        this.ivars().hotkeys.replace(hotkeys);

        // Display configuration changes cancel the overlay mode (4.7).
        // SAFETY: observer registration with a live selector on `this`.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &this,
                sel!(screenParamsChanged:),
                Some(&NSApplicationDidChangeScreenParametersNotification),
                None,
            );
        }

        this.schedule_tick(mtm);
        this
    }

    fn apply_menu_hotkey(item: &NSMenuItem, hotkey: &HotKey) {
        item.setKeyEquivalent(&NSString::from_str(&hotkeys::key_equivalent_string(hotkey.key)));
        item.setKeyEquivalentModifierMask(hotkeys::modifier_flags(hotkey.mods));
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

    /// Start the frozen-area capture: pick the display under the cursor and
    /// log the T0 data (overlay and screenshot come next).
    fn start_capture(&self) {
        if self.ivars().overlay.borrow().is_active() {
            eprintln!("capture hotkey ignored: overlay already active");
            return;
        }
        let mtm = MainThreadMarker::from(self);
        let Some(screen) = crate::capture::screen_under_cursor(mtm) else {
            eprintln!("capture: no screens found");
            return;
        };
        let screen_frame = screen.frame();
        let scale = screen.backingScaleFactor();
        let primary_height = crate::capture::primary_screen_height(mtm);
        let mouse = NSEvent::mouseLocation();
        let mouse_sckit = crate::capture::geometry::point_to_sckit(
            CGPoint::new(mouse.x, mouse.y),
            primary_height,
        );
        eprintln!(
            "capture: screen under cursor frame=({:.0},{:.0} {}x{}) scale={} mouse_sckit=({:.0},{:.0})",
            screen_frame.origin.x,
            screen_frame.origin.y,
            screen_frame.size.width,
            screen_frame.size.height,
            scale,
            mouse_sckit.x,
            mouse_sckit.y
        );

        let jobs_tx = self.ivars().jobs_tx.clone();
        std::thread::spawn(move || {
            objc2::rc::autoreleasepool(|_pool| {
                let result = crate::capture::capture_t0(mouse_sckit);
                let _ = jobs_tx.send(MainJob::CaptureReady {
                    result,
                    screen_frame,
                    scale,
                });
            });
        });
    }

    /// T0 data arrived: enter the frozen-frame overlay mode. Main thread.
    fn capture_ready(
        &self,
        result: Result<crate::capture::T0Capture, String>,
        screen_frame: CGRect,
        scale: f64,
    ) {
        let Ok(t0) = result else {
            if let Err(e) = &result {
                eprintln!("capture: {e}");
            }
            return;
        };
        let overlay = self.ivars().overlay.borrow();
        if overlay.is_active() {
            eprintln!("capture: t0 ready but mode already active, dropping");
            return;
        }
        overlay.show(
            t0,
            screen_frame,
            scale,
            Box::new(move |outcome| match outcome {
                OverlayOutcome::Selected { pixel_rect, image } => {
                    let Some(image) = crate::output::crop(&image, pixel_rect) else {
                        eprintln!("output: empty selection rect {pixel_rect:?}");
                        return;
                    };
                    if let Err(e) = crate::output::copy_to_pasteboard(&image) {
                        eprintln!("output: pasteboard: {e}");
                    }
                }
                OverlayOutcome::Cancelled => {}
            }),
        );
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
    // NSMenuItem targets and the app delegate hold weak references; the
    // controller (and its overlay) live for the whole process.
    std::mem::forget(controller);

    app.run();
}
