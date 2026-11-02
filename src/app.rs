//! Application bootstrap: process life, activation policy, run loop,
//! tray icon and the main menu.

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
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
use objc2_core_foundation::CGPoint;
use objc2_foundation::{
    NSDefaultRunLoopMode, NSNotificationCenter, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer,
};
use objc2_app_kit::NSApplicationDidChangeScreenParametersNotification;

use crate::config::Config;
use crate::editor::session;
use crate::editor::OpenShot;
use crate::editor_window::EditorWindow;
use crate::hotkey_recorder::{FeedOutcome, Recorder};
use crate::hotkeys::{self, HotkeyAction, HotkeyRegistry};
use crate::overlay::{Overlay, OverlayOutcome};
use crate::prefs;
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
pub(crate) enum MainJob {
    CaptureReady {
        result: Result<crate::capture::T0Capture, String>,
        /// Open the editor window after the usual output.
        edit: bool,
    },
    /// A finished capture produced a shot file to open in the editor.
    OpenEditor { path: std::path::PathBuf },
    /// An agent-server request arrived over HTTP; the pump hands it to the
    /// controller on the main thread and the reply goes back over the
    /// channel.
    Agent(crate::agent_server::MainJobAgent),
}

struct AppIvars {
    // NSStatusBar keeps only a weak reference to its items; holding the
    // retained copy here keeps the tray icon alive for the process lifetime.
    #[allow(dead_code)]
    status_item: Retained<NSStatusItem>,
    // Kept for re-reading the shortcuts from the preferences window.
    capture_item: Retained<NSMenuItem>,
    capture_edit_item: Retained<NSMenuItem>,
    open_latest_item: Retained<NSMenuItem>,
    // The capture action consults the config when the mode is wired up.
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
    // The hotkey recorder machine: which prefs row (if any) is recording
    // a new combination. The rules live in `hotkey_recorder.rs`.
    recorder: RefCell<Recorder>,
    // Permanent local key-event monitor that serves the recorder while it
    // records; installed once at startup.
    #[allow(dead_code)]
    monitor: RefCell<Option<Retained<AnyObject>>>,
    // Screen Recording state: whether the explanatory dialog is up (the
    // modal loop re-enters the tick, so hotkey handlers must not re-show
    // it) and whether the permission is currently granted.
    permission_dialog_visible: Cell<bool>,
    permission_granted: Cell<bool>,
    // Checkout label of this instance (branch name for a worktree build,
    // "main" otherwise); shown in the tray menu header, tooltip and the
    // preferences window title.
    checkout_label: String,
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
                crate::log!(
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
                crate::log!(
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
            let started = self.ivars().recorder.borrow_mut().begin(tag);
            if started {
                self.update_prefs_row_titles();
            }
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            // The registry record belongs to this process only while it
            // lives; an ungraceful death leaves a stale file the reader
            // prunes by pid.
            crate::agent_server::deregister(std::process::id());
            let mtm = MainThreadMarker::from(self);
            NSApplication::sharedApplication(mtm).terminate(None);
        }

        #[unsafe(method(screenParamsChanged:))]
        fn screen_params_changed(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            // Display configuration changed (monitor plugged/unplugged):
            // the frozen frame no longer matches reality — cancel the mode.
            if self.ivars().overlay.borrow().is_active() {
                crate::log!("screens changed: cancelling capture mode");
                self.ivars().overlay.borrow().cancel();
            }
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _sender: Option<&AnyObject>) {
            loop {
                let next = self.ivars().jobs_rx.borrow_mut().try_recv().ok();
                match next {
                    Some(MainJob::CaptureReady { result, edit }) => {
                        self.capture_ready(result, edit)
                    }
                    Some(MainJob::OpenEditor { path }) => self.open_editor(path),
                    Some(MainJob::Agent(job)) => {
                        let result = self.handle_agent(job.request);
                        let _ = job.reply.send(result);
                    }
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
            // A click resolution finishes the overlay from the background
            // resolver; polled here so the answer lands on the main thread.
            self.ivars().overlay.borrow().poll_resolved_click();
        }
    }

    unsafe impl NSApplicationDelegate for AppController {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            crate::log!("minisnap launched");
            // Startup preflight (7.1): explain the Screen Recording
            // permission before the user wonders why capture does nothing.
            if !crate::permission::preflight_screen_capture() {
                crate::log!("screen recording: not granted, requesting");
                // Register this exact build with TCC and let the system
                // prompt do the asking; a second dialog of our own would
                // just stack on top of it. The request is what makes the
                // build's row show up in the Screen Recording pane —
                // without it, toggling rows of older builds does nothing.
                crate::permission::request_screen_capture();
            } else {
                crate::log!("screen recording: granted");
            }
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            // Recheck when the user returns from System Settings (7.1).
            let granted = crate::permission::preflight_screen_capture();
            if granted != self.ivars().permission_granted.get() {
                crate::log!("screen recording: now {}", if granted { "granted" } else { "revoked" });
                self.ivars().permission_granted.set(granted);
            }
        }
    }
);

impl AppController {
    fn new(mtm: MainThreadMarker, config: Config, checkout_label: String) -> Retained<Self> {
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
        // The tooltip is the second surface of the instance identity:
        // hovering the tray icon names the checkout without opening the menu.
        button.setToolTip(Some(&NSString::from_str(&format!(
            "minisnap — {checkout_label}"
        ))));

        let menu = NSMenu::new(mtm);
        // Inert header naming the checkout this instance was built from;
        // the label agrees with the executable path shown by `ps -axo comm`.
        let header_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(&format!("minisnap — {checkout_label}")),
                None,
                &NSString::from_str(""),
            )
        };
        header_item.setEnabled(false);
        menu.addItem(&header_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

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
            recorder: RefCell::new(Recorder::idle()),
            monitor: RefCell::new(None),
            permission_dialog_visible: Cell::new(false),
            permission_granted: Cell::new(crate::permission::preflight_screen_capture()),
            checkout_label,
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
            None => crate::log!("prefs: cannot install the key event monitor"),
        }

        // MINISNAP_DEBUG_OPEN_EDITOR=<path>: open the editor on the shot
        // file right at startup, instead of waiting for a capture or the
        // tray — a debug affordance for testing the editor window, the
        // sibling of the tuner's MINISNAP_ARROW_DEBUG.
        if let Some(path) = std::env::var_os("MINISNAP_DEBUG_OPEN_EDITOR") {
            let _ = this.ivars().jobs_tx.send(MainJob::OpenEditor {
                path: std::path::PathBuf::from(path),
            });
        }

        // The agent server rides the same job pump: requests parsed on its
        // listener thread arrive here as MainJob::Agent on the main thread.
        let (_, identity_exe) = instance_identity();
        let identity = crate::agent_proto::AgentIdentity {
            pid: std::process::id(),
            exe: identity_exe.display().to_string(),
            // `checkout_label` moved into the ivars above; the identity
            // needs the same value.
            checkout: this.ivars().checkout_label.clone(),
            port: 0,
        };
        crate::agent_server::start(&config, identity, this.ivars().jobs_tx.clone());

        this.schedule_tick(mtm);
        this
    }

    /// Execute one agent request on the main thread and produce the reply.
    /// Every arm runs the same code path the corresponding menu item,
    /// hotkey or mouse handler runs — the agent channel adds no new
    /// behaviors, it drives the existing ones.
    fn handle_agent(
        &self,
        request: crate::agent_server::AgentRequest,
    ) -> Result<crate::agent_proto::AgentResponse, String> {
        use crate::agent_proto::AgentResponse;
        use crate::agent_server::AgentRequest;
        match request {
            AgentRequest::Ping => {
                Ok(AgentResponse::Result(crate::agent_proto::ActionResult::done(
                    "pong",
                )))
            }
            AgentRequest::State => Ok(AgentResponse::State(self.agent_state())),
            AgentRequest::Action(action) => {
                Ok(AgentResponse::Result(self.agent_action(action)))
            }
            AgentRequest::Editor(op) => Ok(AgentResponse::Result(self.agent_editor(op))),
            AgentRequest::Screenshot { target, path } => Ok(AgentResponse::Screenshot(
                self.agent_screenshot(&target, path)?,
            )),
            AgentRequest::Log { offset } => {
                let (lines, next_offset) = crate::logbuf::read_from(offset);
                Ok(AgentResponse::Log(crate::agent_proto::LogResult {
                    lines,
                    next_offset,
                }))
            }
        }
    }

    /// Produce a screenshot PNG: `composite` is the pure document render,
    /// `window` draws the view hierarchy (works occluded and across
    /// Spaces). The path defaults under the temp dir.
    fn agent_screenshot(
        &self,
        target: &str,
        path: Option<String>,
    ) -> Result<crate::agent_proto::ScreenshotResult, String> {
        use crate::agent_proto::ScreenshotResult;
        let editor = self.ivars().editor.borrow();
        let Some(editor) = editor.as_ref() else {
            return Err("the editor window is not open; nothing to screenshot".into());
        };
        let path = match path {
            Some(path) => std::path::PathBuf::from(path),
            None => {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                std::env::temp_dir()
                    .join(format!("minisnap-agent-{}", std::process::id()))
                    .join(format!("{target}-{stamp}.png"))
            }
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let (width, height) = match target {
            "composite" => editor.agent_composite_png(&path)?,
            "window" => {
                let (bytes, width, height) = editor.agent_window_png()?;
                std::fs::write(&path, bytes)
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                (width, height)
            }
            other => {
                return Err(format!(
                    "unknown screenshot target '{other}' (composite|window)"
                ))
            }
        };
        Ok(ScreenshotResult {
            target: target.to_string(),
            path: path.display().to_string(),
            width,
            height,
        })
    }

    /// Execute an app-level agent action; the result says what happened
    /// ("initiated" for the async capture, "rejected" for a blocked one),
    /// never just "accepted".
    fn agent_action(
        &self,
        action: crate::agent_proto::AgentAction,
    ) -> crate::agent_proto::ActionResult {
        use crate::agent_proto::{AgentAction, ActionResult};
        match action {
            AgentAction::Capture { edit } => self.agent_capture(edit),
            AgentAction::OpenLatest => {
                let dir = self.ivars().config.borrow().save_dir.clone();
                match crate::output::latest_shot(&dir) {
                    Some(path) => {
                        self.open_editor(path);
                        ActionResult::done("opened the latest shot in the editor")
                    }
                    None => ActionResult::noop("no shots in the save folder yet"),
                }
            }
            AgentAction::OpenEditor { path } => {
                let path = std::path::PathBuf::from(path);
                if !path.is_file() {
                    ActionResult::rejected(format!("no such file: {}", path.display()))
                } else {
                    self.open_editor(path);
                    ActionResult::done("editor opened (or focused) on the shot")
                }
            }
            AgentAction::CloseEditor => {
                let editor = self.ivars().editor.borrow();
                match editor.as_ref() {
                    Some(editor) => {
                        editor.window.performClose(None);
                        ActionResult::done("editor window closed (the window object stays for the next shot)")
                    }
                    None => ActionResult::noop("no editor window open"),
                }
            }
            AgentAction::OverlaySelect { x, y, w, h } => {
                match self.ivars().overlay.borrow().agent_select(x, y, w, h) {
                    Ok(()) => ActionResult::done(
                        "overlay finished with the selection; the output stage (crop, shutter, clipboard, file) already ran",
                    ),
                    Err(e) => ActionResult::rejected(e),
                }
            }
            AgentAction::OverlayCancel => {
                if self.ivars().overlay.borrow().agent_cancel() {
                    ActionResult::done("overlay cancelled")
                } else {
                    ActionResult::noop("no overlay active")
                }
            }
            AgentAction::PrefsOpen => {
                self.open_prefs_window();
                ActionResult::done("preferences opened (or focused)")
            }
            AgentAction::PrefsClose => {
                let prefs = self.ivars().prefs.borrow();
                match prefs.as_ref() {
                    Some(prefs) => {
                        prefs.window.orderOut(None);
                        ActionResult::done("preferences window hidden")
                    }
                    None => ActionResult::noop("no preferences window open"),
                }
            }
            AgentAction::Quit => {
                crate::agent_server::deregister(std::process::id());
                let mtm = MainThreadMarker::from(self);
                NSApplication::sharedApplication(mtm).terminate(None);
                ActionResult::done("terminating")
            }
        }
    }

    /// Agent capture: same path as the tray item, except a missing Screen
    /// Recording permission is reported in the result instead of popping
    /// the explanatory modal on the user's screen.
    fn agent_capture(&self, edit: bool) -> crate::agent_proto::ActionResult {
        use crate::agent_proto::ActionResult;
        if !crate::permission::preflight_screen_capture() {
            return ActionResult::rejected(
                "screen recording permission is not granted; grant it in System Settings first",
            );
        }
        if self.ivars().overlay.borrow().is_active() {
            return ActionResult::rejected("the capture overlay is already active");
        }
        self.start_capture(edit);
        ActionResult::initiated(
            "capture started; the overlay opens when the frozen frame is ready (poll /state)",
        )
    }

    /// Execute an editor agent operation on the open document; an absent
    /// editor is an error, never a silent no-op.
    fn agent_editor(
        &self,
        op: crate::agent_proto::AgentEditorOp,
    ) -> crate::agent_proto::ActionResult {
        use crate::agent_proto::{ActionResult, AgentEditorOp};
        let editor = self.ivars().editor.borrow();
        let Some(editor) = editor.as_ref() else {
            return ActionResult::rejected("the editor window is not open");
        };
        match op {
            AgentEditorOp::SetTool { tool } => {
                let tool = match tool.as_deref().map(crate::agent_proto::parse_tool) {
                    Some(None) => return ActionResult::rejected("unknown tool name"),
                    other => other.flatten(),
                };
                editor.agent_set_tool(tool);
                ActionResult::done(match tool {
                    Some(_) => "tool set",
                    None => "tool deselected (viewing mode)",
                })
            }
            AgentEditorOp::SetColor { rgba } => {
                editor.agent_set_color(rgba);
                ActionResult::done("color set (a selected element was recolored)")
            }
            AgentEditorOp::SetWidth { width } => {
                editor.agent_set_width(width);
                ActionResult::done("width set (a selected stroke-bearing element was restyled)")
            }
            AgentEditorOp::Command { command } => match editor.agent_command(command) {
                Ok(()) => ActionResult::done("command applied; the shot file and session journal saved"),
                Err(e) => ActionResult::rejected(e),
            },
            AgentEditorOp::Undo => match editor.agent_undo() {
                Ok(true) => ActionResult::done("undone"),
                Ok(false) => ActionResult::noop("nothing to undo"),
                Err(e) => ActionResult::rejected(e),
            },
            AgentEditorOp::Redo => match editor.agent_redo() {
                Ok(true) => ActionResult::done("redone"),
                Ok(false) => ActionResult::noop("nothing to redo"),
                Err(e) => ActionResult::rejected(e),
            },
            AgentEditorOp::Select { id } => match editor.agent_select(id) {
                Ok(()) => ActionResult::done(match id {
                    Some(_) => "element selected",
                    None => "selection dropped",
                }),
                Err(e) => ActionResult::rejected(e),
            },
            AgentEditorOp::Page { direction } => {
                let delta = match direction.as_str() {
                    "prev" => -1,
                    "next" => 1,
                    _ => {
                        return ActionResult::rejected(format!(
                            "unknown direction '{direction}' (prev|next)"
                        ))
                    }
                };
                editor.agent_page(delta);
                ActionResult::done("paged to the neighbor shot (the edges stop)")
            }
        }
    }

    /// Assemble the full application state: what is open, what the
    /// document holds, what the config says. Pure read over the ivars.
    fn agent_state(&self) -> crate::agent_proto::AppState {
        use crate::agent_proto::{AppState, ConfigSummary};

        let editor = self
            .ivars()
            .editor
            .borrow()
            .as_ref()
            .map(|editor| editor.agent_state());
        let overlay_active = self.ivars().overlay.borrow().is_active();
        let prefs_open = self.ivars().prefs.borrow().is_some();
        let mut windows = Vec::new();
        if let Some(editor) = self.ivars().editor.borrow().as_ref() {
            windows.push(agent_window_info("editor", &editor.window));
        }
        if let Some(prefs) = self.ivars().prefs.borrow().as_ref() {
            windows.push(agent_window_info("preferences", &prefs.window));
        }
        let config = self.ivars().config.borrow();
        let config = ConfigSummary {
            capture_hotkey: hotkeys::render_combo(config.capture_hotkey.mods, config.capture_hotkey.key),
            capture_edit_hotkey: hotkeys::render_combo(
                config.capture_edit_hotkey.mods,
                config.capture_edit_hotkey.key,
            ),
            open_hotkey: hotkeys::render_combo(config.open_hotkey.mods, config.open_hotkey.key),
            save_dir: config.save_dir.display().to_string(),
            retention: config.retention,
        };
        AppState {
            editor,
            overlay_active,
            prefs_open,
            windows,
            config,
        }
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
            *slot = Some(prefs::build(
                mtm,
                &capture_combo,
                &edit_combo,
                &open_combo,
                &self.ivars().checkout_label,
            ));
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
             Recording. If minisnap is not in the list, add it with the + \
             button. macOS shows the permission prompt at most once a month. \
             If capture still does not work afterwards, quit and reopen \
             minisnap.",
        ));
        alert.setAlertStyle(NSAlertStyle::Informational);
        alert.addButtonWithTitle(&NSString::from_str("Open System Settings"));
        alert.addButtonWithTitle(&NSString::from_str("Not Now"));
        crate::log!("permission: running modal…");
        let response = alert.runModal();
        crate::log!("permission: modal returned {response}");
        self.ivars().permission_dialog_visible.set(false);
        if response == NSAlertFirstButtonReturn {
            crate::permission::open_screen_recording_settings();
        }
    }

    /// Local key-event monitor callback. While a row records, the first
    /// keydown fixes the new combination (the event is swallowed); a bare
    /// Esc cancels. Everything passes through untouched when not recording.
    /// NSEvent unpacking only: the recording rules live in the
    /// `hotkey_recorder` machine.
    fn recorded_key_event(&self, event: NonNull<NSEvent>) -> *mut NSEvent {
        let pointer = event.as_ptr();
        let event = unsafe { event.as_ref() };
        if event.r#type() != NSEventType::KeyDown {
            return pointer;
        }
        let mods = hotkeys::modifiers_from_flags(event.modifierFlags());
        let keycode = event.keyCode();
        // The borrow must end before the arms run: finishing a recording
        // re-reads the machine for the row titles.
        let outcome = self.ivars().recorder.borrow_mut().feed(mods, keycode);
        match outcome {
            FeedOutcome::Passthrough => pointer,
            FeedOutcome::Swallowed => {
                // A bare key or an unknown key code: not a combination.
                crate::log!("prefs: key code {keycode:#04x} swallowed, still recording");
                std::ptr::null_mut()
            }
            FeedOutcome::Cancelled => {
                crate::log!("prefs: recording cancelled");
                self.finish_recording(None);
                std::ptr::null_mut()
            }
            FeedOutcome::Recorded { row, hotkey } => {
                self.finish_recording(Some((row, hotkey)));
                std::ptr::null_mut()
            }
        }
    }

    /// Apply (or cancel) a recorded combination: persist the config,
    /// re-register the global hotkeys and refresh menu + row titles. The
    /// recorder machine has already ended the recording; this applies the
    /// result.
    fn finish_recording(&self, recorded: Option<(isize, HotKey)>) {
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
                    crate::log!("prefs: cannot save config: {e}");
                }
            }
            self.ivars()
                .hotkeys
                .borrow_mut()
                .apply(config.capture_hotkey, config.capture_edit_hotkey, config.open_hotkey);
            Self::apply_menu_hotkey(&self.ivars().capture_item, &config.capture_hotkey);
            Self::apply_menu_hotkey(&self.ivars().capture_edit_item, &config.capture_edit_hotkey);
            Self::apply_menu_hotkey(&self.ivars().open_latest_item, &config.open_hotkey);
            crate::log!(
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
        let recording = self.ivars().recorder.borrow().recording_row();
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
            None => crate::log!("open latest: no shots yet"),
        }
    }

    /// Open (or focus) the editor window on a shot file. Created lazily and
    /// reused: every shot replaces the content of the same window.
    fn open_editor(&self, path: std::path::PathBuf) {
        let mtm = MainThreadMarker::from(self);
        let sessions_root = session::default_root();
        let Some(shot) = OpenShot::open(path, &sessions_root) else {
            crate::log!("editor: cannot open shot");
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

    /// Start the frozen-area capture: freeze every display at T0 and open
    /// the overlay on the one under the cursor. With `edit`, the shot opens
    /// in the editor window after the usual output.
    fn start_capture(&self, edit: bool) {
        // No permission — no capture: explain instead (7.2). The preflight
        // here also re-checks after the user granted the permission in
        // System Settings without a relaunch.
        if !crate::permission::preflight_screen_capture() {
            crate::log!("capture blocked: screen recording not granted");
            self.show_permission_dialog();
            return;
        }
        if self.ivars().overlay.borrow().is_active() {
            crate::log!("capture hotkey ignored: overlay already active");
            return;
        }
        let mtm = MainThreadMarker::from(self);
        let primary_height = crate::capture::primary_screen_height(mtm);
        let mouse = NSEvent::mouseLocation();
        let mouse_sckit = crate::capture::geometry::point_to_sckit(
            CGPoint::new(mouse.x, mouse.y),
            primary_height,
        );
        crate::log!(
            "capture: mouse_sckit=({:.0},{:.0})",
            mouse_sckit.x, mouse_sckit.y
        );

        let jobs_tx = self.ivars().jobs_tx.clone();
        std::thread::spawn(move || {
            objc2::rc::autoreleasepool(|_pool| {
                let result = crate::capture::capture_t0(mouse_sckit);
                let _ = jobs_tx.send(MainJob::CaptureReady { result, edit });
            });
        });
    }

    /// T0 data arrived: enter the frozen-frame overlay mode. Main thread.
    fn capture_ready(&self, result: Result<crate::capture::T0Capture, String>, edit: bool) {
        let Ok(t0) = result else {
            if let Err(e) = &result {
                crate::log!("capture: {e}");
            }
            return;
        };
        let overlay = self.ivars().overlay.borrow();
        if overlay.is_active() {
            crate::log!("capture: t0 ready but mode already active, dropping");
            return;
        }
        let (save_dir, retention) = {
            let config = self.ivars().config.borrow();
            (config.save_dir.clone(), config.retention)
        };
        let jobs_tx = self.ivars().jobs_tx.clone();
        overlay.show(t0, Box::new(move |outcome| match outcome {
            OverlayOutcome::Selected {
                pixel_rect,
                image,
                scale,
            } => {
                let Some(image) = crate::output::crop(&image, pixel_rect) else {
                    crate::log!("output: empty selection rect {pixel_rect:?}");
                    return;
                };
                crate::sound::play_shutter();
                if let Err(e) = crate::output::copy_to_pasteboard(&image) {
                    crate::log!("output: pasteboard: {e}");
                }
                match crate::output::save_to_file(&image, &save_dir, retention, scale) {
                    Ok(path) => {
                        crate::log!("output: saved {}", path.display());
                        if edit {
                            let _ = jobs_tx.send(MainJob::OpenEditor { path });
                        }
                    }
                    Err(e) => crate::log!("output: save: {e}"),
                }
            }
            OverlayOutcome::Cancelled => {}
        }));
    }
}

/// One window for the agent state: the Cocoa frame (origin bottom-left)
/// converted to the top-left origin the agent protocol speaks — the same
/// space screenshots and canvas coordinates live in.
fn agent_window_info(
    title: &str,
    window: &objc2_app_kit::NSWindow,
) -> crate::agent_proto::WindowInfo {
    let frame = window.frame();
    let screen_height = window
        .screen()
        .map(|screen| screen.frame().size.height)
        .unwrap_or(frame.origin.y + frame.size.height);
    crate::agent_proto::WindowInfo {
        title: title.to_string(),
        x: frame.origin.x,
        y: screen_height - frame.origin.y - frame.size.height,
        width: frame.size.width,
        height: frame.size.height,
        visible: window.isVisible(),
    }
}

/// Checkout identity of a running instance, derived from its executable
/// path: a binary under `.worktrees/<branch>/` labels itself with the
/// branch name, any other path is the main checkout. The label must agree
/// with what `ps -axo comm` shows for the same process.
fn checkout_label(exe: &Path) -> String {
    let mut components = exe.components();
    while let Some(component) = components.next() {
        if component.as_os_str() == std::ffi::OsStr::new(".worktrees") {
            if let Some(branch) = components.next() {
                return branch.as_os_str().to_string_lossy().into_owned();
            }
        }
    }
    "main".to_owned()
}

/// This instance's identity: the checkout label plus the canonical
/// executable path it was launched from.
fn instance_identity() -> (String, PathBuf) {
    let exe = std::env::current_exe()
        .map(|exe| fs::canonicalize(&exe).unwrap_or(exe))
        .unwrap_or_else(|_| PathBuf::from("<unknown>"));
    let label = checkout_label(&exe);
    (label, exe)
}

/// Boot the NSApplication as a menu-bar agent (no Dock icon, no windows),
/// install the tray icon with its menu and hand control to the run loop.
pub fn run() {
    let mtm = MainThreadMarker::new().expect("minisnap must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let (checkout_label, exe_path) = instance_identity();
    crate::log!(
        "minisnap instance '{}' from {}",
        checkout_label,
        exe_path.display()
    );

    let config = Config::load_or_create();
    crate::log!(
        "config: capture={} open={} save_dir={} retention={}",
        config.capture_hotkey,
        config.open_hotkey,
        config.save_dir.display(),
        config.retention
    );

    let controller = AppController::new(mtm, config, checkout_label);
    app.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    // NSMenuItem targets and the app delegate hold weak references; the
    // controller (and its overlay) live for the whole process.
    std::mem::forget(controller);

    // The shutter warm-up must not delay the launch itself; one runloop
    // turn in, the tray is up and a second of audio init is invisible.
    let warm = RcBlock::new(move |_timer| crate::sound::prewarm());
    // SAFETY: the block only calls sound::prewarm, which self-gates on the
    // main thread this timer fires on.
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0, false, &warm) };

    app.run();
}

#[cfg(test)]
mod tests {
    use super::checkout_label;
    use std::path::Path;

    #[test]
    fn label_from_worktree_bundle_path() {
        let exe = Path::new(
            "/Users/morr/develop/minisnap/.worktrees/editor-filename-label/\
             target/release/minisnap.app/Contents/MacOS/minisnap",
        );
        assert_eq!(checkout_label(exe), "editor-filename-label");
    }

    #[test]
    fn label_from_main_checkout_bundle_path() {
        let exe = Path::new(
            "/Users/morr/develop/minisnap/target/release/minisnap.app/\
             Contents/MacOS/minisnap",
        );
        assert_eq!(checkout_label(exe), "main");
    }

    #[test]
    fn label_from_path_without_worktrees() {
        let exe = Path::new("/Applications/minisnap.app/Contents/MacOS/minisnap");
        assert_eq!(checkout_label(exe), "main");
    }

    #[test]
    fn label_from_bare_worktree_binary_path() {
        let exe = Path::new("/Users/morr/develop/minisnap/.worktrees/overlay-follows-cursor/target/debug/minisnap");
        assert_eq!(checkout_label(exe), "overlay-follows-cursor");
    }

    #[test]
    fn label_when_worktrees_is_the_last_component() {
        let exe = Path::new("/Users/morr/develop/minisnap/.worktrees");
        assert_eq!(checkout_label(exe), "main");
    }
}
