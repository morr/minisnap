//! Application bootstrap: process life, activation policy, run loop,
//! tray icon and the main menu.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::sync::mpsc;

use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication,
    NSApplicationActivationPolicy, NSApplicationDelegate, NSEvent, NSEventMask, NSEventType,
    NSFontWeightSemibold, NSImage, NSImageSymbolConfiguration, NSMenu, NSMenuItem, NSStatusBar,
    NSStatusItem,
};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{
    NSDefaultRunLoopMode, NSNotificationCenter, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer,
};
use objc2_app_kit::NSApplicationDidChangeScreenParametersNotification;

use crate::config::Config;
use crate::editor::session;
use crate::editor::OpenShot;
use crate::editor_window::EditorWindow;
use crate::hotkeys::{self, HotkeyAction, HotkeyRegistry};
use crate::overlay::{Overlay, OverlayOutcome};
use crate::prefs;
use global_hotkey::hotkey::{HotKey, Modifiers};

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
        /// Open the editor window after the usual output.
        edit: bool,
    },
    /// A finished capture produced a shot file to open in the editor.
    OpenEditor { path: std::path::PathBuf },
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
    capture_edit_item: Retained<NSMenuItem>,
    #[allow(dead_code)]
    open_latest_item: Retained<NSMenuItem>,
    // The capture action consults the config when the mode is wired up.
    #[allow(dead_code)]
    config: RefCell<Config>,
    hotkeys: RefCell<HotkeyRegistry>,
    jobs_tx: mpsc::Sender<MainJob>,
    jobs_rx: RefCell<mpsc::Receiver<MainJob>>,
    overlay: RefCell<Overlay>,
    // Created lazily on the first Preferences click; kept afterwards so
    // reopening focuses the existing window.
    prefs: RefCell<Option<prefs::PrefsWindow>>,
    // Created lazily on the first editor-relevant capture; kept afterwards
    // so every shot reuses the same window (no second editor window).
    editor: RefCell<Option<EditorWindow>>,
    // The hotkey row currently recording a new combination, if any.
    recording: Cell<Option<isize>>,
    // Permanent local key-event monitor that serves the recorder while
    // `recording` is set; installed once at startup.
    #[allow(dead_code)]
    monitor: RefCell<Option<Retained<AnyObject>>>,
    // Screen Recording state: whether the explanatory dialog is up (the
    // modal loop re-enters the tick, so hotkey handlers must not re-show
    // it) and whether the permission is currently granted.
    permission_dialog_visible: Cell<bool>,
    permission_granted: Cell<bool>,
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
            self.start_capture(false);
        }

        #[unsafe(method(captureAndEdit:))]
        fn capture_and_edit(&self, _sender: Option<&AnyObject>) {
            self.start_capture(true);
        }

        #[unsafe(method(openLatestImage:))]
        fn open_latest_image(&self, _sender: Option<&AnyObject>) {
            self.open_latest();
        }

        #[unsafe(method(openPreferences:))]
        fn open_preferences(&self, _sender: Option<&AnyObject>) {
            self.open_prefs_window();
        }

        /// Deferred from `openPreferences:` so the activation happens after
        /// the tray menu finishes tracking — ordering a window key from
        /// inside the menu action does not stick.
        #[unsafe(method(prefsWindowToFront:))]
        fn prefs_window_to_front(&self, _sender: Option<&AnyObject>) {
            let mtm = MainThreadMarker::from(self);
            let app = NSApplication::sharedApplication(mtm);
            // Cooperative activation answers to a real user click on the
            // tray icon; the legacy call is the fallback for older systems.
            app.activate();
            app.activateIgnoringOtherApps(true);
            if let Some(prefs) = self.ivars().prefs.borrow().as_ref() {
                prefs.window.makeKeyAndOrderFront(None);
                eprintln!(
                    "prefs window: active={} key={}",
                    app.isActive(),
                    prefs.window.isKeyWindow()
                );
            }
        }

        /// Deferred from the editor opening so the activation happens after
        /// the menu/hotkey path finishes — ordering a window key from inside
        /// those paths does not stick (same scheme as the prefs window).
        #[unsafe(method(editorWindowToFront:))]
        fn editor_window_to_front(&self, _sender: Option<&AnyObject>) {
            let mtm = MainThreadMarker::from(self);
            let app = NSApplication::sharedApplication(mtm);
            app.activate();
            app.activateIgnoringOtherApps(true);
            if let Some(editor) = self.ivars().editor.borrow().as_ref() {
                editor.window.makeKeyAndOrderFront(None);
                eprintln!(
                    "editor window: active={} key={}",
                    app.isActive(),
                    editor.window.isKeyWindow()
                );
            }
        }

        #[unsafe(method(prefsRowClicked:))]
        fn prefs_row_clicked(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is the clicked NSButton; `tag` is the
            // NSView accessor we stamped on the button when building.
            let tag: isize = unsafe { msg_send![sender, tag] };
            self.start_recording(tag);
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
                        edit,
                    }) => self.capture_ready(result, screen_frame, scale, edit),
                    Some(MainJob::OpenEditor { path }) => self.open_editor(path),
                    None => break,
                }
            }
            for action in self.ivars().hotkeys.borrow().pending_actions() {
                match action {
                    HotkeyAction::Capture => self.start_capture(false),
                    HotkeyAction::CaptureEdit => self.start_capture(true),
                    HotkeyAction::OpenLatest => self.open_latest(),
                }
            }
        }
    }

    unsafe impl NSApplicationDelegate for AppController {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            eprintln!("minisnap launched");
            // Startup preflight (7.1): explain the Screen Recording
            // permission before the user wonders why capture does nothing.
            if !crate::permission::preflight_screen_capture() {
                eprintln!("screen recording: not granted, requesting");
                // Register this exact build with TCC and let the system
                // prompt do the asking; a second dialog of our own would
                // just stack on top of it. The request is what makes the
                // build's row show up in the Screen Recording pane —
                // without it, toggling rows of older builds does nothing.
                crate::permission::request_screen_capture();
            } else {
                eprintln!("screen recording: granted");
            }
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            // Recheck when the user returns from System Settings (7.1).
            let granted = crate::permission::preflight_screen_capture();
            if granted != self.ivars().permission_granted.get() {
                eprintln!("screen recording: now {}", if granted { "granted" } else { "revoked" });
                self.ivars().permission_granted.set(granted);
            }
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

        let capture_edit_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Capture and Edit"),
                Some(sel!(captureAndEdit:)),
                &NSString::from_str(""),
            )
        };
        menu.addItem(&capture_edit_item);

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
                // The menu-open shortcut; the global behavior stays with
                // the focused app (⌘Q in the editor window closes it).
                &NSString::from_str("q"),
            )
        };
        menu.addItem(&quit_item);

        status_item.setMenu(Some(&menu));
        let (jobs_tx, jobs_rx) = mpsc::channel::<MainJob>();
        let overlay = Overlay::new(mtm);
        let this = Self::alloc(mtm).set_ivars(AppIvars {
            status_item: status_item.clone(),
            capture_item: capture_item.clone(),
            capture_edit_item: capture_edit_item.clone(),
            open_latest_item: open_latest_item.clone(),
            config: RefCell::new(config.clone()),
            hotkeys: RefCell::new(HotkeyRegistry::new()),
            jobs_tx,
            jobs_rx: RefCell::new(jobs_rx),
            overlay: RefCell::new(overlay),
            prefs: RefCell::new(None),
            editor: RefCell::new(None),
            recording: Cell::new(None),
            monitor: RefCell::new(None),
            permission_dialog_visible: Cell::new(false),
            permission_granted: Cell::new(crate::permission::preflight_screen_capture()),
        });
        // SAFETY: `init` on NSObject is the designated initializer of the root
        // class and has no superclass invariants beyond zeroed ivars.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        // Menu item targets are weak (assign) references: set them on the
        // final retained controller.
        for item in [
            &capture_item,
            &capture_edit_item,
            &open_latest_item,
            &prefs_item,
            &quit_item,
        ] {
            unsafe { item.setTarget(Some(&this)) };
        }

        // Menu shortcuts come from the same config as the registrations —
        // one source of truth.
        let (capture_hk, edit_hk, open_hk) =
            (config.capture_hotkey, config.capture_edit_hotkey, config.open_hotkey);
        Self::apply_menu_hotkey(&capture_item, &capture_hk);
        Self::apply_menu_hotkey(&capture_edit_item, &edit_hk);
        Self::apply_menu_hotkey(&open_latest_item, &open_hk);

        let mut hotkeys = HotkeyRegistry::new();
        hotkeys.apply(capture_hk, edit_hk, open_hk);
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

        // The key-event monitor serves the hotkey recorder; it passes every
        // event through unless a prefs row is recording.
        let weak = Weak::new(&*this);
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            let Some(controller) = weak.load() else {
                return event.as_ptr();
            };
            controller.recorded_key_event(event)
        });
        match unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
        } {
            Some(monitor) => {
                this.ivars().monitor.borrow_mut().replace(monitor);
            }
            None => eprintln!("prefs: cannot install the key event monitor"),
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

    /// Open (or focus) the preferences window. Menu entry point.
    fn open_prefs_window(&self) {
        let mtm = MainThreadMarker::from(self);
        let mut slot = self.ivars().prefs.borrow_mut();
        if slot.is_none() {
            let (capture_combo, edit_combo, open_combo) = {
                let config = self.ivars().config.borrow();
                (
                    hotkeys::render_combo(config.capture_hotkey.mods, config.capture_hotkey.key),
                    hotkeys::render_combo(
                        config.capture_edit_hotkey.mods,
                        config.capture_edit_hotkey.key,
                    ),
                    hotkeys::render_combo(config.open_hotkey.mods, config.open_hotkey.key),
                )
            };
            *slot = Some(prefs::build(mtm, &capture_combo, &edit_combo, &open_combo));
            let window = slot.as_ref().unwrap();
            for button in [&window.capture_button, &window.capture_edit_button, &window.open_button] {
                // SAFETY: target is this controller, whose selector lives as
                // long as the process.
                unsafe {
                    button.setTarget(Some(self));
                    button.setAction(Some(sel!(prefsRowClicked:)));
                }
            }
        }
        // Ordering the window key must wait until the tray menu stops
        // tracking; a one-shot timer in the default run loop mode fires on
        // the next pass once the menu is closed.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                0.0,
                self,
                sel!(prefsWindowToFront:),
                None,
                false,
            )
        };
        unsafe {
            NSRunLoop::currentRunLoop().addTimer_forMode(&timer, &NSDefaultRunLoopMode);
        }
    }

    /// The explanatory Screen Recording dialog (7.1); also shown instead of
    /// the capture mode while the permission is missing (7.2). Runs its own
    /// modal loop; the visibility flag keeps re-entrant callers (the tick
    /// fires inside the modal run loop) from stacking a second dialog.
    fn show_permission_dialog(&self) {
        if self.ivars().permission_dialog_visible.get() {
            return;
        }
        self.ivars().permission_dialog_visible.set(true);
        let mtm = MainThreadMarker::from(self);
        let app = NSApplication::sharedApplication(mtm);
        app.activate();
        app.activateIgnoringOtherApps(true);
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str("Screen Recording Permission Needed"));
        alert.setInformativeText(&NSString::from_str(
            "minisnap freezes the screen so you can select an area to copy or \
             save. Screen Recording access is what makes that possible.\n\n\
             Grant access in System Settings — Privacy & Security — Screen \
             Recording. If capture still does not work afterwards, quit and \
             reopen minisnap.",
        ));
        alert.setAlertStyle(NSAlertStyle::Informational);
        alert.addButtonWithTitle(&NSString::from_str("Open System Settings"));
        alert.addButtonWithTitle(&NSString::from_str("Not Now"));
        eprintln!("permission: running modal…");
        let response = alert.runModal();
        eprintln!("permission: modal returned {response}");
        self.ivars().permission_dialog_visible.set(false);
        if response == NSAlertFirstButtonReturn {
            crate::permission::open_screen_recording_settings();
        }
    }

    /// Begin recording a new combination for the row with the given tag.
    fn start_recording(&self, row: isize) {
        if row != prefs::ROW_CAPTURE
            && row != prefs::ROW_CAPTURE_EDIT
            && row != prefs::ROW_OPEN_LATEST
        {
            return;
        }
        if self.ivars().recording.get().is_some() {
            return;
        }
        self.ivars().recording.set(Some(row));
        self.update_prefs_row_titles();
    }

    /// Local key-event monitor callback. While a row records, the first
    /// keydown fixes the new combination (the event is swallowed); a bare
    /// Esc cancels. Everything passes through untouched when not recording.
    fn recorded_key_event(&self, event: NonNull<NSEvent>) -> *mut NSEvent {
        let Some(row) = self.ivars().recording.get() else {
            return event.as_ptr();
        };
        let pointer = event.as_ptr();
        let event = unsafe { event.as_ref() };
        if event.r#type() != NSEventType::KeyDown {
            return pointer;
        }
        let modifiers = hotkeys::modifiers_from_flags(event.modifierFlags());
        let keycode = event.keyCode();
        if keycode == 0x35 && modifiers == Modifiers::empty() {
            // Bare Esc: cancel the recording, keep the previous combination.
            eprintln!("prefs: recording cancelled");
            self.finish_recording(None);
            return std::ptr::null_mut();
        }
        let Some(key) = hotkeys::scancode_to_key(keycode) else {
            eprintln!("prefs: unknown key code {keycode:#04x}, still recording");
            return std::ptr::null_mut();
        };
        let hotkey =
            HotKey::new(if modifiers.is_empty() { None } else { Some(modifiers) }, key);
        self.finish_recording(Some((row, hotkey)));
        std::ptr::null_mut()
    }

    /// Apply (or cancel) a recorded combination: persist the config,
    /// re-register the global hotkeys and refresh menu + row titles.
    fn finish_recording(&self, recorded: Option<(isize, HotKey)>) {
        self.ivars().recording.set(None);
        if let Some((row, hotkey)) = recorded {
            let mut config = self.ivars().config.borrow_mut();
            match row {
                prefs::ROW_CAPTURE => config.capture_hotkey = hotkey,
                prefs::ROW_CAPTURE_EDIT => config.capture_edit_hotkey = hotkey,
                prefs::ROW_OPEN_LATEST => config.open_hotkey = hotkey,
                _ => {}
            }
            if let Some(path) = Config::standard_path() {
                if let Err(e) = config.save_to(&path) {
                    eprintln!("prefs: cannot save config: {e}");
                }
            }
            self.ivars()
                .hotkeys
                .borrow_mut()
                .apply(config.capture_hotkey, config.capture_edit_hotkey, config.open_hotkey);
            Self::apply_menu_hotkey(&self.ivars().capture_item, &config.capture_hotkey);
            Self::apply_menu_hotkey(&self.ivars().capture_edit_item, &config.capture_edit_hotkey);
            Self::apply_menu_hotkey(&self.ivars().open_latest_item, &config.open_hotkey);
            eprintln!(
                "prefs: hotkeys applied: capture={} open={}",
                config.capture_hotkey, config.open_hotkey
            );
        }
        self.update_prefs_row_titles();
    }

    /// Refresh the two row buttons from the config; the row being recorded
    /// shows the prompt instead of its combination.
    fn update_prefs_row_titles(&self) {
        let prefs_guard = self.ivars().prefs.borrow();
        let Some(prefs_window) = prefs_guard.as_ref() else {
            return;
        };
        let config = self.ivars().config.borrow();
        let recording = self.ivars().recording.get();
        let capture_title = match recording {
            Some(row) if row == prefs::ROW_CAPTURE => "Press keys…".to_string(),
            _ => hotkeys::render_combo(config.capture_hotkey.mods, config.capture_hotkey.key),
        };
        let edit_title = match recording {
            Some(row) if row == prefs::ROW_CAPTURE_EDIT => "Press keys…".to_string(),
            _ => hotkeys::render_combo(
                config.capture_edit_hotkey.mods,
                config.capture_edit_hotkey.key,
            ),
        };
        let open_title = match recording {
            Some(row) if row == prefs::ROW_OPEN_LATEST => "Press keys…".to_string(),
            _ => hotkeys::render_combo(config.open_hotkey.mods, config.open_hotkey.key),
        };
        prefs_window
            .capture_button
            .setTitle(&NSString::from_str(&capture_title));
        prefs_window
            .capture_edit_button
            .setTitle(&NSString::from_str(&edit_title));
        prefs_window
            .open_button
            .setTitle(&NSString::from_str(&open_title));
    }

    /// Open the newest saved shot in the editor; an empty folder is a
    /// silent no-op. Shared by the tray menu and the open hotkey.
    fn open_latest(&self) {
        let dir = self.ivars().config.borrow().save_dir.clone();
        match crate::output::latest_shot(&dir) {
            Some(path) => self.open_editor(path),
            None => eprintln!("open latest: no shots yet"),
        }
    }

    /// Open (or focus) the editor window on a shot file. Created lazily and
    /// reused: every shot replaces the content of the same window.
    fn open_editor(&self, path: std::path::PathBuf) {
        let mtm = MainThreadMarker::from(self);
        let sessions_root = session::default_root();
        let Some(shot) = OpenShot::open(path, &sessions_root) else {
            eprintln!("editor: cannot open shot");
            return;
        };
        {
            let mut slot = self.ivars().editor.borrow_mut();
            if slot.is_none() {
                let save_dir = self.ivars().config.borrow().save_dir.clone();
                *slot = Some(EditorWindow::new(mtm, save_dir));
            }
            slot.as_ref().expect("editor window").show_shot(shot);
        }
        // Ordering the window key must wait until the tray menu or hotkey
        // path stops tracking; a one-shot timer fires on the next pass.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                0.0,
                self,
                sel!(editorWindowToFront:),
                None,
                false,
            )
        };
        unsafe {
            NSRunLoop::currentRunLoop().addTimer_forMode(&timer, &NSDefaultRunLoopMode);
        }
    }

    /// Start the frozen-area capture: pick the display under the cursor and
    /// log the T0 data (overlay and screenshot come next). With `edit`, the
    /// shot opens in the editor window after the usual output.
    fn start_capture(&self, edit: bool) {
        // No permission — no capture: explain instead (7.2). The preflight
        // here also re-checks after the user granted the permission in
        // System Settings without a relaunch.
        if !crate::permission::preflight_screen_capture() {
            eprintln!("capture blocked: screen recording not granted");
            self.show_permission_dialog();
            return;
        }
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
                    edit,
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
        edit: bool,
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
        let (save_dir, retention) = {
            let config = self.ivars().config.borrow();
            (config.save_dir.clone(), config.retention)
        };
        let jobs_tx = self.ivars().jobs_tx.clone();
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
                    match crate::output::save_to_file(&image, &save_dir, retention) {
                        Ok(path) => {
                            eprintln!("output: saved {}", path.display());
                            if edit {
                                let _ = jobs_tx.send(MainJob::OpenEditor { path });
                            }
                        }
                        Err(e) => eprintln!("output: save: {e}"),
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
