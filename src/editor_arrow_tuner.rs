//! The arrow debug tuner: a translucent panel of sliders over the canvas'
//! top-left corner, editing the live arrow params (`arrow::ARROW_PARAMS`)
//! with instant redraw. Built only under `MINISNAP_ARROW_DEBUG` — a
//! tuning instrument, not shipped UI. The panel is its own objc2 view
//! owner: it keeps its slider rows, every control targets the panel
//! itself, and the user's intents leave as ready `ToolbarEvent::ArrowParam*`
//! through the same on-event callback the toolbar reports through — the
//! window applies them. Every view gets an explicit frame in rows of
//! fixed height: a stack-view panel sized from `fittingSize` proved
//! fragile and dropped its bottom row of buttons.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{define_class, extern_methods, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSBox, NSBoxType, NSButton,
    NSColor, NSFocusRingType, NSFont, NSFontWeightRegular, NSPasteboard, NSPasteboardTypeString,
    NSSlider, NSTextAlignment, NSTextField, NSTitlePosition,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSString;

use crate::editor::arrow::{self, ArrowParams};
use crate::editor_toolbar::{icon_button, symbol_image, ToolbarEvent};

/// The window-side sink for tuner events (the same kind of callback the
/// toolbar reports through).
type OnEvent = Box<dyn Fn(ToolbarEvent)>;

/// One knob of the arrow tuner: the `ArrowParams` field it reads and
/// writes, the slider's range, and the row label. Slider tags index this
/// table.
pub(crate) struct ArrowParamSpec {
    pub(crate) name: &'static str,
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) get: fn(ArrowParams) -> f64,
    pub(crate) set: fn(&mut ArrowParams, f64),
}

/// The tuner rows, top to bottom — the main constants of the arrow
/// construction (`ArrowParams`). `half_head_w` stays above the body
/// half-width (1.1 stroke widths), so even a dragged-low slider cannot
/// invert the wing into the body.
pub(crate) const ARROW_PARAM_SPECS: &[ArrowParamSpec] = &[
    ArrowParamSpec {
        name: "head_len_w",
        min: 2.0,
        max: 8.0,
        get: |p| p.head_len_w,
        set: |p, v| p.head_len_w = v,
    },
    ArrowParamSpec {
        name: "half_head_w",
        min: 1.2,
        max: 4.0,
        get: |p| p.half_head_w,
        set: |p, v| p.half_head_w = v,
    },
    ArrowParamSpec {
        name: "notch_at",
        min: 0.4,
        max: 0.95,
        get: |p| p.notch_at,
        set: |p, v| p.notch_at = v,
    },
    ArrowParamSpec {
        name: "notch_bow",
        min: 0.0,
        max: 1.0,
        get: |p| p.notch_bow,
        set: |p, v| p.notch_bow = v,
    },
    ArrowParamSpec {
        name: "outer_bow",
        min: 0.0,
        max: 1.0,
        get: |p| p.outer_bow,
        set: |p, v| p.outer_bow = v,
    },
    ArrowParamSpec {
        name: "notch_round_w",
        min: 0.0,
        max: 0.8,
        get: |p| p.notch_round_w,
        set: |p, v| p.notch_round_w = v,
    },
    ArrowParamSpec {
        name: "light_toward_white",
        min: 0.0,
        max: 0.6,
        get: |p| p.light_toward_white,
        set: |p, v| p.light_toward_white = v,
    },
    ArrowParamSpec {
        name: "dark_shade",
        min: 0.3,
        max: 1.0,
        get: |p| p.dark_shade,
        set: |p, v| p.dark_shade = v,
    },
    ArrowParamSpec {
        name: "shadow_dx",
        min: -4.0,
        max: 4.0,
        get: |p| p.shadow_dx,
        set: |p, v| p.shadow_dx = v,
    },
    ArrowParamSpec {
        name: "shadow_dy",
        min: -4.0,
        max: 4.0,
        get: |p| p.shadow_dy,
        set: |p, v| p.shadow_dy = v,
    },
    ArrowParamSpec {
        name: "shadow_blur",
        min: 0.0,
        max: 8.0,
        get: |p| p.shadow_blur,
        set: |p, v| p.shadow_blur = v,
    },
    ArrowParamSpec {
        name: "shadow_alpha",
        min: 0.0,
        max: 1.0,
        get: |p| p.shadow_alpha,
        set: |p, v| p.shadow_alpha = v,
    },
];

/// The text the tuner's `prompt` button puts on the clipboard: the tuned
/// values plus a ready-to-paste instruction for the coding agent (ZCode)
/// to bake them in as the new defaults, backing the old ones up to /tmp.
fn arrow_params_prompt(params: ArrowParams) -> String {
    let mut out = String::from(
        "Примени настроенные в debug-панели значения параметров стрелки \
         как новые дефолты в minisnap.\n\n",
    );
    out.push_str("Значения (все 12 полей ArrowParams):\n");
    for spec in ARROW_PARAM_SPECS {
        out.push_str(&format!("{} = {:.2}\n", spec.name, (spec.get)(params)));
    }
    out.push_str("\nШаги:\n");
    out.push_str(
        "1. Забекапь прежние значения из ArrowParams::DEFAULT в \
         src/editor/arrow.rs в файл /tmp/minisnap-arrow-defaults-backup-<время>.txt — \
         те же 12 строк вида `имя = значение`.\n",
    );
    out.push_str(
        "2. Подставь значения из списка выше в ArrowParams::DEFAULT. Кроме \
         DEFAULT ничего в коде не меняй: живые параметры — рантайм-крутилки \
         тюнера, новыми дефолтами становятся только значения DEFAULT.\n",
    );
    out.push_str(
        "3. Прогони cargo test: стрелочные тесты должны остаться зелёными. \
         Если пиксельный тест стрелки падает из-за изменившегося силуэта — \
         сдвинь его контрольные точки осмысленно, не ослабляй пороги.\n",
    );
    out.push_str(
        "4. Закоммить по скиллу git: только src/editor/arrow.rs; сообщение \
         вроде `set the arrow defaults to the tuned look`.\n",
    );
    out
}

/// Ivars of the tuner panel.
pub(crate) struct ArrowTunerIvars {
    /// The tuner rows: each arrow-param slider with its value field
    /// (position matches ARROW_PARAM_SPECS).
    rows: RefCell<Vec<(Retained<NSSlider>, Retained<NSTextField>)>>,
    /// The window-side sink for everything the user does to the panel.
    on_event: RefCell<Option<OnEvent>>,
}

define_class!(
    /// The arrow debug tuner panel: a translucent box the window drops
    /// over the canvas' top-left corner. It owns its slider rows and
    /// every control targets it — no editor state lives here, the user's
    /// intents leave as `ToolbarEvent::ArrowParam*` through the callback.
    #[unsafe(super(NSBox))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ArrowTunerIvars]
    pub(crate) struct ArrowTunerPanel;

    unsafe impl NSObjectProtocol for ArrowTunerPanel {}

    impl ArrowTunerPanel {
        #[unsafe(method_id(initWithFrame:))]
        unsafe fn __init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self> {
            let this = this.set_ivars(ArrowTunerIvars {
                rows: RefCell::new(Vec::new()),
                on_event: RefCell::new(None),
            });
            // SAFETY: NSBox's designated initializer with plain ivars set.
            unsafe { msg_send![super(this), initWithFrame: frame_rect] }
        }

        #[unsafe(method(arrowParamChanged:))]
        fn __arrow_param_changed(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is a tuner slider; `tag` and `doubleValue`
            // are its accessors.
            let tag: isize = unsafe { msg_send![sender, tag] };
            let value: f64 = unsafe { msg_send![sender, doubleValue] };
            if let Some((_, field)) = self.ivars().rows.borrow().get(tag as usize) {
                field.setStringValue(&NSString::from_str(&format!("{value:.2}")));
            }
            self.emit(ToolbarEvent::ArrowParamChanged {
                index: tag as usize,
                value,
            });
        }

        #[unsafe(method(arrowTunerReset:))]
        fn __arrow_tuner_reset(&self, _sender: Option<&AnyObject>) {
            let defaults = arrow::ArrowParams::DEFAULT;
            let rows = self.ivars().rows.borrow();
            for (spec, (slider, field)) in ARROW_PARAM_SPECS.iter().zip(rows.iter()) {
                let value = (spec.get)(defaults);
                slider.setDoubleValue(value);
                field.setStringValue(&NSString::from_str(&format!("{value:.2}")));
            }
            self.emit(ToolbarEvent::ArrowParamsReset);
        }

        #[unsafe(method(arrowParamReset:))]
        fn __arrow_param_reset(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: the sender is a per-row reset button; `tag` is its
            // accessor.
            let tag: isize = unsafe { msg_send![sender, tag] };
            // Only this knob returns to its shipped default; the rest of
            // the tuning session stays.
            let defaults = arrow::ArrowParams::DEFAULT;
            let Some(spec) = ARROW_PARAM_SPECS.get(tag as usize) else {
                return;
            };
            let value = (spec.get)(defaults);
            if let Some((slider, field)) = self.ivars().rows.borrow().get(tag as usize) {
                slider.setDoubleValue(value);
                field.setStringValue(&NSString::from_str(&format!("{value:.2}")));
            }
            self.emit(ToolbarEvent::ArrowParamReset(tag as usize));
        }

        #[unsafe(method(arrowPromptCopy:))]
        fn __arrow_prompt_copy(&self, _sender: Option<&AnyObject>) {
            // The tuner's whole point: carry the tuned values out of the
            // app and into the coding agent that bakes them in as the new
            // defaults. Pure clipboard work — the document learns nothing.
            let text = NSString::from_str(&arrow_params_prompt(arrow::arrow_params()));
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            // SAFETY: NSPasteboardTypeString is a plain extern NSString
            // constant.
            let ok = pasteboard.setString_forType(&text, unsafe { NSPasteboardTypeString });
            if !ok {
                eprintln!("editor: the pasteboard rejected the tuner prompt");
            }
        }
    }
);

impl ArrowTunerPanel {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> Retained<Self>;
    );

    fn emit(&self, event: ToolbarEvent) {
        if let Some(on_event) = self.ivars().on_event.borrow().as_ref() {
            on_event(event);
        }
    }
}

/// Build the tuner panel: the box, the spec rows and the two action
/// buttons. The panel is returned unplaced — the window frames it over
/// the canvas' top-left corner and adds it to the content view. Every
/// control targets the panel itself; the user's intents leave through
/// `on_event` as `ToolbarEvent::ArrowParam*`.
pub(crate) fn arrow_tuner_panel(
    mtm: MainThreadMarker,
    on_event: impl Fn(ToolbarEvent) + 'static,
) -> Retained<ArrowTunerPanel> {
    // Row geometry, in points: label | slider | this-row reset | value.
    const TOP: f64 = 10.0;
    const BOTTOM: f64 = 10.0;
    const INSET: f64 = 12.0;
    const ROW_H: f64 = 20.0;
    const ROW_GAP: f64 = 2.0;
    const LABEL_W: f64 = 104.0;
    const SLIDER_X: f64 = INSET + LABEL_W + 4.0;
    const SLIDER_W: f64 = 88.0;
    const RESET_X: f64 = SLIDER_X + SLIDER_W + 4.0;
    const RESET_W: f64 = 22.0;
    const VALUE_X: f64 = RESET_X + RESET_W + 4.0;
    const VALUE_W: f64 = 44.0;
    const ACTIONS_GAP: f64 = 8.0;
    const ACTION_H: f64 = 24.0;
    const ACTION_W: f64 = 64.0;
    const W: f64 = VALUE_X + VALUE_W + INSET;
    let n = ARROW_PARAM_SPECS.len() as f64;
    let h = TOP + n * ROW_H + (n - 1.0) * ROW_GAP + ACTIONS_GAP + ACTION_H + BOTTOM;

    // SAFETY: designated initializer with plain ivars set.
    let panel =
        unsafe { ArrowTunerPanel::init_with_frame(ArrowTunerPanel::alloc(mtm), CGRect::ZERO) };
    *panel.ivars().on_event.borrow_mut() = Some(Box::new(on_event));
    // A classic drawn box, not a layer-backed view: on this macOS every
    // control is SwiftUI-hosted, and inline-hosted controls (NSButton)
    // stop compositing inside a view with a hand-configured layer — they
    // went invisible. NSBox paints its fill the old way and leaves the
    // control hosting alone.
    // NSBoxCustom: the Primary style draws a system material and ignores
    // the fill color; Custom is the type where fillColor actually paints.
    panel.setBoxType(NSBoxType::Custom);
    panel.setTitlePosition(NSTitlePosition::NoTitle);
    panel.setBorderWidth(0.0);
    panel.setFillColor(&NSColor::blackColor().colorWithAlphaComponent(0.7));
    panel.setCornerRadius(10.0);
    panel.setContentViewMargins(objc2_foundation::NSSize::new(0.0, 0.0));
    panel.setFrameSize(CGSize::new(W, h));
    let content_area = panel.contentView().expect("box content view");
    // The panel's buttons draw in the dark look whatever the system
    // appearance is: on the dark fill their titles read white and the
    // template icons stay light.
    // SAFETY: NSAppearanceNameDarkAqua is a plain extern NSString
    // constant.
    let dark_appearance = unsafe { NSAppearance::appearanceNamed(NSAppearanceNameDarkAqua) };
    let font = NSFont::monospacedSystemFontOfSize_weight(
        10.0,
        // SAFETY: NSFontWeightRegular is a plain extern CGFloat constant.
        unsafe { NSFontWeightRegular },
    );
    let params = arrow::arrow_params();
    let mut rows = Vec::new();
    for (tag, spec) in ARROW_PARAM_SPECS.iter().enumerate() {
        let y = h - TOP - ROW_H - tag as f64 * (ROW_H + ROW_GAP);
        let label = NSTextField::labelWithString(&NSString::from_str(spec.name), mtm);
        label.setFont(Some(&font));
        label.setTextColor(Some(&NSColor::whiteColor()));
        label.setFrame(CGRect::new(
            CGPoint::new(INSET, y + 3.0),
            CGSize::new(LABEL_W, 14.0),
        ));
        let value = (spec.get)(params);
        // SAFETY: class convenience constructor for the slider.
        let slider = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                value,
                spec.min,
                spec.max,
                Some(&*panel),
                Some(sel!(arrowParamChanged:)),
                mtm,
            )
        };
        slider.setTag(tag as isize);
        // Sliders report on every tick of a drag, not only on release.
        slider.setContinuous(true);
        slider.setFocusRingType(NSFocusRingType::None);
        slider.setRefusesFirstResponder(true);
        slider.setFrame(CGRect::new(
            CGPoint::new(SLIDER_X, y + 1.0),
            CGSize::new(SLIDER_W, 18.0),
        ));
        // Reset for this one knob, tagged with the row.
        let reset_name = format!("reset {}", spec.name);
        let reset = icon_button(
            mtm,
            symbol_image("arrow.counterclockwise", &reset_name),
            &reset_name,
            tag as isize,
        );
        // SAFETY: target is this panel, alive while the editor window
        // lives.
        unsafe {
            reset.setTarget(Some(&*panel));
            reset.setAction(Some(sel!(arrowParamReset:)));
        }
        reset.setToolTip(Some(&NSString::from_str(&format!(
            "reset {} to its default",
            spec.name
        ))));
        if let Some(appearance) = &dark_appearance {
            reset.setAppearance(Some(appearance));
        }
        reset.setFrame(CGRect::new(
            CGPoint::new(RESET_X, y + 1.0),
            CGSize::new(RESET_W, 18.0),
        ));
        let field =
            NSTextField::labelWithString(&NSString::from_str(&format!("{value:.2}")), mtm);
        field.setFont(Some(&font));
        field.setTextColor(Some(&NSColor::whiteColor()));
        field.setAlignment(NSTextAlignment::Right);
        field.setFrame(CGRect::new(
            CGPoint::new(VALUE_X, y + 3.0),
            CGSize::new(VALUE_W, 14.0),
        ));
        content_area.addSubview(&label);
        content_area.addSubview(&slider);
        content_area.addSubview(&reset);
        content_area.addSubview(&field);
        rows.push((slider, field));
    }
    // Bottom actions: reset everything to the shipped defaults, or copy
    // the tuned values as a prompt that makes the coding agent bake them
    // in as the new defaults (the old ones backed up to /tmp).
    let action_button = |title: &str, action: objc2::runtime::Sel, x: f64| {
        // SAFETY: class convenience constructor, no external invariants.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(&*panel),
                Some(action),
                mtm,
            )
        };
        button.setFont(Some(&font));
        button.setFocusRingType(NSFocusRingType::None);
        button.setRefusesFirstResponder(true);
        if let Some(appearance) = &dark_appearance {
            button.setAppearance(Some(appearance));
        }
        button.setFrame(CGRect::new(
            CGPoint::new(x, BOTTOM),
            CGSize::new(ACTION_W, ACTION_H),
        ));
        button
    };
    let reset = action_button("reset", sel!(arrowTunerReset:), INSET);
    reset.setToolTip(Some(&NSString::from_str(
        "reset all the arrow params to the shipped defaults",
    )));
    let prompt = action_button("prompt", sel!(arrowPromptCopy:), INSET + ACTION_W + 8.0);
    prompt.setToolTip(Some(&NSString::from_str(
        "copy the tuned params and a prompt that bakes them in as the \
         new defaults",
    )));
    content_area.addSubview(&reset);
    content_area.addSubview(&prompt);
    *panel.ivars().rows.borrow_mut() = rows;
    panel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_prompt_carries_every_param_and_the_steps() {
        let params = ArrowParams::DEFAULT;
        let prompt = arrow_params_prompt(params);
        for spec in ARROW_PARAM_SPECS {
            let line = format!("{} = {:.2}\n", spec.name, (spec.get)(params));
            assert!(prompt.contains(&line), "prompt lacks {line:?}");
        }
        assert!(prompt.contains("ArrowParams::DEFAULT"));
        assert!(prompt.contains("/tmp"));
        assert!(prompt.contains("cargo test"));
    }
}
