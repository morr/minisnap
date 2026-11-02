//! HUD of the frozen-frame overlay: the chrome layers over the frozen
//! frame (dimming strips, loupe, axis guides, coordinate readout, the red
//! mode border) and the logical-pixel frame sampler they read. Extracted
//! from `overlay.rs`, which keeps the session-side orchestration (window,
//! surfaces, target switching, click resolution).

use objc2_core_foundation::{CFRetained, CGPoint, CGRect};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImage, CGImageAlphaInfo,
    CGContext,
};

/// The frozen frame downsampled to logical pixels (one Retina pixel pair
/// becomes one unit): the loupe magnifies UI pixels instead of physical
/// ones, and the axis guides sample content brightness from `buf`.
pub(crate) struct FrameSampler {
    /// Kept alive alongside the image: the exported image references the
    /// context's buffer.
    #[allow(dead_code)]
    ctx: CFRetained<CGContext>,
    image: CFRetained<CGImage>,
    /// Backing bytes of `ctx`; the context does not own them, so the buffer
    /// must outlive the exported image. Nothing reads it in production any
    /// more (the content-adaptive guide coloring is gone) — only the tests.
    #[allow(dead_code)]
    buf: Vec<u8>,
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Physical pixels per logical pixel (backing scale).
    pub(crate) factor: usize,
}

impl FrameSampler {
    /// Downsample the frozen frame to logical pixels (one backing-scale
    /// pixel group per unit). Returns None when the bitmap context could not
    /// be created; the HUD then works without the loupe.
    pub(crate) fn new(w_px: usize, h_px: usize, scale: f64, image: &CGImage) -> Option<Self> {
        let factor = (scale.round() as usize).max(1);
        let lw = (w_px / factor).max(1);
        let lh = (h_px / factor).max(1);
        let space = CGColorSpace::new_device_rgb()?;
        let mut buf = vec![0u8; lw * lh * 4];
        // SAFETY: plain CGBitmapContextCreate; buf is stored in the sampler
        // alongside the context whose buffer it backs.
        let ctx = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                lw,
                lh,
                8,
                lw * 4,
                Some(&*space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }?;
        CGContext::draw_image(
            Some(&ctx),
            CGRect::new(
                CGPoint::ZERO,
                objc2_core_foundation::CGSize::new(lw as f64, lh as f64),
            ),
            Some(image),
        );
        let down = CGBitmapContextCreateImage(Some(&ctx))?;
        Some(FrameSampler {
            ctx,
            image: down,
            buf,
            width: lw,
            height: lh,
            factor,
        })
    }

    pub(crate) fn image(&self) -> &CGImage {
        &self.image
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small synthetic frame via the same CG bitmap pipeline.
    fn synthetic_frame(w: usize, h: usize) -> CFRetained<CGImage> {
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                // Left half black, right half white.
                let v = if x < w / 2 { 0 } else { 255 };
                buf[i] = v;
                buf[i + 1] = v;
                buf[i + 2] = v;
                buf[i + 3] = 255;
            }
        }
        let space = CGColorSpace::new_device_rgb().expect("rgb colorspace");
        // SAFETY: buf outlives the context within this function.
        let ctx = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                Some(&*space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .expect("bitmap context");
        CGBitmapContextCreateImage(Some(&ctx)).expect("image from context")
    }

    /// Mean per-channel brightness of one sampler column, read straight
    /// from the backing bytes.
    fn column_brightness(sampler: &FrameSampler, x: usize) -> u32 {
        let mut total: u64 = 0;
        for y in 0..sampler.height {
            let i = (y * sampler.width + x) * 4;
            total += (sampler.buf[i] as u64) + (sampler.buf[i + 1] as u64) + (sampler.buf[i + 2] as u64);
        }
        (total / (sampler.height as u64 * 3)) as u32
    }

    #[test]
    fn sampler_downsamples_to_logical_pixels() {
        let image = synthetic_frame(64, 64);
        let sampler = FrameSampler::new(64, 64, 2.0, &image).expect("sampler");
        assert_eq!((sampler.width, sampler.height, sampler.factor), (32, 32, 2));
        // Left half of the frame is black, right half white: a column in the
        // dark half reads dark, one in the light half reads light.
        let dark = column_brightness(&sampler, 4);
        let light = column_brightness(&sampler, 28);
        assert!(dark < 128, "dark col: {dark}");
        assert!(light > 128, "light col: {light}");
    }

    #[test]
    fn sampler_handles_scale_one() {
        let image = synthetic_frame(64, 64);
        let sampler = FrameSampler::new(64, 64, 1.0, &image).expect("sampler");
        assert_eq!((sampler.width, sampler.height, sampler.factor), (64, 64, 1));
    }
}
