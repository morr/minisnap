//! One-off probe: where does a committed text element's ink land when the
//! same `draw_elements` calls run through a raw CG bitmap context vs an
//! AppKit bitmap-rep context (the nature of the window backing and of
//! `cacheDisplayInRect`)? Prints both ink tops in image rows.

use objc2::{define_class, msg_send, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSBitmapImageFileType, NSColor, NSGraphicsContext,
    NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColorSpace, CGContext, CGImage, CGImageAlphaInfo};
use objc2_foundation::{NSDictionary, NSObjectProtocol, NSString};

use minisnap::editor::{Element, ElementKind, Point, Rgba};

const MAGENTA: Rgba = [1.0, 0.0, 1.0, 1.0];

struct ProbeIvars {
    base: std::cell::RefCell<Option<CFRetained<CGImage>>>,
    elements: std::cell::RefCell<Vec<Element>>,
}

define_class!(
    /// A view whose drawRect: replicates the canvas draw: card fill, base
    /// image, then draw_elements in the current context — the exact path
    /// cacheDisplayInRect exercises on the real canvas.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ProbeIvars]
    struct ProbeView;

    unsafe impl NSObjectProtocol for ProbeView {}

    impl ProbeView {
        #[unsafe(method_id(initWithFrame:))]
        unsafe fn __init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> objc2::rc::Retained<Self> {
            let this = this.set_ivars(ProbeIvars {
                base: std::cell::RefCell::new(None),
                elements: std::cell::RefCell::new(Vec::new()),
            });
            // SAFETY: NSView's designated initializer with plain ivars set.
            unsafe { msg_send![super(this), initWithFrame: frame_rect] }
        }

        #[unsafe(method(drawRect:))]
        unsafe fn __draw_rect(&self, _dirty: CGRect) {
            let Some(context) = NSGraphicsContext::currentContext() else {
                return;
            };
            let cg = context.CGContext();
            let stage = std::env::var("PROBE_STAGE")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            let ivars = self.ivars();
            let base = ivars.base.borrow().clone().expect("base");
            // draw()'s exact stage stack:
            // card fill+stroke+addClip (stage>=2) → checker fill (>=3) →
            // translate(4,4) (>=5) → clip image rect (>=4) → base image
            // (>=1) → draw_elements.
            CGContext::save_g_state(Some(&cg));
            let path = unsafe {
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    CGRect::new(CGPoint::new(4.0, 4.0), CGSize::new(192.0, 146.0)),
                    8.0,
                    8.0,
                )
            };
            unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(0.97, 0.97, 0.97, 1.0).setFill() };
            unsafe { path.fill() };
            if stage >= 2 {
                unsafe { path.addClip() };
            }
            if stage >= 3 {
                // The checker: a light fill over the card, like checker_color.
                unsafe {
                    NSColor::colorWithSRGBRed_green_blue_alpha(0.85, 0.85, 0.87, 1.0).setFill()
                };
                CGContext::fill_rect(
                    Some(&cg),
                    CGRect::new(CGPoint::new(4.0, 4.0), CGSize::new(192.0, 146.0)),
                );
            }
            if stage >= 5 {
                CGContext::translate_ctm(Some(&cg), 4.0, 4.0);
            }
            if stage >= 4 {
                CGContext::clip_to_rect(
                    Some(&cg),
                    CGRect::new(CGPoint::ZERO, CGSize::new(200.0, 150.0)),
                );
            }
            if stage >= 1 {
                CGContext::draw_image(
                    Some(&cg),
                    CGRect::new(CGPoint::ZERO, CGSize::new(200.0, 150.0)),
                    Some(&base),
                );
            }
            minisnap::editor::render::draw_elements(&cg, &base, 150.0, &ivars.elements.borrow());
            CGContext::restore_g_state(Some(&cg));
        }
    }
);

objc2::extern_methods!(
    unsafe impl ProbeView {
        #[unsafe(method(initWithFrame:))]
        unsafe fn init_with_frame(
            this: objc2::rc::Allocated<Self>,
            frame_rect: CGRect,
        ) -> objc2::rc::Retained<Self>;
    }
);

fn solid_frame(w: usize, h: usize) -> CFRetained<CGImage> {
    use objc2_core_graphics::{CGBitmapContextCreate, CGBitmapContextCreateImage};
    let buf = vec![128u8; w * h * 4];
    let space = CGColorSpace::new_device_rgb().expect("rgb colorspace");
    let ctx = unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            w,
            h,
            8,
            w * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }
    .expect("bitmap context");
    unsafe { CGBitmapContextCreateImage(Some(&ctx)) }.expect("image from context")
}

fn ink_top(bytes: &[u8], w: usize, h: usize) -> Option<f64> {
    (0..h)
        .find(|&y| {
            (0..w).any(|x| {
                let px = &bytes[(y * w + x) * 4..];
                px[0] > 180 && px[2] > 180 && px[1] < 120
            })
        })
        .map(|y| y as f64)
}

fn pixel_bytes(image: &CGImage) -> Vec<u8> {
    use objc2_core_foundation::{CFData, CFRetained};
    use objc2_core_graphics::CGDataProvider;
    let provider = CGImage::data_provider(Some(image)).expect("data provider");
    let data: CFRetained<CFData> = CGDataProvider::data(Some(&provider)).expect("data");
    data.to_vec()
}

fn main() {
    let Some(mtm) = MainThreadMarker::new() else {
        panic!("the probe must run on the main thread");
    };
    let base = solid_frame(200, 150);
    let element = Element {
        id: 1,
        kind: ElementKind::Text {
            origin: Point::new(20.0, 40.0),
            text: "WWWW".into(),
            color: MAGENTA,
            size: 32.0,
        },
    };

    // Raw CG bitmap (the composite path).
    let one_x =
        minisnap::editor::render::render_composite(&base, &[element.clone()]).expect("composite");
    let top_cg = ink_top(&pixel_bytes(&one_x), 200, 150);
    println!("raw CG bitmap: ink top at image row {:?}", top_cg);

    // The cacheDisplayInRect path on a drawRect: replica — the exact
    // mechanism agent_window_png uses on the real canvas view.
    let probe = unsafe {
        ProbeView::init_with_frame(
            ProbeView::alloc(mtm),
            CGRect::new(CGPoint::ZERO, CGSize::new(200.0, 150.0)),
        )
    };
    probe.ivars().base.borrow_mut().replace(base.clone());
    probe.ivars().elements.borrow_mut().push(element);
    // The real canvas is a CHILD of the contentView (a toolbar sibling
    // above it), not the content view itself — replicate: default content
    // view, the probe docked to the bottom 150pt of a 186pt window (36pt
    // toolbar strip above), and cacheDisplayInRect on the content view,
    // like agent_window_png does.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            CGRect::new(CGPoint::ZERO, CGSize::new(200.0, 186.0)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    let content = window.contentView().expect("content view");
    unsafe { probe.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(200.0, 150.0))) };
    unsafe {
        probe.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        )
    };
    // A toolbar-ish sibling drawn BEFORE the canvas: a plain NSView with
    // an NSTextField label — AppKit text drawing, like the real toolbar's
    // file label. If the label's drawing leaves the shared cached context
    // with a flipped text matrix, the canvas's CoreText output flips.
    let toolbar = unsafe {
        NSView::initWithFrame(
            mtm.alloc::<objc2_app_kit::NSView>(),
            CGRect::new(CGPoint::new(0.0, 150.0), CGSize::new(200.0, 36.0)),
        )
    };
    let label = unsafe {
        NSTextField::labelWithString(&objc2_foundation::NSString::from_str("test-gray.png"), mtm)
    };
    unsafe {
        label.setFrame(CGRect::new(
            CGPoint::new(8.0, 8.0),
            CGSize::new(120.0, 20.0),
        ))
    };
    toolbar.addSubview(&label);
    content.addSubview(&toolbar);
    content.addSubview(&probe);
    let rep = content
        .bitmapImageRepForCachingDisplayInRect(content.bounds())
        .expect("bitmap rep");
    unsafe { content.cacheDisplayInRect_toBitmapImageRep(content.bounds(), &rep) };

    // Serialize the rep the way agent_window_png does, drop the PNG next
    // to the Swift scanners.
    // SAFETY: plain serialize of a live rep; the empty properties
    // dictionary carries no typed values to misuse.
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .expect("PNG encode");
    let out = std::path::Path::new("/tmp/minisnap-drive/appkit_probe.png");
    data.writeToFile_atomically(&NSString::from_str(out.to_str().expect("utf8 path")), true);
    println!("wrote {}", out.display());
}
