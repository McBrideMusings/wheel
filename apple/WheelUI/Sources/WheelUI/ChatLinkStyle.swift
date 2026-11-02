import Foundation
import SwiftUI
import Textual
import WheelKit
#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif

/// How a link in a reply is drawn: underlined or not, in the system link colour, the reply's text
/// colour or the accent colour. A colour that reads below 4.5 to 1 against the chat ground is
/// darkened (light) or lightened (dark) until it reaches it. `InlineStyle.chat` and the debug
/// `/state` both read this, so the report names what the screen is told to draw.
public enum ChatLinkStyle {
    public static let minimumContrast = 4.5

    public struct RGB: Equatable, Sendable {
        public var r: Double, g: Double, b: Double
        public var hex: String {
            func byte(_ v: Double) -> String { String(format: "%02x", Int((v * 255).rounded())) }
            return "#" + byte(r) + byte(g) + byte(b)
        }
        /// WCAG relative luminance.
        var luminance: Double {
            func lin(_ v: Double) -> Double { v <= 0.03928 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4) }
            return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
        }
        func mixed(toward t: Double, by f: Double) -> RGB { RGB(r: r + (t - r) * f, g: g + (t - g) * f, b: b + (t - b) * f) }
    }

    public static func contrast(_ a: RGB, _ b: RGB) -> Double {
        let (hi, lo) = (max(a.luminance, b.luminance), min(a.luminance, b.luminance))
        return (hi + 0.05) / (lo + 0.05)
    }

    /// `color` moved toward black on a light ground, white on a dark one, until it reads at
    /// `minimumContrast`; unchanged when it already does.
    static func readable(_ color: RGB, on ground: RGB, dark: Bool) -> RGB {
        var out = color
        var step = 0.0
        while contrast(out, ground) < minimumContrast, step < 1 {
            step += 0.02
            out = color.mixed(toward: dark ? 1 : 0, by: step)
        }
        return out
    }

    /// The colour a link of `kind` takes in light or dark appearance, and the ground it is read against.
    public static func resolved(_ kind: ChatLinkSettings.LinkColor, dark: Bool) -> (color: RGB, ground: RGB) {
        let ground = resolve(groundSource, dark: dark)
        let source: RGB
        switch kind {
        case .link: source = resolve(linkSource, dark: dark)
        case .accent: source = resolve(accentSource, dark: dark)
        case .text: source = resolve(textSource, dark: dark)
        }
        return (readable(source, on: ground, dark: dark), ground)
    }

    /// The colour of each kind as a SwiftUI colour that follows the appearance it is drawn in,
    /// made once: a reply's style asks for it on every body build.
    private static let colors: [ChatLinkSettings.LinkColor: Color] =
        Dictionary(uniqueKeysWithValues: ChatLinkSettings.LinkColor.allCases.map { ($0, make($0)) })

    static func color(_ kind: ChatLinkSettings.LinkColor) -> Color { colors[kind] ?? make(kind) }

    private static func make(_ kind: ChatLinkSettings.LinkColor) -> Color {
        let light = resolved(kind, dark: false).color, dark = resolved(kind, dark: true).color
        #if canImport(AppKit)
        return Color(nsColor: NSColor(name: nil) { appearance in
            let rgb = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: rgb.r, green: rgb.g, blue: rgb.b, alpha: 1)
        })
        #else
        return Color(uiColor: UIColor { traits in
            let rgb = traits.userInterfaceStyle == .dark ? dark : light
            return UIColor(red: rgb.r, green: rgb.g, blue: rgb.b, alpha: 1)
        })
        #endif
    }

    #if canImport(AppKit)
    private static let linkSource = NSColor.linkColor
    private static let accentSource = NSColor.controlAccentColor
    private static let textSource = NSColor.labelColor
    private static let groundSource = NSColor.windowBackgroundColor
    private static func resolve(_ color: NSColor, dark: Bool) -> RGB {
        var out = RGB(r: 0, g: 0, b: 0)
        NSAppearance(named: dark ? .darkAqua : .aqua)?.performAsCurrentDrawingAppearance {
            if let c = color.usingColorSpace(.sRGB) { out = RGB(r: c.redComponent, g: c.greenComponent, b: c.blueComponent) }
        }
        return out
    }
    #else
    private static let linkSource = UIColor.link
    private static let accentSource = UIColor.tintColor
    private static let textSource = UIColor.label
    private static let groundSource = UIColor.systemBackground
    private static func resolve(_ color: UIColor, dark: Bool) -> RGB {
        let c = color.resolvedColor(with: UITraitCollection(userInterfaceStyle: dark ? .dark : .light))
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        c.getRed(&r, green: &g, blue: &b, alpha: &a)
        return RGB(r: r, g: g, b: b)
    }
    #endif

    /// What the debug `/state` reports for a reply's Markdown: every link run's text, address,
    /// kind, underline and colour in light and dark, then each colour's contrast against the ground.
    public static func report(of replies: [String], look: ChatAppearance) -> [String: Any] {
        let settings = look.links
        var runs: [[String: Any]] = []
        for reply in replies {
            guard let text = try? AttributedString(markdown: ChatRich.prepare(reply).markdown) else { continue }
            for run in text.runs {
                guard let url = run.link else { continue }
                let kind: String
                switch ChatLinkAction(url: url) {
                case .file: kind = "file"
                case .pane: kind = "pane"
                case .artifact: kind = "artifact"
                case .web: kind = "web"
                case .run, .copy: kind = "pattern"
                case .loadImage: kind = "image"
                case nil: kind = ChatRich.timestamp(of: url) != nil ? "time" : "other"
                }
                runs.append([
                    "text": String(text[run.range].characters), "url": url.absoluteString, "kind": kind,
                    "underline": settings.underline, "color": settings.color.rawValue,
                    "light": resolved(settings.color, dark: false).color.hex,
                    "dark": resolved(settings.color, dark: true).color.hex,
                ])
            }
        }
        var ratios: [[String: Any]] = []
        for kind in ChatLinkSettings.LinkColor.allCases {
            for dark in [false, true] {
                let r = resolved(kind, dark: dark)
                ratios.append(["color": kind.rawValue, "mode": dark ? "dark" : "light", "hex": r.color.hex,
                               "ground": r.ground.hex, "ratio": Double(String(format: "%.2f", contrast(r.color, r.ground))) ?? 0])
            }
        }
        return ["underline": settings.underline, "color": settings.color.rawValue, "runs": runs, "contrast": ratios]
    }
}

extension InlineStyle {
    /// This style with links drawn the way `look.links` says.
    func linked(_ look: ChatAppearance) -> InlineStyle {
        let color = ChatLinkStyle.color(look.links.color)
        return look.links.underline
            ? link(.foregroundColor(color), .underlineStyle(.single))
            : link(.foregroundColor(color))
    }
}
