//! Frozen-frame overlay: a full-screen window over the captured display
//! showing the T0 frame, with selection UI layers on top. Created once at
//! app start and reused for every capture.

use std::cell::{Cell, RefCell};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{
    define_class, extern_methods, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSEvent, NSScreen, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSNotification, NSString};
use objc2_quartz_core::{CALayer, CATransaction};

use crate::capture::{T0Capture, T0Surface, T0Window};
use crate::overlay_hud::{Hud, DIM_ALPHA};

use presentation::{ScreenPair, SurfaceMap};

// The shielding window level sits above the menu bar and everything else.
extern "C" {
    fn CGShieldingWindowLevel() -> i64;
}

/// What the user did inside the overlay mode.
#[derive(Debug)]
pub enum OverlayOutcome {
    /// A final selection: the pixel rect of the T0 image plus the image
    /// itself for the crop in the output stage, and the backing scale of
    /// the display the selection finished on (the file name density
    /// suffix).
    Selected {
        pixel_rect: CGRect,
        image: Retained<CGImage>,
        scale: f64,
    },
    /// The mode was cancelled (Esc / other-display click / screen change).
    Cancelled,
}

/// What finish() was asked to do; the frozen frame attaches to the public
/// outcome inside.
#[derive(Debug, Clone, Copy)]
enum FinishRequest {
    Selected { pixel_rect: CGRect },
    Cancelled,
}

define_class!(
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    struct OverlayWindow;

    unsafe impl NSObjectProtocol for OverlayWindow {}

    impl OverlayWindow {
        // Borderless windows cannot become key by default; without this the
        // overlay would never receive keyboard events (Esc).
        #[unsafe(method(canBecomeKeyWindow))]
        fn __can_become_key_window(&self) -> bool {
            true
        }
    }
);

impl OverlayWindow {
    extern_methods!(
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool;

        #[unsafe(method(initWithContentRect:styleMask:backing:defer:))]
        unsafe fn init_with_content_rect(
            this: Allocated<Self>,
            content_rect: CGRect,
            style_mask: NSWindowStyleMask,
            backing: NSBackingStoreType,
            defer: bool,
        ) -> Retained<Self>;
    );
}

/// Interaction state inside the overlay mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayState {
    /// Moving the cursor, no button held.
    Idle,
    /// Left button held: selection rectangle is being drawn.
    Drag,
}

/// Movement tolerance (in points) within which down+up counts as a click.
const CLICK_TOLERANCE_PT: f64 = 3.0;

/// Target presentation plumbing: the per-session surface↔screen pairing
/// (`SurfaceMap`) and its queries. Pure — no AppKit calls — so it
/// unit-tests on synthetic frames; the thin `for_surfaces` constructor is
/// the only part that touches NSScreen.
mod presentation {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    use objc2_core_foundation::{CGPoint, CGRect};

    use crate::capture::{self, geometry, T0Surface};

    /// The live screen behind a surface: AppKit frame and backing scale.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct ScreenPair {
        pub(crate) frame: CGRect,
        pub(crate) scale: f64,
    }

    /// Surface↔screen pairing for one overlay session, built once at
    /// show(): every surface is matched against the live screens by its
    /// whole SCKit frame flipped into AppKit space (origin + size, small
    /// tolerance). A surface without a live screen stays unpaired and is
    /// unreachable for target switching — its display remains veiled for
    /// the whole session. The screen list cannot change mid-session (a
    /// display configuration change cancels the mode), so the pairing
    /// holds until finish().
    #[derive(Debug, Default)]
    pub(crate) struct SurfaceMap {
        pairs: Vec<Option<ScreenPair>>,
    }

    impl SurfaceMap {
        /// Pair `surfaces` with the live screens. Main thread only.
        pub(crate) fn for_surfaces(surfaces: &[T0Surface], mtm: MainThreadMarker) -> SurfaceMap {
            let screens = NSScreen::screens(mtm)
                .iter()
                .map(|screen| (screen.frame(), screen.backingScaleFactor()))
                .collect::<Vec<_>>();
            let frames = surfaces
                .iter()
                .map(|surface| surface.display_frame_sckit)
                .collect::<Vec<_>>();
            Self::from_frames(&frames, &screens, capture::primary_screen_height(mtm))
        }

        /// Pure pairing, unit-tested on synthetic frames.
        fn from_frames(
            display_frames_sckit: &[CGRect],
            screens: &[(CGRect, f64)],
            primary_height_pt: f64,
        ) -> SurfaceMap {
            SurfaceMap {
                pairs: display_frames_sckit
                    .iter()
                    .map(|display_frame| {
                        screens
                            .iter()
                            .find(|&&(screen_frame, _)| {
                                geometry::screen_matches_display(
                                    screen_frame,
                                    *display_frame,
                                    primary_height_pt,
                                    geometry::FRAME_MATCH_TOLERANCE_PT,
                                )
                            })
                            .map(|&(frame, scale)| ScreenPair { frame, scale })
                    })
                    .collect(),
            }
        }

        /// The paired screen of surface `index`, if it has one.
        pub(crate) fn screen_of(&self, index: usize) -> Option<ScreenPair> {
            self.pairs.get(index).copied().flatten()
        }

        /// The first surface other than `current` whose paired screen
        /// contains the global AppKit point (half-open frame bounds) — the
        /// target-switch candidate for the cursor position. Unpaired
        /// surfaces never match. The current target is filtered out before
        /// the search, not after: on mirrored setups two screens share a
        /// frame, and searching first would find the target itself.
        pub(crate) fn switch_candidate(&self, point: CGPoint, current: usize) -> Option<usize> {
            self.pairs
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != current)
                .find(|(_, pair)| {
                    pair.is_some_and(|p| geometry::frame_contains_point(p.frame, point))
                })
                .map(|(index, _)| index)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
            CGRect::new(CGPoint::new(x, y), objc2_core_foundation::CGSize::new(w, h))
        }

        fn pt(x: f64, y: f64) -> CGPoint {
            CGPoint::new(x, y)
        }

        #[test]
        fn pairs_surfaces_to_screens_by_full_frame() {
            // Primary 900pt tall, external 1080p above it (AppKit y=900).
            // SCKit space is 1980pt tall: external at y=0, primary at
            // y=1080.
            let screens = [(rect(0.0, 900.0, 1920.0, 1080.0), 2.0), (rect(0.0, 0.0, 1440.0, 900.0), 2.0)];
            let map = SurfaceMap::from_frames(
                &[rect(0.0, 0.0, 1920.0, 1080.0), rect(0.0, 1080.0, 1440.0, 900.0)],
                &screens,
                1980.0,
            );
            let external = map.screen_of(0).expect("external paired");
            assert_eq!(
                (external.frame.origin.x, external.frame.origin.y),
                (0.0, 900.0)
            );
            assert_eq!(external.scale, 2.0);
            let primary = map.screen_of(1).expect("primary paired");
            assert_eq!(
                (primary.frame.origin.x, primary.frame.origin.y),
                (0.0, 0.0)
            );
        }

        #[test]
        fn twin_monitors_told_apart_by_origin() {
            // Two identical monitors side by side: sizes alone cannot tell
            // them apart, the whole frame must match.
            let screens = [(rect(-1920.0, 0.0, 1920.0, 1080.0), 1.0), (rect(0.0, 0.0, 1920.0, 1080.0), 1.0)];
            let map = SurfaceMap::from_frames(
                &[rect(-1920.0, 0.0, 1920.0, 1080.0), rect(0.0, 0.0, 1920.0, 1080.0)],
                &screens,
                1080.0,
            );
            assert_eq!(map.screen_of(0).expect("left paired").frame.origin.x, -1920.0);
            assert_eq!(map.screen_of(1).expect("right paired").frame.origin.x, 0.0);
        }

        #[test]
        fn surface_without_live_screen_is_unpaired() {
            let screens = [(rect(0.0, 0.0, 1440.0, 900.0), 2.0)];
            // First surface pairs the screen; the second sits at a SCKit
            // frame no live screen matches (primary height 1980: SCKit
            // (0,900) is AppKit (0,0), outside the screen list).
            let map = SurfaceMap::from_frames(
                &[rect(0.0, 1080.0, 1440.0, 900.0), rect(0.0, 900.0, 1920.0, 1080.0)],
                &screens,
                1980.0,
            );
            assert!(map.screen_of(0).is_some());
            assert!(map.screen_of(1).is_none());
            // Out-of-range indices are unpaired too, not a panic.
            assert!(map.screen_of(2).is_none());
        }

        #[test]
        fn tolerance_absorbs_frame_rounding() {
            // The two frameworks report frames with slight rounding
            // differences; within FRAME_MATCH_TOLERANCE_PT they still pair.
            let screens = [(rect(0.0, 899.5, 1920.0, 1080.5), 2.0)];
            let map = SurfaceMap::from_frames(&[rect(0.0, 0.0, 1920.0, 1080.0)], &screens, 1980.0);
            assert!(map.screen_of(0).is_some());
        }

        #[test]
        fn switch_candidate_finds_other_target() {
            // Two 1000pt screens side by side (primary height 1000), the
            // second surface without a pair.
            let screens = [(rect(0.0, 0.0, 1000.0, 1000.0), 1.0), (rect(1000.0, 0.0, 1000.0, 1000.0), 2.0)];
            let map = SurfaceMap::from_frames(
                &[
                    rect(0.0, 0.0, 1000.0, 1000.0),
                    rect(1000.0, 0.0, 1000.0, 1000.0),
                    rect(5000.0, 0.0, 1000.0, 1000.0),
                ],
                &screens,
                1000.0,
            );
            // Cursor over the current target: nothing to switch to.
            assert_eq!(map.switch_candidate(pt(500.0, 500.0), 0), None);
            // Cursor over the second screen: its surface wins.
            assert_eq!(map.switch_candidate(pt(1500.0, 500.0), 0), Some(1));
            // The unpaired surface is never a candidate, its virtual frame
            // is not on any screen.
            assert_eq!(map.switch_candidate(pt(5000.0, 500.0), 0), None);
            // Current excluded, the first other covering surface wins.
            assert_eq!(map.switch_candidate(pt(1500.0, 500.0), 1), None);
        }

        #[test]
        fn switch_candidate_uses_half_open_bounds() {
            let screens = [(rect(0.0, 0.0, 1000.0, 1000.0), 1.0), (rect(1000.0, 0.0, 1000.0, 1000.0), 1.0)];
            let map = SurfaceMap::from_frames(
                &[rect(0.0, 0.0, 1000.0, 1000.0), rect(1000.0, 0.0, 1000.0, 1000.0)],
                &screens,
                1000.0,
            );
            // Exactly on the boundary between the screens: belongs to the
            // right one (origin in, max out).
            assert_eq!(map.switch_candidate(pt(1000.0, 500.0), 0), Some(1));
            // Past the far edge: nothing.
            assert_eq!(map.switch_candidate(pt(2000.0, 500.0), 0), None);
        }

        #[test]
        fn switch_candidate_filters_current_before_search() {
            // Mirrored setup: two screens share one frame, two surfaces
            // pair to it. With the current target filtered out first, the
            // cursor over the shared frame still finds the mirror surface.
            let screens = [(rect(0.0, 0.0, 1000.0, 1000.0), 1.0), (rect(0.0, 0.0, 1000.0, 1000.0), 1.0)];
            let map = SurfaceMap::from_frames(
                &[rect(0.0, 0.0, 1000.0, 1000.0), rect(0.0, 0.0, 1000.0, 1000.0)],
                &screens,
                1000.0,
            );
            assert_eq!(map.switch_candidate(pt(500.0, 500.0), 1), Some(0));
            assert_eq!(map.switch_candidate(pt(500.0, 500.0), 0), Some(1));
        }
    }
}

struct OverlayIvars {
    window: RefCell<Option<Retained<OverlayWindow>>>,
    veils: RefCell<Vec<Retained<NSWindow>>>,
    active: Cell<bool>,
    finishing: Cell<bool>,
    on_done: RefCell<Option<Box<dyn FnOnce(OverlayOutcome)>>>,
    /// Frozen frame per display at T0; switching the capture target walks
    /// this list.
    surfaces: RefCell<Vec<T0Surface>>,
    /// Index into `surfaces` of the current capture target.
    target: Cell<usize>,
    /// Surface↔screen pairing for the session, built once at show().
    surface_map: RefCell<SurfaceMap>,
    image: RefCell<Option<Retained<CGImage>>>,
    /// Full-image pixel size.
    image_size_px: Cell<(usize, usize)>,
    /// Captured display frame in ScreenCaptureKit global points.
    display_frame_sckit: Cell<CGRect>,
    /// Visible regular windows at capture time (T0), front-to-back, for
    /// click-to-window hit-testing.
    windows: RefCell<Vec<T0Window>>,
    /// Pending click resolution: the background thread's channel answering
    /// with the resolved pixel rect. Polled from the app tick on the main
    /// thread (SCK calls must leave the blocked main thread alone).
    resolving: RefCell<Option<std::sync::mpsc::Receiver<CGRect>>>,
    /// Overlay window frame = captured screen frame (AppKit points).
    screen_frame: Cell<CGRect>,
    scale: Cell<f64>,
    frame_layer: Retained<CALayer>,
    state: Cell<OverlayState>,
    /// Selection start point and current cursor, display-local (AppKit,
    /// bottom-left origin, points).
    drag_start: Cell<CGPoint>,
    mouse: Cell<CGPoint>,
    /// Layer tree root; attached to the view's layer on the first show.
    root_layer: Retained<CALayer>,
    layers_attached: Cell<bool>,
    /// All chrome above the frame layer: dimming, loupe, guides, readout,
    /// mode border and the frame sampler (see `overlay_hud`).
    hud: Hud,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = OverlayIvars]
    struct OverlayView;

    unsafe impl NSObjectProtocol for OverlayView {}

    impl OverlayView {
        #[unsafe(method_id(initWithFrame:))]
        unsafe fn __init_with_frame(
            this: Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self> {
            let this = this.set_ivars(OverlayIvars {
                window: RefCell::new(None),
                veils: RefCell::new(Vec::new()),
                active: Cell::new(false),
                finishing: Cell::new(false),
                on_done: RefCell::new(None),
                surfaces: RefCell::new(Vec::new()),
                target: Cell::new(0),
                surface_map: RefCell::new(SurfaceMap::default()),
                image: RefCell::new(None),
                image_size_px: Cell::new((0, 0)),
                display_frame_sckit: Cell::new(CGRect::ZERO),
                windows: RefCell::new(Vec::new()),
                screen_frame: Cell::new(CGRect::ZERO),
                scale: Cell::new(1.0),
                resolving: RefCell::new(None),
                frame_layer: CALayer::new(),
                state: Cell::new(OverlayState::Idle),
                drag_start: Cell::new(CGPoint::ZERO),
                mouse: Cell::new(CGPoint::ZERO),
                root_layer: CALayer::new(),
                layers_attached: Cell::new(false),
                hud: Hud::new(),
            });
            // SAFETY: NSView's designated initializer with plain ivars set.
            unsafe { msg_send![super(this), initWithFrame: frame_rect] }
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn __accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn __key_down(&self, event: &NSEvent) {
            if event.keyCode() == 53 {
                self.finish(FinishRequest::Cancelled);
            }
        }

        #[unsafe(method(mouseDown:))]
        fn __mouse_down(&self, event: &NSEvent) {
            self.handle_mouse_down(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn __mouse_dragged(&self, event: &NSEvent) {
            self.handle_mouse_dragged(event);
        }

        #[unsafe(method(mouseMoved:))]
        fn __mouse_moved(&self, event: &NSEvent) {
            self.handle_mouse_moved(event);
        }

        #[unsafe(method(mouseUp:))]
        fn __mouse_up(&self, event: &NSEvent) {
            self.handle_mouse_up(event);
        }

        #[unsafe(method(screenDidChange:))]
        fn __screen_did_change(&self, _notification: &NSNotification) {
            // Display configuration changed while frozen: the T0 frame no
            // longer matches the screens, so the mode is invalid.
            if self.ivars().active.get() {
                crate::log!("overlay: display configuration changed, cancelling");
                self.finish(FinishRequest::Cancelled);
            }
        }
    }
);

/// Click anywhere on a veil = cancel the whole mode.
#[derive(Default)]
struct VeilIvars {
    on_click: RefCell<Option<Box<dyn Fn()>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = VeilIvars]
    struct VeilView;

    unsafe impl NSObjectProtocol for VeilView {}

    impl VeilView {
        #[unsafe(method_id(initWithFrame:))]
        unsafe fn __init_with_frame(
            this: Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self> {
            let this = this.set_ivars(VeilIvars {
                on_click: RefCell::new(None),
            });
            // SAFETY: NSView's designated initializer with plain ivars set.
            unsafe { msg_send![super(this), initWithFrame: frame_rect] }
        }

        #[unsafe(method(mouseDown:))]
        fn __mouse_down(&self, _event: &NSEvent) {
            if let Some(on_click) = self.ivars().on_click.borrow_mut().take() {
                on_click();
            }
        }
    }
);

impl VeilView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: overridden designated initializer, ivars fully set.
        unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::ZERO) }
    }
}

impl OverlayView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let view = unsafe { Self::init_with_frame(Self::alloc(mtm), CGRect::ZERO) };
        // Cancel the mode when the display configuration changes (monitor
        // plugged or unplugged, resolution change): the frozen frame and the
        // window frames no longer match the screens. The observer lives as
        // long as the view, i.e. for the whole process.
        let name = NSString::from_str("NSWorkspaceDidChangeScreenParametersNotification");
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: the selector names the screenDidChange: method defined on
        // OverlayView; the observer outlives every notification (process
        // lifetime), so no removal bookkeeping is needed.
        unsafe {
            center.addObserver_selector_name_object(
                &view,
                objc2::sel!(screenDidChange:),
                Some(&name),
                None,
            );
        }
        view
    }

    /// Build a veil window over one of the other displays: same shielding
    /// level, translucent black, a click cancels the mode.
    fn make_veil(mtm: MainThreadMarker, frame: CGRect, view: &OverlayView) -> Retained<NSWindow> {
        let window = unsafe {
            OverlayWindow::init_with_content_rect(
                OverlayWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: CGShieldingWindowLevel is a plain C function.
        let level = unsafe { CGShieldingWindowLevel() };
        window.setLevel(level as _);
        window.setOpaque(false);
        window.setBackgroundColor(Some(
            &NSColor::blackColor().colorWithAlphaComponent(DIM_ALPHA),
        ));
        window.setIgnoresMouseEvents(false);
        window.setHidesOnDeactivate(false);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        let veil_view = VeilView::new(mtm);
        // Cancel the whole mode when the veil is clicked.
        let target = view.retain();
        *veil_view.ivars().on_click.borrow_mut() =
            Some(Box::new(move || target.finish(FinishRequest::Cancelled)));
        window.setContentView(Some(&veil_view));
        // Upcast to the base window class for the veil list.
        Retained::from(&window)
    }

    /// Tear down the current veils and cover every screen except the
    /// current target's display (`ivars.screen_frame`). A veil click
    /// cancels the mode.
    fn rebuild_veils(&self) {
        let ivars = self.ivars();
        {
            let mut old_veils = ivars.veils.borrow_mut();
            for veil in old_veils.drain(..) {
                veil.orderOut(None);
            }
        }
        let mtm = MainThreadMarker::new().expect("overlay on main thread");
        let target_frame = ivars.screen_frame.get();
        let mut veils = Vec::new();
        for screen in NSScreen::screens(mtm).iter() {
            let frame = screen.frame();
            if frame.origin == target_frame.origin && frame.size == target_frame.size {
                continue;
            }
            let veil = Self::make_veil(mtm, frame, self);
            veil.orderFrontRegardless();
            veils.push(veil);
        }
        *ivars.veils.borrow_mut() = veils;
    }

    /// Cursor moved without a button: with the cursor over another
    /// display — switch the capture target to it (the overlay is the key
    /// window, so mouseMoved keeps arriving even over a veiled screen, and
    /// the global position comes from `NSEvent::mouseLocation`); on the
    /// current display — guides + loupe in idle mode. During a drag the
    /// method exits early: no switching mid-selection, the rectangle is
    /// clamped to the display edge.
    fn handle_mouse_moved(&self, event: &NSEvent) {
        let ivars = self.ivars();
        if !ivars.active.get() || ivars.state.get() != OverlayState::Idle {
            return;
        }
        let mouse = NSEvent::mouseLocation();
        let switch_to = {
            let map = ivars.surface_map.borrow();
            map.switch_candidate(mouse, ivars.target.get())
        };
        if let Some(index) = switch_to {
            self.switch_to_surface(index);
            return;
        }
        let point = crate::capture::geometry::clamp_point(
            self.local_point(event),
            CGRect::new(CGPoint::ZERO, ivars.screen_frame.get().size),
        );
        ivars.mouse.set(point);
        ivars.hud.update(point, None);
    }

    /// Point the flat ivars (frozen frame, display frames, scale) at the
    /// surface `index`, shown on `screen_frame` with `scale`.
    fn apply_target(&self, index: usize, screen_frame: CGRect, scale: f64) {
        let ivars = self.ivars();
        let surfaces = ivars.surfaces.borrow();
        let surface = &surfaces[index];
        let w_px = CGImage::width(Some(&surface.image));
        let h_px = CGImage::height(Some(&surface.image));
        *ivars.image.borrow_mut() = Some(surface.image.clone());
        ivars.image_size_px.set((w_px, h_px));
        ivars.display_frame_sckit.set(surface.display_frame_sckit);
        ivars.screen_frame.set(screen_frame);
        ivars.scale.set(scale);
    }

    /// The single target presentation path, shared by show() and
    /// switch_to_surface(): point the ivars at surface `index`, reset the
    /// HUD to the idle look, swap the frozen frame and the layer geometry
    /// without implicit animations, hand the HUD its new sharpness, border
    /// and sampler (`Hud::present`), rebuild the veils, move the window
    /// onto the paired screen. One presentation change has one home: this
    /// method (its twin paths once cost a 166-line rewrite to keep in
    /// sync).
    fn present_target(&self, index: usize, screen: ScreenPair) {
        let ivars = self.ivars();
        let (screen_frame, scale) = (screen.frame, screen.scale);
        self.apply_target(index, screen_frame, scale);
        ivars.target.set(index);

        // Reset the interaction state: idle, HUD down, no dimming. For
        // switch this restores the bare frame; for show it clears whatever
        // the previous session left behind (a drag interrupted by Esc
        // leaves the state machine in Drag and the dim strips on screen).
        ivars.state.set(OverlayState::Idle);
        ivars.hud.dim(None);
        ivars.hud.hide();

        // Layers follow the display: root/frame geometry and the frozen
        // bitmap. Without disabling actions Core Animation tweens the
        // swap, and the frame visibly cross-fades from the previous
        // session's frame.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        ivars.root_layer.setFrame(CGRect::new(CGPoint::ZERO, screen_frame.size));
        let frame_layer = &ivars.frame_layer;
        frame_layer.setContentsScale(scale);
        frame_layer.setFrame(CGRect::new(CGPoint::ZERO, screen_frame.size));
        if let Some(image) = ivars.image.borrow().as_ref() {
            let image: &CGImage = image;
            // SAFETY: CGImage is an ObjC object at runtime; CALayer.contents
            // expects it as AnyObject.
            let as_any: &AnyObject = unsafe { &*(std::ptr::from_ref(image).cast::<AnyObject>()) };
            // SAFETY: contents setter with a valid image object.
            unsafe { frame_layer.setContents(Some(as_any)) };
        }
        CATransaction::commit();

        // HUD sharpness, border path and the sampler follow the new display;
        // the border fade-in and idle pulse restart inside.
        let (w_px, h_px) = ivars.image_size_px.get();
        let source = ivars.image.borrow();
        let frame = source.as_ref().map(|image| (&**image, w_px, h_px));
        ivars.hud.present(scale, screen_frame.size, frame);
        drop(source);

        // Veils flip with the target: the new display is uncovered, the
        // rest is covered. A veil click cancels the mode.
        self.rebuild_veils();

        // The window sits on the target's screen and stays key.
        if let Some(window) = ivars.window.borrow().as_ref() {
            window.setFrame_display(screen_frame, false);
            window.makeKeyAndOrderFront(None);
        }
    }

    /// Point the frame layer at the T0 frame of the target surface (the
    /// display under the cursor) and show the overlay on its screen.
    /// Main thread only.
    fn show(&self, t0: T0Capture, on_done: Box<dyn FnOnce(OverlayOutcome)>) {
        let ivars = self.ivars();
        let mtm = MainThreadMarker::new().expect("overlay on main thread");
        if ivars.active.get() {
            crate::log!("overlay: capture request ignored, mode already active");
            return;
        }
        // The starting target must have a live screen behind it: without
        // one there is nothing to show the frozen frame on. The pairing is
        // session data: built once here, it serves every mouseMoved until
        // finish() (the screen list cannot change mid-session — a display
        // configuration change cancels the mode).
        let map = SurfaceMap::for_surfaces(&t0.surfaces, mtm);
        let Some(screen) = map.screen_of(t0.target) else {
            crate::log!("overlay: no live screen matches the capture target, dropping");
            return;
        };
        let screen_frame = screen.frame;
        *ivars.surfaces.borrow_mut() = t0.surfaces;
        *ivars.surface_map.borrow_mut() = map;
        *ivars.windows.borrow_mut() = t0.windows;
        *ivars.on_done.borrow_mut() = Some(on_done);
        ivars.finishing.set(false);
        // A stale receiver from a cancelled session must not eat the new
        // session's clicks (the mouse guards) or deliver its late answer.
        *ivars.resolving.borrow_mut() = None;
        ivars.active.set(true);

        // Attach the layer tree to the view once.
        if !ivars.layers_attached.get() {
            ivars.layers_attached.set(true);
            self.setWantsLayer(true);
            if let Some(view_layer) = self.layer() {
                let root = &ivars.root_layer;
                root.setFrame(CGRect::new(CGPoint::ZERO, screen_frame.size));
                // z-order: frozen frame, then the HUD chrome on top.
                root.addSublayer(&ivars.frame_layer);
                ivars.hud.attach(root, ivars.scale.get());
                view_layer.addSublayer(root);
            }
        }
        // The window joins all spaces for the whole session; the
        // presentation below moves it onto the target's screen.
        if let Some(window) = ivars.window.borrow().as_ref() {
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
        }
        // This accessory app is not active when launched from a shell or
        // when another app is frontmost: NSApp.activate() alone is
        // silently denied without user-interaction attribution, the
        // window never becomes key, and Esc goes nowhere until the
        // first click activates the app. The deprecated
        // activateIgnoringOtherApps is still honored for this exact
        // case; the extra makeKeyAndOrderFront on the next runloop tick
        // (after the presentation) locks the key status in after the
        // activation lands.
        let app =
            NSApplication::sharedApplication(MainThreadMarker::new().expect("overlay on main thread"));
        // SAFETY: deprecated NSObject method with a BOOL argument.
        let _: () = unsafe { msg_send![&app, activateIgnoringOtherApps: true] };
        app.activate();

        self.present_target(t0.target, screen);

        if let Some(window) = ivars.window.borrow().as_ref() {
            // SAFETY: NSObject delayed-perform scheduling; the selector is a
            // valid window method, the object argument is nil.
            let _: () = unsafe {
                msg_send![window,
                    performSelector: objc2::sel!(makeKeyAndOrderFront:),
                    withObject: None::<&AnyObject>,
                    afterDelay: 0.05
                ]
            };
        }

        let key_state = ivars
            .window
            .borrow()
            .as_ref()
            .map(|w| w.isKeyWindow())
            .unwrap_or(false);
        let (w_px, h_px) = ivars.image_size_px.get();
        crate::log!(
            "overlay: frozen {}x{} px on screen ({:.0},{:.0} {}x{}), app active: {}, key window: {}, surfaces: {}, windows: {}, veils: {}",
            w_px,
            h_px,
            screen_frame.origin.x,
            screen_frame.origin.y,
            screen_frame.size.width,
            screen_frame.size.height,
            app.isActive(),
            key_state,
            ivars.surfaces.borrow().len(),
            ivars.windows.borrow().len(),
            ivars.veils.borrow().len()
        );
    }

    /// Switch the capture target to the surface `index`: the shared
    /// presentation path does the work (idle reset, frame swap, sampler,
    /// veils, window move, border restart). Refuses a surface without a
    /// live screen behind it and re-entrant calls (mode not active or
    /// finishing, the current target). Returns whether the switch
    /// happened.
    fn switch_to_surface(&self, index: usize) -> bool {
        let ivars = self.ivars();
        if !ivars.active.get() || ivars.finishing.get() {
            return false;
        }
        if index == ivars.target.get() {
            return false;
        }
        let Some(screen) = ivars.surface_map.borrow().screen_of(index) else {
            crate::log!("overlay: no live screen for surface {index}, target kept");
            return false;
        };
        self.present_target(index, screen);
        crate::log!(
            "overlay: target switched to surface {index} on screen ({:.0},{:.0} {}x{}) scale={}",
            screen.frame.origin.x,
            screen.frame.origin.y,
            screen.frame.size.width,
            screen.frame.size.height,
            screen.scale
        );
        true
    }

    /// Cursor position in display-local points (bottom-left origin).
    fn local_point(&self, event: &NSEvent) -> CGPoint {
        let location = event.locationInWindow();
        // The borderless content view covers the whole window, so window
        // coordinates are display-local coordinates.
        CGPoint::new(location.x, location.y)
    }

    fn handle_mouse_down(&self, event: &NSEvent) {
        let ivars = self.ivars();
        if crate::capture::click_trace() {
            crate::log!(
                "mouse-down: active={}, resolving={}",
                ivars.active.get(),
                ivars.resolving.borrow().is_some()
            );
        }
        if !ivars.active.get() {
            return;
        }
        // A click resolution is in flight; the overlay is about to close.
        if ivars.resolving.borrow().is_some() {
            return;
        }
        let point = self.local_point(event);
        ivars.drag_start.set(point);
        ivars.mouse.set(point);
        ivars.state.set(OverlayState::Drag);
        // Drag: the border holds full red, no pulse.
        ivars.hud.stop_border_pulse();
        // Zero-size hole: full dimming until the drag opens a rectangle.
        ivars.hud.dim(Some(CGRect::new(point, objc2_core_foundation::CGSize::ZERO)));
        ivars.hud.update(point, Some((0.0, 0.0)));
    }

    fn handle_mouse_dragged(&self, event: &NSEvent) {
        let ivars = self.ivars();
        if ivars.state.get() != OverlayState::Drag {
            return;
        }
        // Cursor leaving the display clamps the rectangle to its bounds; the
        // drag loop keeps receiving events, so the rect follows the cursor.
        let point = crate::capture::geometry::clamp_point(
            self.local_point(event),
            CGRect::new(CGPoint::ZERO, ivars.screen_frame.get().size),
        );
        ivars.mouse.set(point);
        let start = ivars.drag_start.get();
        let rect = CGRect::new(
            CGPoint::new(start.x.min(point.x), start.y.min(point.y)),
            objc2_core_foundation::CGSize::new((point.x - start.x).abs(), (point.y - start.y).abs()),
        );
        ivars.hud.dim(Some(rect));
        let pixel_rect = crate::capture::geometry::selection_to_pixel_rect(
            rect,
            ivars.screen_frame.get().size,
            ivars.scale.get(),
        );
        // The HUD speaks logical pixels, like the loupe.
        let factor = ivars.scale.get().round().max(1.0);
        ivars.hud.update(
            point,
            Some((pixel_rect.size.width / factor, pixel_rect.size.height / factor)),
        );
    }

    /// Picks up a resolved click rect from the background resolver (if any)
    /// and finishes the mode with it. Called from the app tick on the main
    /// thread; until the answer arrives the overlay stays on screen.
    fn poll_resolved_click(&self) {
        let Some(rx) = self.ivars().resolving.borrow_mut().take() else {
            return;
        };
        match rx.try_recv() {
            Ok(rect) => self.finish(FinishRequest::Selected { pixel_rect: rect }),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.ivars().resolving.replace(Some(rx));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
        }
    }

    fn handle_mouse_up(&self, event: &NSEvent) {
        let ivars = self.ivars();
        // A click resolution is in flight; the next tick finishes the mode.
        if ivars.resolving.borrow().is_some() {
            return;
        }
        if ivars.state.get() != OverlayState::Drag {
            return;
        }
        let up = self.local_point(event);
        ivars.mouse.set(up);
        ivars.state.set(OverlayState::Idle);
        ivars.hud.dim(None);

        let start = ivars.drag_start.get();
        let moved = (up.x - start.x).abs() > CLICK_TOLERANCE_PT
            || (up.y - start.y).abs() > CLICK_TOLERANCE_PT;
        if !moved {
            // Click: capture the topmost T0 window under the cursor; a miss
            // (empty desktop area) takes the whole display. Window frames
            // are SCKit-global top-left points; the click converts from
            // display-local AppKit through the pixel mapping's Y flip at
            // scale 1 (points, not pixels), plus the display's SCKit origin.
            let screen_h = ivars.screen_frame.get().size.height;
            let sckit = ivars.display_frame_sckit.get();
            let local = crate::capture::geometry::display_point_to_pixel(up, screen_h, 1.0);
            let point_sckit = CGPoint::new(sckit.origin.x + local.x, sckit.origin.y + local.y);
            let t0_windows = ivars.windows.borrow().clone();
            crate::log!(
                "click: sckit point ({:.0},{:.0}), {} T0 windows",
                point_sckit.x,
                point_sckit.y,
                t0_windows.len()
            );
            // The list is only candidates: freshness and stacking order are
            // verified by rendering (see resolve_click_target). The whole
            // resolution runs off the main thread — it fetches SCK content
            // and renders windows, and SCK completions do not reach a main
            // thread blocked on the answer. The result lands via
            // poll_resolved_click on the app tick.
            match ivars.image.borrow().as_ref() {
                Some(frozen) => {
                    let frozen = frozen.clone();
                    let sckit = ivars.display_frame_sckit.get();
                    let (tx, rx) = std::sync::mpsc::channel();
                    ivars.resolving.replace(Some(rx));
                    std::thread::spawn(move || {
                        let rect = crate::capture::resolve_click_target(
                            &t0_windows,
                            point_sckit,
                            sckit,
                            &frozen,
                        );
                        let _ = tx.send(rect);
                    });
                    return;
                }
                None => {
                    let (w_px, h_px) = ivars.image_size_px.get();
                    let rect = CGRect::new(
                        CGPoint::ZERO,
                        objc2_core_foundation::CGSize::new(w_px as f64, h_px as f64),
                    );
                    self.finish(FinishRequest::Selected { pixel_rect: rect });
                }
            }
            return;
        }

        // Drag: normalize the selection and crop in pixel space.
        let screen_frame = ivars.screen_frame.get();
        let scale = ivars.scale.get();
        let rect = crate::capture::geometry::intersect(
            CGRect::new(
                CGPoint::new(start.x.min(up.x), start.y.min(up.y)),
                objc2_core_foundation::CGSize::new((up.x - start.x).abs(), (up.y - start.y).abs()),
            ),
            CGRect::new(CGPoint::ZERO, screen_frame.size),
        );
        let pixel_rect = crate::capture::geometry::selection_to_pixel_rect(
            rect,
            screen_frame.size,
            scale,
        );
        self.finish(FinishRequest::Selected { pixel_rect });
    }

    /// Agent: finish as a selection of the rect in captured-frame pixels
    /// (top-left origin). Clamps into the frame; an empty result is an
    /// error rather than a silent no-op.
    fn agent_select(&self, x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
        let ivars = self.ivars();
        if !ivars.active.get() || ivars.finishing.get() {
            return Err("overlay: the capture mode is not active".into());
        }
        let (image_w, image_h) = ivars.image_size_px.get();
        let rect = crate::capture::geometry::intersect(
            CGRect::new(CGPoint::new(x, y), CGSize::new(w, h)),
            CGRect::new(
                CGPoint::ZERO,
                CGSize::new(image_w as f64, image_h as f64),
            ),
        );
        if rect == CGRect::ZERO {
            return Err(format!(
                "overlay: empty selection rect ({x},{y},{w},{h}) in a {image_w}x{image_h} frame"
            ));
        }
        self.finish(FinishRequest::Selected { pixel_rect: rect });
        Ok(())
    }

    /// Complete the mode: hide everything and hand the outcome to the owner.
    fn finish(&self, request: FinishRequest) {        let ivars = self.ivars();
        if !ivars.active.get() || ivars.finishing.get() {
            return;
        }
        ivars.finishing.set(true);
        ivars.active.set(false);
        // The mode is over; a click still resolving is cancelled with it —
        // drop the receiver so its late answer cannot finish anything.
        *ivars.resolving.borrow_mut() = None;

        for veil in ivars.veils.borrow().iter() {
            veil.orderOut(None);
        }
        ivars.veils.borrow_mut().clear();
        // The T0 surface set belongs to the finished mode; the selected
        // frame's reference travels inside the outcome.
        *ivars.surfaces.borrow_mut() = Vec::new();
        *ivars.surface_map.borrow_mut() = SurfaceMap::default();
        let image = ivars.image.borrow_mut().take();
        let scale = ivars.scale.get();
        // The selection carries the frame to the output stage; a missing
        // image (should never happen) downgrades to a cancelled mode.
        let outcome = match request {
            FinishRequest::Selected { pixel_rect } => match image {
                Some(image) => OverlayOutcome::Selected {
                    pixel_rect,
                    image,
                    scale,
                },
                None => {
                    crate::log!("overlay: selection finished without a frame, dropping");
                    OverlayOutcome::Cancelled
                }
            },
            FinishRequest::Cancelled => OverlayOutcome::Cancelled,
        };
        ivars.hud.drop_sampler();
        ivars.hud.hide();

        // Get out of the user's way first: the window drops the moment the
        // mode ends, and only then the output stage runs on the exposed
        // desktop (crop, shutter, clipboard, file). Nothing of the capture
        // pipeline lingers on screen.
        ivars.hud.stop_border_pulse();
        if let Some(window) = ivars.window.borrow().as_ref() {
            window.orderOut(None);
        }

        if let Some(on_done) = ivars.on_done.borrow_mut().take() {
            on_done(outcome);
        }
    }
}

/// The reusable overlay mode: one window + view + per-show veil windows.
pub struct Overlay {
    // Kept alive together with the view; both live for the process.
    #[allow(dead_code)]
    window: Retained<OverlayWindow>,
    view: Retained<OverlayView>,
}

impl Overlay {
    /// Create the overlay machinery once at startup. Cheap to keep around.
    pub fn new(mtm: MainThreadMarker) -> Self {
        let window = unsafe {
            OverlayWindow::init_with_content_rect(
                OverlayWindow::alloc(mtm),
                CGRect::ZERO,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: CGShieldingWindowLevel is a plain C function.
        let level = unsafe { CGShieldingWindowLevel() };
        window.setLevel(level as _);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setIgnoresMouseEvents(false);
        window.setAcceptsMouseMovedEvents(true);
        window.setHidesOnDeactivate(false);
        let view = OverlayView::new(mtm);
        // Cross-link the window and its content view.
        view.ivars().window.replace(Some(window.clone()));
        window.setContentView(Some(&view));
        window.setInitialFirstResponder(Some(&view));
        Overlay { window, view }
    }

    pub fn is_active(&self) -> bool {
        self.view.ivars().active.get()
    }

    /// Enter the frozen-frame mode on the display under the cursor.
    /// Main thread only.
    pub fn show(&self, t0: T0Capture, on_done: Box<dyn FnOnce(OverlayOutcome)>) {
        self.view.show(t0, on_done);
    }

    /// Cancel the active mode without a result (e.g. display config change).
    pub fn cancel(&self) {
        self.view.finish(FinishRequest::Cancelled);
    }

    /// Agent: finish the active mode as a selection of the given rect in
    /// the captured frame's pixel space (top-left origin) — the same
    /// outcome path as a user drag. An error names the reason: the mode
    /// is not active, or the rect is empty after clamping into the frame.
    pub fn agent_select(&self, x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
        self.view.agent_select(x, y, w, h)
    }

    /// Agent: cancel the active mode; `false` when nothing was active.
    pub fn agent_cancel(&self) -> bool {
        if !self.is_active() {
            return false;
        }
        self.view.finish(FinishRequest::Cancelled);
        true
    }

    /// Delivers a resolved click result from the background resolver, if
    /// one is ready. Called from the app tick. Main thread only.
    pub fn poll_resolved_click(&self) {
        self.view.poll_resolved_click();
    }
}
