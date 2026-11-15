//! Preferences window: four hotkey recorder rows plus the upload section
//! (a provider checkbox with its fields), built in code (no xib).
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

use crate::config::ArtifactsProviderConfig;

/// Sender tags telling the recorder which hotkey a row edits.
pub const ROW_CAPTURE: isize = 0;
pub const ROW_CAPTURE_EDIT: isize = 1;
pub const ROW_OPEN_LATEST: isize = 2;
pub const ROW_CAPTURE_UPLOAD: isize = 3;

/// Fixed window width; the height follows the visible rows.
const CONTENT_WIDTH: f64 = 380.0;
const CONTENT_MARGIN: f64 = 16.0;
const ROW_SPACING: f64 = 14.0;
/// Window-side height budget of one row. Rows keep their intrinsic size;
/// the budget pads each so nothing clips (the original three-row window
/// was built on the same share).
const ROW_HEIGHT: f64 = 29.0;

/// Which config value one field row edits.
#[derive(Clone, Copy, PartialEq)]
enum FieldKey {
    Token,
    Project,
    Proxy,
}

/// One field row of a provider section: what it shows, what it edits.
struct UploadField {
    key: FieldKey,
    label: &'static str,
    placeholder: &'static str,
}

/// The upload-provider registry: what the upload section shows. A future
/// hosting provider adds an entry here, its config section and a
/// `build_backend` branch; the section rows are built from the entry.
struct UploadProvider {
    id: &'static str,
    title: &'static str,
    fields: &'static [UploadField],
}

const UPLOAD_PROVIDERS: &[UploadProvider] = &[UploadProvider {
    id: "artifacts",
    title: "Artifacts",
    fields: &[
        UploadField {
            key: FieldKey::Token,
            label: "Access token",
            placeholder: "paste your artifacts token",
        },
        UploadField {
            key: FieldKey::Project,
            label: "Project",
            placeholder: "project namespace",
        },
        UploadField {
            key: FieldKey::Proxy,
            label: "Proxy",
            placeholder: "http://127.0.0.1:8118",
        },
    ],
}];

/// Owned pieces of the preferences window that the controller updates.
pub struct PrefsWindow {
    pub window: Retained<NSWindow>,
    /// "Capture Frozen Area" combo button (tag `ROW_CAPTURE`).
    pub capture_button: Retained<NSButton>,
    /// "Capture and Edit" combo button (tag `ROW_CAPTURE_EDIT`).
    pub capture_edit_button: Retained<NSButton>,
    /// "Capture and Upload" combo button (tag `ROW_CAPTURE_UPLOAD`).
    pub capture_upload_button: Retained<NSButton>,
    /// "Open Latest Image" combo button (tag `ROW_OPEN_LATEST`).
    pub open_button: Retained<NSButton>,
    /// The upload section (one row per registry provider).
    pub upload: UploadSectionUi,
    /// The root vertical stack; the section rows are arranged here.
    stack: Retained<NSStackView>,
}

/// The upload section controls: the provider checkbox plus its fields,
/// visible only while the checkbox is on.
pub struct UploadSectionUi {
    /// Provider id the section is built for.
    pub provider_id: &'static str,
    /// The enable checkbox (switch) of the provider row.
    pub checkbox: Retained<NSButton>,
    pub token_field: Retained<NSTextField>,
    pub project_field: Retained<NSTextField>,
    pub proxy_field: Retained<NSTextField>,
    /// The field rows; arranged subviews of the stack only while visible.
    field_rows: Vec<Retained<NSStackView>>,
}

/// Window-side content height for the given number of visible field rows.
fn content_height(field_rows: usize) -> f64 {
    // Four recorder rows + one provider checkbox row.
    let rows = 5.0 + field_rows as f64;
    CONTENT_MARGIN * 2.0 + rows * ROW_HEIGHT + (rows - 1.0) * ROW_SPACING
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

/// A labelled text field row; the field takes the spare width, not the label.
fn field_row(
    mtm: MainThreadMarker,
    label: &str,
    placeholder: &str,
    value: &str,
) -> (Retained<NSStackView>, Retained<NSTextField>) {
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setAlignment(NSLayoutAttribute::CenterY);
    row.setDistribution(NSStackViewDistribution::Fill);
    row.setSpacing(8.0);

    let label = row_label(mtm, label);
    let field = NSTextField::textFieldWithString(&NSString::from_str(value), mtm);
    field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
    field.setContentHuggingPriority_forOrientation(
        100.0,
        NSLayoutConstraintOrientation::Horizontal,
    );
    row.addArrangedSubview(&label);
    row.addArrangedSubview(&field);
    (row, field)
}

impl PrefsWindow {
    /// Reveal or hide the provider field rows and refit the window height.
    pub fn set_provider_fields_visible(&self, visible: bool) {
        for field_row in &self.upload.field_rows {
            if visible {
                // SAFETY: plain view accessor on the section's own rows.
                if unsafe { field_row.superview() }.is_none() {
                    // Field rows come after the checkbox row; because they
                    // are the last section rows, appending keeps the order.
                    self.stack.addArrangedSubview(field_row);
                }
            } else {
                self.stack.removeArrangedSubview(field_row);
                // SAFETY: plain view surgery on the section's own rows.
                unsafe { field_row.removeFromSuperview() };
            }
        }
        let count = usize::from(visible) * self.upload.field_rows.len();
        self.window
            .setContentSize(CGSize::new(CONTENT_WIDTH, content_height(count)));
    }
}

/// Build the preferences window with the given rendered combinations and
/// the upload section values. Buttons start without a target; the
/// controller attaches its action (and the field delegates).
/// The window title carries the checkout label of the running instance.
pub fn build(
    mtm: MainThreadMarker,
    capture_combo: &str,
    capture_edit_combo: &str,
    capture_upload_combo: &str,
    open_combo: &str,
    upload_provider: &ArtifactsProviderConfig,
    checkout_label: &str,
) -> PrefsWindow {
    let field_rows_visible = usize::from(upload_provider.enabled);
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            CGRect::new(
                CGPoint::ZERO,
                CGSize::new(CONTENT_WIDTH, content_height(field_rows_visible)),
            ),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setTitle(&NSString::from_str(&format!(
        "Preferences — {checkout_label}"
    )));
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
    let (capture_edit_row, capture_edit_button) =
        row(mtm, "Capture and Edit", capture_edit_combo, ROW_CAPTURE_EDIT);
    let (capture_upload_row, capture_upload_button) =
        row(mtm, "Capture and Upload", capture_upload_combo, ROW_CAPTURE_UPLOAD);
    let (open_row, open_button) = row(mtm, "Open Latest Image", open_combo, ROW_OPEN_LATEST);
    stack.addArrangedSubview(&capture_row);
    stack.addArrangedSubview(&capture_edit_row);
    stack.addArrangedSubview(&capture_upload_row);
    stack.addArrangedSubview(&open_row);

    // The upload section: one checkbox row per registry provider, its
    // field rows under it (arranged only while the checkbox is on).
    let provider = &UPLOAD_PROVIDERS[0];
    // SAFETY: class convenience constructor, no external invariants.
    let checkbox = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(provider.title),
            None,
            None,
            mtm,
        )
    };
    checkbox.setState(if upload_provider.enabled {
        objc2_app_kit::NSControlStateValueOn
    } else {
        objc2_app_kit::NSControlStateValueOff
    });
    stack.addArrangedSubview(&checkbox);

    let mut field_rows = Vec::new();
    let mut token_field = None;
    let mut project_field = None;
    let mut proxy_field = None;
    for field_spec in provider.fields {
        let value = match field_spec.key {
            FieldKey::Token => upload_provider.token.as_str(),
            FieldKey::Project => upload_provider.project.as_str(),
            FieldKey::Proxy => upload_provider.proxy.as_str(),
        };
        let (row_stack, field) = field_row(mtm, field_spec.label, field_spec.placeholder, value);
        match field_spec.key {
            FieldKey::Token => token_field = Some(field.clone()),
            FieldKey::Project => project_field = Some(field.clone()),
            FieldKey::Proxy => proxy_field = Some(field.clone()),
        }
        if upload_provider.enabled {
            stack.addArrangedSubview(&row_stack);
        }
        field_rows.push(row_stack);
    }
    let upload = UploadSectionUi {
        provider_id: provider.id,
        checkbox,
        token_field: token_field.expect("registry must define the token field"),
        project_field: project_field.expect("registry must define the project field"),
        proxy_field: proxy_field.expect("registry must define the proxy field"),
        field_rows,
    };

    PrefsWindow {
        window,
        capture_button,
        capture_edit_button,
        capture_upload_button,
        open_button,
        upload,
        stack,
    }
}
