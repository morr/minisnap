//! Output of a finished selection: pixel-space crop of the T0 frame, the
//! pasteboard, and the silent PNG file with collision-safe naming.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypePNG,
    NSPasteboardTypeTIFF,
};
use objc2_core_foundation::{CFRetained, CFString, CFURL, CGRect};
use objc2_core_graphics::CGImage;
use objc2_foundation::NSDictionary;
use objc2_image_io::CGImageDestination;

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

/// Local calendar components of a Unix timestamp shifted by `utc_offset_secs`
/// (civil-from-days, Howard Hinnant's algorithm; valid for any year ≥ 1).
/// Returns (year, month, day, hour, minute, second).
fn timestamp_components(unix_secs: i64, utc_offset_secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = unix_secs + utc_offset_secs;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// `MiniSnap YYYY-MM-DD HH-MM-SS` in local time.
fn shot_base_name(unix_secs: i64, utc_offset_secs: i64) -> String {
    let (y, mo, d, h, mi, s) = timestamp_components(unix_secs, utc_offset_secs);
    format!("MiniSnap {y:04}-{mo:02}-{d:02} {h:02}-{mi:02}-{s:02}")
}

/// First unused `MiniSnap …png` path for the given local time. Same-second
/// collisions get a Finder-style ` (2)`, ` (3)`… suffix; a nanosecond
/// fallback guarantees the name is free even after 999 shots in one second.
fn unique_shot_path(dir: &Path, unix_secs: i64, utc_offset_secs: i64) -> PathBuf {
    let base = shot_base_name(unix_secs, utc_offset_secs);
    let first = dir.join(format!("{base}.png"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000u32 {
        let candidate = dir.join(format!("{base} ({n}).png"));
        if !candidate.exists() {
            return candidate;
        }
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    dir.join(format!("{base} ({nanos}).png"))
}

/// System time zone offset from UTC, in seconds.
fn local_utc_offset_secs() -> i64 {
    objc2_foundation::NSTimeZone::systemTimeZone().secondsFromGMT() as i64
}

/// Encode the image as PNG into `path` via ImageIO.
pub(crate) fn encode_png(image: &CGImage, path: &Path) -> Result<(), String> {
    let url = CFURL::from_file_path(path).ok_or_else(|| format!("bad path {}", path.display()))?;
    // SAFETY: url/type are live CF objects; the destination type parameters
    // (count=1, no options) have no further invariants.
    let dest = unsafe {
        CGImageDestination::with_url(&url, &CFString::from_str("public.png"), 1, None)
    }
    .ok_or_else(|| format!("cannot create image destination for {}", path.display()))?;
    // SAFETY: `image` is a live CGImage; no properties needed.
    unsafe { dest.add_image(image, None) };
    if !unsafe { dest.finalize() } {
        return Err(format!("PNG encoding failed for {}", path.display()));
    }
    Ok(())
}

/// Encode into `path` atomically: write a `.tmp` sibling first, then rename,
/// so a crash mid-encode never leaves a half-written shot behind.
pub(crate) fn encode_png_atomic(image: &CGImage, path: &Path) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    encode_png(image, &tmp)?;
    fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))
}

/// Does this file name match the app's own shot pattern —
/// `MiniSnap …png`, with an optional ` (n)` before the extension?
/// Foreign files in the folder never match and are never touched.
fn is_shot_file_name(name: &str) -> bool {
    name.starts_with("MiniSnap ") && name.ends_with(".png")
}

/// All shot files in `dir`, oldest first (by modification time, name as
/// tie-break). Missing directory reads as empty.
fn shot_files_oldest_first(dir: &Path) -> Vec<(PathBuf, std::ffi::OsString)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, std::ffi::OsString, std::time::SystemTime)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            if !is_shot_file_name(&name.to_string_lossy()) {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((entry.path(), name, modified))
        })
        .collect();
    files.sort_by_key(|(_, name, modified)| (*modified, name.clone()));
    files.into_iter().map(|(path, name, _)| (path, name)).collect()
}

/// Keep at most `limit` shot files in `dir`, deleting the oldest ones.
/// Best-effort: a file that cannot be removed is logged and skipped.
pub fn enforce_retention(dir: &Path, limit: u32) {
    let files = shot_files_oldest_first(dir);
    let excess = files.len().saturating_sub(limit as usize);
    for (path, name) in files.into_iter().take(excess) {
        if let Err(e) = fs::remove_file(&path) {
            eprintln!("retention: cannot remove {}: {e}", path.display());
        } else {
            eprintln!("retention: removed {}", name.to_string_lossy());
        }
    }
}

/// Save the shot silently as `dir/MiniSnap YYYY-MM-DD HH-MM-SS.png`, creating
/// `dir` (and parents) on demand, numbering same-second collisions and
/// enforcing the retention limit. Returns the file path.
pub fn save_to_file(image: &CGImage, dir: &Path, retention: u32) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let path = unique_shot_path(dir, unix_secs, local_utc_offset_secs());
    encode_png(image, &path)?;
    enforce_retention(dir, retention);
    Ok(path)
}

/// The newest shot file in `dir`, if any. A missing or empty folder reads
/// as None.
pub fn latest_shot(dir: &Path) -> Option<PathBuf> {
    shot_files_oldest_first(dir)
        .pop()
        .map(|(path, _)| path)
}

/// All shot files in `dir`, oldest first. Missing directory reads as empty.
pub fn sorted_shots(dir: &Path) -> Vec<PathBuf> {
    shot_files_oldest_first(dir)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
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

    #[test]
    fn epoch_maps_to_unix_start() {
        assert_eq!(timestamp_components(0, 0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn utc_timestamp_splits_into_components() {
        // 2026-10-29 12:34:56 UTC.
        assert_eq!(timestamp_components(1793277296, 0), (2026, 10, 29, 12, 34, 56));
    }

    #[test]
    fn offsets_roll_days_forward_and_back() {
        // 2026-10-29 23:59:00 UTC +1h → next day.
        assert_eq!(timestamp_components(1793318340, 3600), (2026, 10, 30, 0, 59, 0));
        // 2026-10-29 01:30:00 UTC -2h → previous day.
        assert_eq!(timestamp_components(1793237400, -7200), (2026, 10, 28, 23, 30, 0));
    }

    #[test]
    fn base_name_matches_spec_pattern() {
        // 2026-10-29 12:34:56 UTC rendered with a +3h offset.
        assert_eq!(shot_base_name(1793277296, 3 * 3600), "MiniSnap 2026-10-29 15-34-56");
    }

    #[test]
    fn unique_path_numbers_same_second_collisions() {
        let dir = std::env::temp_dir().join(format!("minisnap-output-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let base = shot_base_name(1793277296, 0);

        let first = unique_shot_path(&dir, 1793277296, 0);
        assert_eq!(first, dir.join(format!("{base}.png")));
        fs::write(&first, b"x").unwrap();

        let second = unique_shot_path(&dir, 1793277296, 0);
        assert_eq!(second, dir.join(format!("{base} (2).png")));
        fs::write(&second, b"x").unwrap();

        let third = unique_shot_path(&dir, 1793277296, 0);
        assert_eq!(third, dir.join(format!("{base} (3).png")));

        // A different second is unaffected by the collisions.
        let other = unique_shot_path(&dir, 1793277296 + 1, 0);
        assert!(other.to_string_lossy().ends_with("12-34-57.png"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_creates_nested_dir_and_writes_png() {
        let dir = std::env::temp_dir().join(format!("minisnap-output-save-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let nested = dir.join("one").join("minisnap");
        let frame = synthetic_frame();
        let path = save_to_file(&frame, &nested, 100).expect("save");
        assert!(path.is_file(), "file written");
        assert!(path.extension().unwrap() == "png");
        // A valid PNG starts with the well-known signature.
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        // Second save in the same second gets the collision number.
        let second = save_to_file(&frame, &nested, 100).expect("save again");
        assert_ne!(path, second);
        assert!(second.to_string_lossy().contains(" (2)"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_keeps_latest_and_spares_foreign_files() {
        use std::io::Write;
        let dir =
            std::env::temp_dir().join(format!("minisnap-output-retention-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 101 own shots created sequentially, with strictly increasing
        // modification times: name[0] is the oldest.
        let start = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mut names = Vec::new();
        for i in 0..101u32 {
            let name = format!(
                "MiniSnap 2026-01-01 {:02}-{:02}-{:02}.png",
                i / 3600,
                (i / 60) % 60,
                i % 60
            );
            let path = dir.join(&name);
            let mut file = fs::File::create(&path).unwrap();
            file.write_all(b"png").unwrap();
            file.set_modified(start + std::time::Duration::from_secs(i as u64))
                .unwrap();
            drop(file);
            names.push(name);
        }
        // Foreign files that never match the shot pattern.
        let foreign = ["holiday photo.jpg", "MiniSnap notes.txt", "minisnap lower.png"];
        for name in foreign {
            fs::write(dir.join(name), b"x").unwrap();
        }

        enforce_retention(&dir, 100);

        let left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left.iter().filter(|n| is_shot_file_name(n)).count(), 100);
        assert!(!left.contains(&names[0]), "oldest shot removed");
        assert!(left.contains(&names[100]), "newest shot kept");
        for name in foreign {
            assert!(left.contains(&name.to_string()), "{name} untouched");
        }

        // A limit above the current count removes nothing.
        enforce_retention(&dir, 10_000);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 100 + foreign.len());
        let _ = fs::remove_dir_all(&dir);
    }
}
