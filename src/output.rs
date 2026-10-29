//! Output of a finished selection: pixel-space crop of the T0 frame and the
//! pasteboard. File saving and retention land in 5.2/5.3.

use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypePNG,
    NSPasteboardTypeTIFF,
};
use objc2_core_foundation::{CFRetained, CGRect};
use objc2_core_graphics::CGImage;
use objc2_foundation::NSDictionary;

/// Crop the T0 frame to `rect` (top-left-origin pixel space). None for an
/// empty rect.
pub fn crop(image: &CGImage, rect: CGRect) -> Option<CFRetained<CGImage>> {
    let rect = rect.standardize();
    if rect.size.width <= 0.0 || rect.size.height <= 0.0 {
        return None;
    }
    CGImage::with_image_in_rect(Some(image), rect)
}

/// Put the image on the general pasteboard as PNG (plus TIFF for older
/// apps). Main thread.
pub fn copy_to_pasteboard(image: &CGImage) -> Result<(), String> {
    MainThreadMarker::new().ok_or("not on the main thread")?;
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::dictionary())
    }
    .ok_or("PNG encoding failed")?;
    let tiff = rep
        .TIFFRepresentation()
        .ok_or("TIFF encoding failed")?;
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let png_ok = pb.setData_forType(Some(&png), unsafe { NSPasteboardTypePNG });
    let tiff_ok = pb.setData_forType(Some(&tiff), unsafe { NSPasteboardTypeTIFF });
    if !(png_ok && tiff_ok) {
        return Err(format!("pasteboard rejected data (png={png_ok}, tiff={tiff_ok})"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::{CGPoint, CGSize};
    use objc2_core_graphics::{CGColorSpace, CGImageAlphaInfo};
    use objc2_core_graphics::{CGBitmapContextCreateImage, CGContext};

    // The plain CoreGraphics bitmap-context constructor (objc2-core-graphics
    // only wraps the block-based adaptive variant).
    extern "C-unwind" {
        fn CGBitmapContextCreate(
            data: *mut std::ffi::c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: &CGColorSpace,
            bitmap_info: u32,
        ) -> *mut CGContext;
    }

    /// A 64x64 RGB frame, red above y=32, blue below (top-left origin).
    fn synthetic_frame() -> CFRetained<CGImage> {
        let (w, h) = (64usize, 64usize);
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let (r, g, b) = if y < 32 { (255, 0, 0) } else { (0, 0, 255) };
                buf[i] = r;
                buf[i + 1] = g;
                buf[i + 2] = b;
                buf[i + 3] = 255;
            }
        }
        let space = CGColorSpace::new_device_rgb().expect("rgb colorspace");
        // SAFETY: buf outlives the context within this function.
        let ctx_ptr = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                &space,
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        };
        assert!(!ctx_ptr.is_null(), "bitmap context");
        // SAFETY: non-null context from the constructor above.
        let ctx = unsafe { CFRetained::from_raw(std::ptr::NonNull::new_unchecked(ctx_ptr)) };
        // The image references the context's own buffer; no drawing needed.
        // SAFETY: plain C constructor on a live context.
        unsafe { CGBitmapContextCreateImage(Some(&ctx)) }.expect("image from context")
    }

    #[test]
    fn crop_maps_top_left_origin_pixels() {
        let frame = synthetic_frame();
        let w = CGImage::width(Some(&frame));
        let h = CGImage::height(Some(&frame));
        assert_eq!((w, h), (64, 64));

        // Bottom half in pixel space (top-left origin): y=32..64, the blue
        // rows.
        let rect = CGRect::new(CGPoint::new(16.0, 32.0), CGSize::new(32.0, 32.0));
        let cropped = crop(&frame, rect).expect("crop");
        assert_eq!(
            (CGImage::width(Some(&cropped)), CGImage::height(Some(&cropped))),
            (32, 32)
        );
    }

    #[test]
    fn crop_rejects_empty_rects() {
        let frame = synthetic_frame();
        assert!(crop(&frame, CGRect::ZERO).is_none());
        let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 10.0));
        assert!(crop(&frame, rect).is_none());
    }
}
