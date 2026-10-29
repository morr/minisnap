//! Preferences window: two hotkey recorder rows, built in code (no xib).
//!
//! The window is created once and kept by the app controller; reopening
//! from the menu just brings the existing one to the front.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSLayoutAttribute,
    NSLayoutConstraintOrientation, NSStackView, NSStackViewDistribution, NSTextField,
    NSUserInterfaceLayoutOrientation, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSString;

/// Sender tags telling the recorder which hotkey a row edits.
pub const ROW_CAPTURE: isize = 0;
pub const ROW_OPEN_LATEST: isize = 1;

/// Fixed window size: two rows fit with comfortable margins.
const CONTENT_SIZE: CGSize = CGSize::new(380.0, 116.0);
const CONTENT_MARGIN: f64 = 16.0;
const ROW_SPACING: f64 = 14.0;

/// Owned pieces of the preferences window that the controller updates.
pub struct PrefsWindow {
    pub window: Retained<NSWindow>,
    /// "Capture Frozen Area" combo button (tag `ROW_CAPTURE`).
    pub capture_button: Retained<NSButton>,
    /// "Open Latest Image" combo button (tag `ROW_OPEN_LATEST`).
    pub open_button: Retained<NSButton>,
}

/// Non-editable text label for a row title.
fn row_label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    NSTextField::labelWithString(&NSString::from_str(text), mtm)
}

/// The button that displays (and later records) a hotkey combination.
fn combo_button(mtm: MainThreadMarker, combo: &str, tag: isize) -> Retained<NSButton> {
    // SAFETY: class convenience constructor, no external invariants.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(combo),
            None,
            None,
            mtm,
        )
    };
    button.setTag(tag);
    button
}

/// One settings row: the action title at the leading edge, the combo button
/// at the trailing edge (the label has the low hugging priority, so it
/// absorbs the extra width).
fn row(
    mtm: MainThreadMarker,
    title: &str,
    combo: &str,
    tag: isize,
) -> (Retained<NSStackView>, Retained<NSButton>) {
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setAlignment(NSLayoutAttribute::CenterY);
    row.setDistribution(NSStackViewDistribution::Fill);
    row.setSpacing(12.0);

    let label = row_label(mtm, title);
    label.setContentHuggingPriority_forOrientation(249.0, NSLayoutConstraintOrientation::Horizontal);
    let button = combo_button(mtm, combo, tag);
    row.addArrangedSubview(&label);
    row.addArrangedSubview(&button);
    (row, button)
}

/// Build the preferences window with the given rendered combinations.
/// Buttons start without a target; the controller attaches its action.
pub fn build(mtm: MainThreadMarker, capture_combo: &str, open_combo: &str) -> PrefsWindow {
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            CGRect::new(CGPoint::ZERO, CONTENT_SIZE),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setTitle(&NSString::from_str("Preferences"));
    // The window lives in the controller for the whole process; closing it
    // only hides it, reopening brings the same window back (spec: no second
    // window is created).
    // SAFETY: plain setter on the freshly created window.
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.center();

    let content = window.contentView().expect("content view");
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    // Rows stretch to the full width, so their trailing combo buttons line up.
    stack.setAlignment(NSLayoutAttribute::Width);
    stack.setDistribution(NSStackViewDistribution::GravityAreas);
    stack.setSpacing(ROW_SPACING);
    let size = content.bounds().size;
    stack.setFrame(CGRect::new(
        CGPoint::new(CONTENT_MARGIN, CONTENT_MARGIN),
        CGSize::new(size.width - 2.0 * CONTENT_MARGIN, size.height - 2.0 * CONTENT_MARGIN),
    ));
    stack.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
    content.addSubview(&stack);

    let (capture_row, capture_button) = row(mtm, "Capture Frozen Area", capture_combo, ROW_CAPTURE);
    let (open_row, open_button) = row(mtm, "Open Latest Image", open_combo, ROW_OPEN_LATEST);
    stack.addArrangedSubview(&capture_row);
    stack.addArrangedSubview(&open_row);

    PrefsWindow {
        window,
        capture_button,
        open_button,
    }
}
