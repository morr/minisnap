//! Core Text drawing for text and callout elements: colored fill over a
//! white halo stroke, readable on any background.

use objc2_core_foundation::{CFAttributedString, CFDictionary, CFRetained, CFString, CFType};
use objc2_core_graphics::{CGColor, CGColorSpace, CGContext, CGTextDrawingMode};
use objc2_core_text::{
    kCTFontAttributeName, kCTForegroundColorAttributeName, CTFont, CTLine,
};

use super::{Point, Rgba};

/// The font for text and callout annotations (covers Cyrillic).
pub(crate) const FONT_NAME: &str = "Helvetica-Bold";

/// White halo stroke width as a fraction of the font size.
const HALO_FRACTION: f64 = 0.125;

/// Height of one text line as a fraction of the font size.
const LINE_HEIGHT_FRACTION: f64 = 1.25;

/// Height of one line of text at `size`.
pub fn line_height(size: f64) -> f64 {
    size * LINE_HEIGHT_FRACTION
}

/// Typographic metrics of a whole text block: the widest line and the
/// stacked height.
pub struct BlockMetrics {
    pub width: f64,
    pub height: f64,
}

/// Measure a text block; lines are split on `\n` (a Shift+Enter line
/// separator is normalized to `\n` by the field editor commit).
pub fn measure_block(text: &str, size: f64) -> Option<BlockMetrics> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut width = 0.0_f64;
    for line in &lines {
        if let Some(m) = measure(line, size) {
            width = width.max(m.width);
        }
    }
    if width <= 0.0 {
        return None;
    }
    let lh = line_height(size);
    let last = lines
        .last()
        .and_then(|line| measure(line, size))
        .map(|m| m.height())
        .unwrap_or(lh);
    Some(BlockMetrics {
        width,
        height: (lines.len() - 1) as f64 * lh + last,
    })
}

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

/// Build a one-line attributed string with `FONT_NAME` at `size`, to be
/// drawn in `color`. The color rides the string as
/// `kCTForegroundColorAttributeName`: macOS 26's CoreText stopped
/// delivering the context fill color to `CTLineDraw` (the glyphs come out
/// black), while the attribute works.
fn make_line(text: &str, size: f64, color: Rgba) -> Option<CFRetained<CTLine>> {
    if text.is_empty() {
        return None;
    }
    let font = // SAFETY: null matrix means the default transform.
        unsafe { CTFont::with_name(&CFString::from_str(FONT_NAME), size, std::ptr::null()) };
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: a live colorspace and a live four-component array.
    let cg_color = unsafe { CGColor::new(Some(&space), color.as_ptr())? };
    // SAFETY: reading extern static CFString constants.
    let keys = unsafe { [kCTFontAttributeName, kCTForegroundColorAttributeName] };
    let values: [&CFType; 2] = [&*font, &*cg_color];
    // SAFETY: the keys are static CFStrings, the values live CF objects;
    // the dictionary borrows them for its lifetime.
    let attributes = unsafe {
        CFDictionary::<CFString, CFType>::from_slices(&keys, &values)
    };
    // SAFETY: the attributes are valid keys and values for an attributed
    // string.
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
    // The metrics do not depend on the color; black is as good as any.
    let line = make_line(text, size, [0.0, 0.0, 0.0, 1.0])?;
    let (mut ascent, mut descent, mut leading) = (0.0f64, 0.0f64, 0.0f64);
    // SAFETY: the out-parameters are plain locals.
    let width = unsafe { line.typographic_bounds(&mut ascent, &mut descent, &mut leading) };
    Some(TextMetrics {
        width,
        ascent,
        descent,
    })
}

/// Draw a text block with `origin` at the top-left of the block, one line
/// per `\n`. The context's user space must be the image pixel space (Y
/// down, as set up by `render::draw_elements`).
pub fn draw_text(ctx: &CGContext, origin: Point, text: &str, color: Rgba, size: f64) {
    let lh = line_height(size);
    for (index, line_text) in text.split('\n').enumerate() {
        let Some(line) = make_line(line_text, size, color) else {
            continue;
        };
        let Some(metrics) = measure(line_text, size) else {
            continue;
        };
        let line_origin = Point::new(origin.x, origin.y + index as f64 * lh);
        draw_line(ctx, line_origin, &line, &metrics, size);
    }
}

/// Draw one measured line with `origin` at the top-left of the line box.
/// The glyphs' color rides the attributed string (see `make_line`); the
/// text position resets before each pass — macOS 26's CoreText advances it
/// across a draw, which used to shove the fill pass one line-width aside.
fn draw_line(ctx: &CGContext, origin: Point, line: &CTLine, metrics: &TextMetrics, size: f64) {
    CGContext::save_g_state(Some(ctx));
    // Core Text draws upright in a Y-up space; undo the caller's flip
    // locally and put the baseline one ascent below the block's top.
    CGContext::translate_ctm(Some(ctx), origin.x, origin.y + metrics.ascent);
    CGContext::scale_ctm(Some(ctx), 1.0, -1.0);
    // Pass 1: the white halo.
    CGContext::set_text_position(Some(ctx), 0.0, 0.0);
    CGContext::set_text_drawing_mode(Some(ctx), CGTextDrawingMode::Stroke);
    CGContext::set_rgb_stroke_color(Some(ctx), 1.0, 1.0, 1.0, 1.0);
    CGContext::set_line_width(Some(ctx), size * HALO_FRACTION);
    // SAFETY: plain draw on a live line/context pair.
    unsafe { line.draw(ctx) };
    // Pass 2: the colored fill.
    CGContext::set_text_position(Some(ctx), 0.0, 0.0);
    CGContext::set_text_drawing_mode(Some(ctx), CGTextDrawingMode::Fill);
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
    fn multiline_text_measures_and_renders() {
        let single = measure_block("WW", 16.0).expect("single");
        let multi = measure_block("WW\nWW", 16.0).expect("multi");
        assert_eq!(multi.width, single.width);
        assert!(multi.height > single.height * 1.5);
        assert!(measure_block("", 16.0).is_none());

        // Both lines render: non-gray pixels appear in the top and the
        // bottom halves of the block.
        let base = solid_frame(96, 64);
        let element = Element {
            id: 1,
            kind: ElementKind::Text {
                origin: Point::new(10.0, 6.0),
                text: "WW\nWW".into(),
                color: MAGENTA,
                size: 24.0,
            },
        };
        let composite =
            super::super::render::render_composite(&base, &[element]).expect("composite");
        let bytes = pixel_bytes(&composite);
        let non_gray_in = |y0: usize, y1: usize| {
            (y0..y1).any(|y| {
                (0..96usize).any(|x| {
                    let i = (y * 96 + x) * 4;
                    (bytes[i] as i32 - 128).abs() > 40 || (bytes[i + 2] as i32 - 128).abs() > 40
                })
            })
        };
        assert!(non_gray_in(0, 32), "first line missing");
        assert!(non_gray_in(32, 64), "second line missing");
    }

    #[test]
    fn text_fill_renders_in_the_element_color() {
        // Regression guard for macOS 26: CoreText stopped delivering the
        // context fill color to CTLineDraw, so the color must ride the
        // attributed string — the glyphs used to come out black.
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
        let magenta = bytes
            .chunks_exact(4)
            .filter(|px| px[0] > 180 && px[2] > 180 && px[1] < 120)
            .count();
        assert!(magenta > 0, "the fill did not render in the element color");
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
