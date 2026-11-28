//! AppKit from Rust through objc2 (ADR-0056): what the app holds outside every GPUI window, and
//! where its windows sit on screen. Each piece is a typed Rust call on the main thread, never an op of the Swift bridge's JSON ABI.
pub mod clipboard;
pub mod dialogs;
pub mod film;
pub mod images;
pub mod live_resize;
pub mod motion;
pub mod page_scheme;
pub mod pane_host;
pub mod status_item;
pub mod video_stream;
pub mod web;
pub mod window;

pub use dialogs::{Alert, Answer, Dialogs, MenuItem};
pub use status_item::{ItemEvent, MenuEntry, StatusItem};

use objc2::{AnyThread as _, MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace, NSGraphicsContext, NSImage};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize};

/// The main thread's marker. Every caller runs on GPUI's main thread, where AppKit lives.
pub fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().expect("AppKit is called from the main thread")
}

/// `image` drawn into a bitmap `scale` pixels per point, as a PNG.
pub fn png(image: &NSImage, scale: f64) -> Option<Vec<u8>> {
    png_in(image, image.size(), scale)
}

/// `image` drawn to fill `size` points at `scale` pixels per point, as a PNG.
pub fn png_in(image: &NSImage, size: NSSize, scale: f64) -> Option<Vec<u8>> {
    let (w, h) = ((size.width * scale).ceil() as isize, (size.height * scale).ceil() as isize);
    if w <= 0 || h <= 0 {
        return None;
    }
    // SAFETY: a fresh 8-bit RGBA bitmap that allocates its own planes (a null plane pointer).
    let rep: Retained<NSBitmapImageRep> = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(), std::ptr::null_mut(), w, h, 8, 4, true, false, NSDeviceRGBColorSpace, 0, 0,
        )
    }?;
    rep.setSize(size);
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect_fromRect_operation_fraction(NSRect::new(NSPoint::ZERO, size), NSRect::ZERO, NSCompositingOperation::SourceOver, 1.);
    NSGraphicsContext::restoreGraphicsState_class();
    // SAFETY: an empty properties dictionary is what the PNG encoder accepts.
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
    Some(data.to_vec())
}
