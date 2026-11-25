//! What AppKit and CoreImage draw for the client: SF Symbols and the app's icon as PNGs (GPUI
//! has no symbol font of its own), and QR codes as modules or as an image on the clipboard.
use objc2::{AnyThread as _, MainThreadMarker, rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{NSApplication, NSCIImageRep, NSColor, NSFontWeightRegular, NSImage, NSImageSymbolConfiguration, NSPasteboard, NSScreen};
use objc2_core_foundation::CGAffineTransform;
use objc2_core_image::{CIFilter, CIImage};
use objc2_foundation::{NSArray, NSData, NSSize, NSString, NSObjectNSKeyValueCoding as _};

/// The main screen's pixels per point; 2 on a host with no screen or off the main thread (a unit test).
fn screen_scale() -> f64 {
    MainThreadMarker::new().and_then(NSScreen::mainScreen).map_or(2., |s| s.backingScaleFactor())
}

/// SF Symbol `name` at `size` points in `rgba` (sRGB, 0...1), at the screen's scale: the PNG's
/// bytes and its size in points, or None for a name the system lacks. The Swift app draws the same
/// symbols through SwiftUI's Image(systemName:).
pub fn symbol(name: &str, size: f64, rgba: [f64; 4]) -> Option<(Vec<u8>, f64, f64)> {
    let color = NSColor::colorWithSRGBRed_green_blue_alpha(rgba[0], rgba[1], rgba[2], rgba[3]);
    // SAFETY: NSFontWeightRegular is a constant AppKit defines.
    let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, unsafe { NSFontWeightRegular })
        .configurationByApplyingConfiguration(&NSImageSymbolConfiguration::configurationWithPaletteColors(&NSArray::from_retained_slice(&[color])));
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)?.imageWithSymbolConfiguration(&configuration)?;
    let points = image.size();
    let png = super::png(&image, screen_scale())?;
    Some((png, points.width, points.height))
}

/// The app's icon as a PNG, `size` points square at the screen's scale (Settings' server header).
pub fn app_icon(size: f64) -> Option<Vec<u8>> {
    let icon = NSApplication::sharedApplication(super::main_thread()).applicationIconImage()?;
    super::png_in(&icon, NSSize::new(size, size), screen_scale())
}

/// CoreImage's QR code for `text` (correction level M), one pixel per module, with its one-module quiet zone.
fn qr_image(text: &str) -> Option<Retained<CIImage>> {
    // SAFETY: CIQRCodeGenerator takes its message as NSData and its correction level as NSString,
    // the keys its documentation names.
    unsafe {
        let filter = CIFilter::filterWithName(&NSString::from_str("CIQRCodeGenerator"))?;
        filter.setValue_forKey(Some(&NSData::with_bytes(text.as_bytes())), &NSString::from_str("inputMessage"));
        filter.setValue_forKey(Some(&NSString::from_str("M")), &NSString::from_str("inputCorrectionLevel"));
        filter.outputImage()
    }
}

/// The modules of a QR code for `text`, top row first, "1" for dark and "0" for light, with the
/// quiet zone: what the Devices page and the pairing window draw as cells. Empty when CoreImage drew nothing.
pub fn qr_rows(text: &str) -> Vec<String> {
    let Some(image) = qr_image(text) else { return vec![] };
    let rep = objc2_app_kit::NSBitmapImageRep::initWithCIImage(objc2_app_kit::NSBitmapImageRep::alloc(), &image);
    let (w, h) = (rep.pixelsWide(), rep.pixelsHigh());
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| {
                    // colorAtX:y: counts y from the top row.
                    let dark = rep.colorAtX_y(x, y).and_then(|c| c.colorUsingColorSpace(&objc2_app_kit::NSColorSpace::genericGrayColorSpace())).is_some_and(|c| c.whiteComponent() < 0.5);
                    if dark { '1' } else { '0' }
                })
                .collect()
        })
        .collect()
}

/// Put the QR code for `text` on the general pasteboard as an image, each module scaled up to fill
/// 440 pixels so it stays sharp (the pairing window's Copy image). Returns whether it drew.
pub fn copy_qr_image(text: &str) -> bool {
    let Some(code) = qr_image(text) else { return false };
    // SAFETY: extent and imageByApplyingTransform: read and scale a finite generator output.
    let scaled = unsafe {
        let scale = (440. / code.extent().size.width).floor();
        code.imageByApplyingTransform(CGAffineTransform { a: scale, b: 0., c: 0., d: scale, tx: 0., ty: 0. })
    };
    let rep = NSCIImageRep::imageRepWithCIImage(&scaled);
    let image = NSImage::initWithSize(NSImage::alloc(), rep.size());
    image.addRepresentation(&rep);
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    board.writeObjects(&NSArray::from_retained_slice(&[ProtocolObject::from_retained(image)]))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_qr_code_reads_from_its_top_row() {
        let rows = super::qr_rows(r#"{"url":"wss://127.0.0.1:7430/ws"}"#);
        assert_eq!(rows.len(), 31);
        assert!(rows.iter().all(|r| r.len() == 31));
        assert!(rows[0].chars().all(|c| c == '0'), "the quiet zone");
        // Finder patterns sit top left, top right and bottom left; the bottom right has none.
        assert!(rows[1].starts_with("01111111") && rows[1].ends_with("11111110"), "{}", rows[1]);
        assert!(rows[29].starts_with("01111111") && !rows[29].ends_with("11111110"), "{}", rows[29]);
    }
}
