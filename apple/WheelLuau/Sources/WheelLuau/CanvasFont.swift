import CoreText
import Foundation
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// A Canvas 2D `font` string as a pane script writes it (`"bold 13px sans-serif"`): the subset of the CSS font
/// shorthand `[italic] [weight] <size>px [family]`. A weight is `normal`, `bold` or 100 to 900; a family containing
/// `mono` is the monospaced system font, `serif` the serif one, `rounded` the rounded one, anything else the system
/// font. A string that names no size keeps the default size, 10px, as Canvas 2D does.
public struct CanvasFont: Equatable, Sendable {
    public enum Design: String, Sendable { case `default`, monospaced, serif, rounded }

    public var size: Double = 10
    /// 100 to 900, CSS's scale: 400 regular, 700 bold.
    public var weight: Int = 400
    public var italic = false
    public var design: Design = .default

    public init(size: Double = 10, weight: Int = 400, italic: Bool = false, design: Design = .default) {
        self.size = size
        self.weight = weight
        self.italic = italic
        self.design = design
    }

    public init(_ spec: String) {
        var family: [String] = []
        for word in spec.lowercased().split(whereSeparator: { $0 == " " || $0 == "," }).map(String.init) {
            if word == "italic" || word == "oblique" { italic = true }
            else if word == "normal" { continue }
            else if word == "bold" || word == "bolder" { weight = 700 }
            else if word == "lighter" { weight = 300 }
            else if let w = Int(word), (1...1000).contains(w) { weight = w }
            else if word.hasSuffix("px"), let s = Double(word.dropLast(2)), s > 0 { size = min(s, 1000) }
            else { family.append(word) }
        }
        let name = family.joined(separator: " ")
        if name.contains("mono") { design = .monospaced }
        else if name.contains("rounded") { design = .rounded }
        else if family.contains("serif") || name.contains("times") || name.contains("georgia") { design = .serif }
    }

    /// The weight as one of the nine system weights, by CSS's rounding.
    public var weightName: String {
        switch weight {
        case ..<150: "ultraLight"
        case ..<250: "thin"
        case ..<350: "light"
        case ..<450: "regular"
        case ..<550: "medium"
        case ..<650: "semibold"
        case ..<750: "bold"
        case ..<850: "heavy"
        default: "black"
        }
    }

    #if os(macOS)
    private typealias PlatformFont = NSFont
    private typealias PlatformWeight = NSFont.Weight
    #else
    private typealias PlatformFont = UIFont
    private typealias PlatformWeight = UIFont.Weight
    #endif

    private var platformWeight: PlatformWeight {
        switch weightName {
        case "ultraLight": .ultraLight
        case "thin": .thin
        case "light": .light
        case "medium": .medium
        case "semibold": .semibold
        case "bold": .bold
        case "heavy": .heavy
        case "black": .black
        default: .regular
        }
    }

    /// The system font this names: the same face SwiftUI's `Font.system(size:weight:design:)` draws.
    public var ctFont: CTFont {
        let s = CGFloat(size)
        var font: PlatformFont = design == .monospaced
            ? .monospacedSystemFont(ofSize: s, weight: platformWeight)
            : .systemFont(ofSize: s, weight: platformWeight)
        var descriptor = font.fontDescriptor
        #if os(macOS)
        if design == .serif, let d = descriptor.withDesign(.serif) { descriptor = d }
        if design == .rounded, let d = descriptor.withDesign(.rounded) { descriptor = d }
        if italic { descriptor = descriptor.withSymbolicTraits(.italic) }
        font = NSFont(descriptor: descriptor, size: s) ?? font
        #else
        if design == .serif, let d = descriptor.withDesign(.serif) { descriptor = d }
        if design == .rounded, let d = descriptor.withDesign(.rounded) { descriptor = d }
        if italic, let d = descriptor.withSymbolicTraits(.traitItalic) { descriptor = d }
        font = UIFont(descriptor: descriptor, size: s)
        #endif
        return font as CTFont
    }

    /// What Canvas 2D's `measureText` gives: the advance width of `text` set in this font, and the font's ascent and
    /// descent above and below the alphabetic baseline, in points.
    public func measure(_ text: String) -> (width: Double, ascent: Double, descent: Double) {
        let font = ctFont
        let attributed = NSAttributedString(string: text, attributes: [NSAttributedString.Key(kCTFontAttributeName as String): font])
        let line = CTLineCreateWithAttributedString(attributed)
        let width = CTLineGetTypographicBounds(line, nil, nil, nil)
        return (width, Double(CTFontGetAscent(font)), Double(CTFontGetDescent(font)))
    }
}
