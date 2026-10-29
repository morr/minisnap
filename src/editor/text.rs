//! Core Text drawing for text and callout elements: colored fill over a
//! white halo stroke, readable on any background.

use objc2_core_foundation::{CFAttributedString, CFDictionary, CFRetained, CFString};
use objc2_core_graphics::{CGContext, CGTextDrawingMode};
use objc2_core_text::{kCTFontAttributeName, CTFont, CTLine};

use super::{Point, Rgba};

/// The font for text and callout annotations (covers Cyrillic).
const FONT_NAME: &str = "Helvetica-Bold";

/// White halo stroke width as a fraction of the font size.
const HALO_FRACTION: f64 = 0.125;

/// Typographic metrics of a text line.
#[derive(Clone, Copy, Debug)]
pub struct TextMetrics {
    pub width: f64,
    pub ascent: f64,
    pub descent: f64,
}

impl TextMetrics {
    pub fn height(&self) -> f64 {
        self.ascent + self.descent
    }
}

/// Build a one-line attributed string with `FONT_NAME` at `size`.
fn make_line(text: &str, size: f64) -> Option<CFRetained<CTLine>> {
    if text.is_empty() {
        return None;
    }
    let font = // SAFETY: null matrix means the default transform.
        unsafe { CTFont::with_name(&CFString::from_str(FONT_NAME), size, std::ptr::null()) };
    // SAFETY: the attribute key is a valid static CFString, the value a live
    // font; the dictionary borrows both for its lifetime.
    let attributes = unsafe {
        CFDictionary::<CFString, CTFont>::from_slices(&[&kCTFontAttributeName], &[&font])
    };
    // SAFETY: the only attribute (a live CTFont) is a valid value for
    // kCTFontAttributeName.
    let attributed = unsafe {
        CFAttributedString::new(
            None,
            Some(&CFString::from_str(text)),
            Some(attributes.as_ref()),
        )?
    };
    // SAFETY: attributed string is a live CF object; plain constructor.
    Some(unsafe { CTLine::with_attributed_string(&attributed) })
}

/// Measure a text line: width plus ascent/descent.
pub fn measure(text: &str, size: f64) -> Option<TextMetrics> {
    let line = make_line(text, size)?;
    let (mut ascent, mut descent, mut leading) = (0.0f64, 0.0f64, 0.0f64);
    // SAFETY: the out-parameters are plain locals.
    let width = unsafe { line.typographic_bounds(&mut ascent, &mut descent, &mut leading) };
    Some(TextMetrics {
        width,
        ascent,
        descent,
    })
}

/// Draw a text line with `origin` at the top-left of the text block. The
/// context's user space must be the image pixel space (Y down, as set up by
/// `render::draw_elements`).
pub fn draw_text(ctx: &CGContext, origin: Point, text: &str, color: Rgba, size: f64) {
    let Some(line) = make_line(text, size) else {
        return;
    };
    let Some(metrics) = measure(text, size) else {
        return;
    };
    CGContext::save_g_state(Some(ctx));
    // Core Text draws upright in a Y-up space; undo the caller's flip
    // locally and put the baseline one ascent below the block's top.
    CGContext::translate_ctm(Some(ctx), origin.x, origin.y + metrics.ascent);
    CGContext::scale_ctm(Some(ctx), 1.0, -1.0);
    CGContext::set_text_position(Some(ctx), 0.0, 0.0);
    // Pass 1: the white halo.
    CGContext::set_text_drawing_mode(Some(ctx), CGTextDrawingMode::Stroke);
    CGContext::set_rgb_stroke_color(Some(ctx), 1.0, 1.0, 1.0, 1.0);
    CGContext::set_line_width(Some(ctx), size * HALO_FRACTION);
    // SAFETY: plain draw on a live line/context pair.
    unsafe { line.draw(ctx) };
    // Pass 2: the colored fill.
    CGContext::set_text_drawing_mode(Some(ctx), CGTextDrawingMode::Fill);
    CGContext::set_rgb_fill_color(Some(ctx), color[0], color[1], color[2], color[3]);
    // SAFETY: plain draw on a live line/context pair.
    unsafe { line.draw(ctx) };
    CGContext::restore_g_state(Some(ctx));
}

#[cfg(test)]
mod tests {
    use super::super::tests::{pixel_bytes, solid_frame};
    use super::super::{Element, ElementKind, Point, Rgba};
    use super::*;

    const MAGENTA: Rgba = [1.0, 0.0, 1.0, 1.0];

    #[test]
    fn nonempty_text_measures_positive() {
        let m = measure("Hello", 16.0).expect("metrics");
        assert!(m.width > 0.0 && m.height() > 0.0);
        // Cyrillic glyphs resolve too.
        let m = measure("Привет мир", 14.0).expect("metrics");
        assert!(m.width > 0.0 && m.height() > 0.0);
        assert!(measure("", 16.0).is_none());
    }

    #[test]
    fn text_renders_into_the_composite() {
        let base = solid_frame(96, 64);
        let element = Element {
            id: 1,
            kind: ElementKind::Text {
                origin: Point::new(10.0, 10.0),
                text: "WW".into(),
                color: MAGENTA,
                size: 32.0,
            },
        };
        let composite =
            super::super::render::render_composite(&base, &[element]).expect("composite");
        let bytes = pixel_bytes(&composite);
        // Somewhere inside the text block a glyph or its halo draws over the
        // gray base.
        let non_gray = (0..64usize)
            .flat_map(|y| (0..96usize).map(move |x| (x, y)))
            .filter(|(x, y)| {
                let i = (y * 96 + x) * 4;
                let (r, g, b) = (bytes[i] as i32, bytes[i + 1] as i32, bytes[i + 2] as i32);
                (r - 128).abs() > 40 || (g - 128).abs() > 40 || (b - 128).abs() > 40
            })
            .count();
        assert!(non_gray > 0, "no glyph pixels found");
    }
}
