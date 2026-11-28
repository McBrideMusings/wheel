//! A Canvas 2D `font` string as a pane script writes it (`"bold 13px sans-serif"`), and what
//! `measureText` reads from it.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSFont, NSFontAttributeName, NSFontDescriptorSymbolicTraits};
use objc2_core_foundation::CFAttributedString;
use objc2_core_text::{CTFont, CTLine};
use objc2_foundation::{NSAttributedString, NSDictionary, NSString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Design {
    Default,
    Monospaced,
    Serif,
    Rounded,
}

/// The subset of the CSS font shorthand `[italic] [weight] <size>px [family]`. A weight is
/// `normal`, `bold` or 100 to 900; a family containing `mono` is the monospaced system font,
/// `serif` the serif one, `rounded` the rounded one, anything else the system font. A string
/// that names no size keeps the default size, 10px, as Canvas 2D does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasFont {
    pub size: f64,
    /// 100 to 900, CSS's scale: 400 regular, 700 bold.
    pub weight: i32,
    pub italic: bool,
    pub design: Design,
}

impl Default for CanvasFont {
    fn default() -> Self {
        Self { size: 10.0, weight: 400, italic: false, design: Design::Default }
    }
}

/// What Canvas 2D's `measureText` gives, in points: the advance width, and the font's ascent and
/// descent above and below the alphabetic baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub width: f64,
    pub ascent: f64,
    pub descent: f64,
}

impl CanvasFont {
    pub fn parse(spec: &str) -> Self {
        let mut font = Self::default();
        let mut family: Vec<String> = Vec::new();
        let lower = spec.to_lowercase();
        for word in lower.split([' ', ',']).filter(|w| !w.is_empty()) {
            if word == "italic" || word == "oblique" {
                font.italic = true;
            } else if word == "normal" {
                continue;
            } else if word == "bold" || word == "bolder" {
                font.weight = 700;
            } else if word == "lighter" {
                font.weight = 300;
            } else if let Some(w) = word.parse::<i32>().ok().filter(|w| (1..=1000).contains(w)) {
                font.weight = w;
            } else if let Some(s) = word.strip_suffix("px").and_then(|s| s.parse::<f64>().ok()).filter(|s| *s > 0.0) {
                font.size = s.min(1000.0);
            } else {
                family.push(word.to_string());
            }
        }
        let name = family.join(" ");
        if name.contains("mono") {
            font.design = Design::Monospaced;
        } else if name.contains("rounded") {
            font.design = Design::Rounded;
        } else if family.iter().any(|f| f == "serif") || name.contains("times") || name.contains("georgia") {
            font.design = Design::Serif;
        }
        font
    }

    /// The weight as one of the nine system weights, by CSS's rounding.
    pub fn weight_name(&self) -> &'static str {
        match self.weight {
            ..150 => "ultraLight",
            150..250 => "thin",
            250..350 => "light",
            350..450 => "regular",
            450..550 => "medium",
            550..650 => "semibold",
            650..750 => "bold",
            750..850 => "heavy",
            _ => "black",
        }
    }

    fn system_weight(&self) -> f64 {
        // SAFETY: AppKit's weight constants are immutable statics.
        unsafe {
            use objc2_app_kit::*;
            match self.weight_name() {
                "ultraLight" => NSFontWeightUltraLight,
                "thin" => NSFontWeightThin,
                "light" => NSFontWeightLight,
                "medium" => NSFontWeightMedium,
                "semibold" => NSFontWeightSemibold,
                "bold" => NSFontWeightBold,
                "heavy" => NSFontWeightHeavy,
                "black" => NSFontWeightBlack,
                _ => NSFontWeightRegular,
            }
        }
    }

    /// The system font this names: the face SwiftUI's `Font.system(size:weight:design:)` draws.
    pub fn ns_font(&self) -> Retained<NSFont> {
        let weight = self.system_weight();
        let font = if self.design == Design::Monospaced {
            NSFont::monospacedSystemFontOfSize_weight(self.size, weight)
        } else {
            NSFont::systemFontOfSize_weight(self.size, weight)
        };
        let mut descriptor = font.fontDescriptor();
        // SAFETY: the design names are immutable statics.
        let design = unsafe {
            match self.design {
                Design::Serif => Some(objc2_app_kit::NSFontDescriptorSystemDesignSerif),
                Design::Rounded => Some(objc2_app_kit::NSFontDescriptorSystemDesignRounded),
                _ => None,
            }
        };
        if let Some(d) = design.and_then(|d| descriptor.fontDescriptorWithDesign(d)) {
            descriptor = d;
        }
        if self.italic {
            descriptor = descriptor.fontDescriptorWithSymbolicTraits(NSFontDescriptorSymbolicTraits::TraitItalic);
        }
        NSFont::fontWithDescriptor_size(&descriptor, self.size).unwrap_or(font)
    }

    pub fn measure(&self, text: &str) -> Metrics {
        let font = self.ns_font();
        // SAFETY: NSFont is toll-free bridged with CTFont, NSAttributedString with
        // CFAttributedString, and NSFontAttributeName is kCTFontAttributeName.
        unsafe {
            let key: &NSString = NSFontAttributeName;
            let value: &AnyObject = &font;
            let attributes = NSDictionary::from_slices(&[key], &[value]);
            let string = NSAttributedString::new_with_attributes(&NSString::from_str(text), &attributes);
            let cf = &*(Retained::as_ptr(&string) as *const CFAttributedString);
            let line = CTLine::with_attributed_string(cf);
            let width = line.typographic_bounds(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            let ct = &*(Retained::as_ptr(&font) as *const CTFont);
            Metrics { width, ascent: ct.ascent(), descent: ct.descent() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_css_shorthand() {
        let f = |size, weight, italic, design| CanvasFont { size, weight, italic, design };
        assert_eq!(CanvasFont::parse("bold 13px sans-serif"), f(13.0, 700, false, Design::Default));
        assert_eq!(CanvasFont::parse("600 11px monospace"), f(11.0, 600, false, Design::Monospaced));
        assert_eq!(CanvasFont::parse("italic 12px serif"), f(12.0, 400, true, Design::Serif));
        assert_eq!(CanvasFont::parse(""), CanvasFont::default());
    }
}
