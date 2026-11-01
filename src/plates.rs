//! Shared mechanics of the highlighted underlays (plates): the named
//! sublayers a control carries to light up on hover (and, for the active
//! tool, to hold the accent). Two surfaces use it — the toolbar buttons
//! with their hover plates and the color-swatch button with its disc, and
//! the palette preset swatches with their hover rings. No state here:
//! lookups, the centering pass and the crossfade wrapper.

use objc2::rc::Retained;
use objc2_app_kit::{NSAnimationContext, NSButton, NSWorkspace};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_quartz_core::CALayer;

/// Layer names of the toolbar button's own sublayers: the plate every
/// button carries and the swatch's color disc. Matching by name keeps the
/// system's private button sublayers (the glyph image on recent macOS)
/// untouched — reframing those blew the glyphs up to the plate size.
pub const PLATE_LAYER_NAME: &str = "minisnap-toolbar-plate";
pub const DISC_LAYER_NAME: &str = "minisnap-swatch-disc";

/// Hover-container side, in points: the plate sublayer that lights up on
/// hover (and carries the active-tool accent). Deliberately larger than
/// the button cell — the container grows around the icon, which stays at
/// its glyph size, centered in the container.
pub const TOOLBAR_PLATE_SIZE: f64 = 26.0;

/// Diameter of a color swatch's visible circle, in points: the disc sits
/// inset in the swatch button, at the scale of the icon glyphs, so the
/// swatch reads as one more toolbar button rather than a big dot.
pub const SWATCH_DISC: f64 = 18.0;

/// Hover background of a toolbar button: a faint black wash, the light
/// counterpart of the dark theme's white wash.
pub const HOVER_BG: [f64; 4] = [0.0, 0.0, 0.0, 0.06];

/// Resting outline of a color swatch: dark enough to mark the lightest
/// presets (white, yellow) against the light column and popover at the 3:1
/// boundary contrast graphics are expected to reach, faint enough not to
/// read as a highlight.
pub const SWATCH_RING: [f64; 4] = [0.0, 0.0, 0.0, 0.15];

/// Hover outline of a color swatch: the ring darkens on hover instead of
/// the fill — the fill is the color itself.
pub const SWATCH_RING_HOVER: [f64; 4] = [0.0, 0.0, 0.0, 0.3];

/// Crossfade length of toolbar plate changes (hover, tool highlight), in
/// seconds. Short enough that feedback stays responsive; the database rule
/// is only that instant swaps are the anti-pattern.
const PLATE_TRANSITION: f64 = 0.15;

/// The button's sublayer with the given name, if any.
pub fn named_sublayer(button: &NSButton, name: &str) -> Option<Retained<CALayer>> {
    let layer = button.layer()?;
    // SAFETY: plain layer-tree reads, no external invariants.
    let layers = unsafe { layer.sublayers() }?;
    let count = unsafe { layers.count() };
    for i in 0..count {
        let sublayer = layers.objectAtIndexedSubscript(i);
        if let Some(named) = sublayer.name() {
            if named.to_string() == name {
                return Some(sublayer);
            }
        }
    }
    None
}

/// A toolbar button's hover/active plate, found by its layer name.
pub fn plate(button: &NSButton) -> Option<Retained<CALayer>> {
    named_sublayer(button, PLATE_LAYER_NAME)
}

/// The swatch button's color disc.
pub fn disc(button: &NSButton) -> Option<Retained<CALayer>> {
    named_sublayer(button, DISC_LAYER_NAME)
}

/// Center a plate (and, on the swatch, the color disc) inside the
/// button's settled bounds: the stack lays the button out at its
/// fitting size, so the sublayers center on whatever the frame actually
/// is. The button's other sublayers — including the system's private
/// ones that host the glyph on recent macOS — are left alone.
pub fn center_toolbar_layers(button: &NSButton) {
    let Some(layer) = button.layer() else {
        return;
    };
    let bounds = layer.bounds();
    // SAFETY: plain layer-tree reads, no external invariants.
    let layers = unsafe { layer.sublayers() };
    let Some(layers) = layers else {
        return;
    };
    let count = unsafe { layers.count() };
    for i in 0..count {
        let sublayer = layers.objectAtIndexedSubscript(i);
        let side = if let Some(named) = sublayer.name() {
            match named.to_string().as_str() {
                PLATE_LAYER_NAME => Some(TOOLBAR_PLATE_SIZE),
                DISC_LAYER_NAME => Some(SWATCH_DISC),
                _ => None,
            }
        } else {
            None
        };
        if let Some(side) = side {
            sublayer.setFrame(CGRect::new(
                CGPoint::new(
                    (bounds.size.width - side) / 2.0,
                    (bounds.size.height - side) / 2.0,
                ),
                CGSize::new(side, side),
            ));
        }
    }
}

/// Run toolbar plate changes inside an implicit-animation group: hover and
/// highlight crossfade instead of popping (instant state changes are a UX
/// anti-pattern). Only layer properties animate; a color tint applied to a
/// symbol stays instant. When the system asks to reduce motion the change
/// applies directly. Core Animation replaces a conflicting in-flight
/// transition, and the final state is whatever the block sets — nothing
/// depends on the animation finishing.
pub fn animate_plate_changes(change: impl FnOnce()) {
    let reduce_motion = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
    if reduce_motion {
        change();
        return;
    }
    NSAnimationContext::beginGrouping();
    let context = NSAnimationContext::currentContext();
    context.setDuration(PLATE_TRANSITION);
    context.setAllowsImplicitAnimation(true);
    change();
    NSAnimationContext::endGrouping();
}
