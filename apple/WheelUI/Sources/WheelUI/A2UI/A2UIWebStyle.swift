import SwiftUI

/// The look of Wheel's web artifacts (`daemon/src/wheel.css`), as colours and sizes, so a native
/// page draws what the same page draws in a web pane.
struct WebTheme {
    let bg, group, fill, separator, separatorStrong, label, secondary, tertiary: Color
    let red, orange, green, blue, purple, accentFill: Color

    static let radius: CGFloat = 10
    static let rowHeight: CGFloat = 28
    static let body: CGFloat = 13, callout: CGFloat = 12, caption: CGFloat = 11

    static let shared = WebTheme()

    private init() {
        bg = Self.adaptive(light: (0xf5, 0xf5, 0xf7, 1), dark: (0x1c, 0x1c, 0x1e, 1))
        group = Self.adaptive(light: (0xff, 0xff, 0xff, 1), dark: (0x2a, 0x2a, 0x2c, 1))
        fill = Self.adaptive(light: (0, 0, 0, 0.05), dark: (255, 255, 255, 0.06))
        separator = Self.adaptive(light: (0, 0, 0, 0.1), dark: (255, 255, 255, 0.1))
        separatorStrong = Self.adaptive(light: (0, 0, 0, 0.25), dark: (255, 255, 255, 0.25))
        label = Self.adaptive(light: (0, 0, 0, 0.85), dark: (255, 255, 255, 0.88))
        secondary = Self.adaptive(light: (0, 0, 0, 0.55), dark: (255, 255, 255, 0.56))
        tertiary = Self.adaptive(light: (0, 0, 0, 0.3), dark: (255, 255, 255, 0.28))
        red = Self.adaptive(light: (0xe0, 0x35, 0x2b, 1), dark: (0xff, 0x45, 0x3a, 1))
        orange = Self.adaptive(light: (0xe6, 0x7e, 0x00, 1), dark: (0xff, 0x9f, 0x0a, 1))
        green = Self.adaptive(light: (0x24, 0x8a, 0x3d, 1), dark: (0x30, 0xd1, 0x58, 1))
        blue = Self.adaptive(light: (0x00, 0x7a, 0xff, 1), dark: (0x0a, 0x84, 0xff, 1))
        purple = Self.adaptive(light: (0xa5, 0x50, 0xd6, 1), dark: (0xbf, 0x5a, 0xf2, 1))
        accentFill = Self.adaptive(light: (0x00, 0x7a, 0xff, 0.12), dark: (0x0a, 0x84, 0xff, 0.2))
    }

    typealias RGBA = (Double, Double, Double, Double)

    private static func adaptive(light: RGBA, dark: RGBA) -> Color {
        func color(_ v: RGBA) -> (Double, Double, Double, Double) { (v.0 / 255, v.1 / 255, v.2 / 255, v.3) }
        let l = color(light), d = color(dark)
        #if os(macOS)
        return Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            let c = isDark ? d : l
            return NSColor(srgbRed: c.0, green: c.1, blue: c.2, alpha: c.3)
        })
        #else
        return Color(uiColor: UIColor { traits in
            let c = traits.userInterfaceStyle == .dark ? d : l
            return UIColor(red: c.0, green: c.1, blue: c.2, alpha: c.3)
        })
        #endif
    }

    /// The colour a status word or tone name stands for (`ok`, `warn`, `bad`, `muted`); nil is the label colour.
    func tone(_ name: String?) -> Color? {
        switch name {
        case "ok": green
        case "warn": orange
        case "bad": red
        case "muted": secondary
        case "faint": tertiary
        default: nil
        }
    }
}

/// What a child asks of a `FlowLayout`: take the rest of its line, or sit at the line's right end.
struct FlowGrow: LayoutValueKey { static let defaultValue = false }
struct FlowTrail: LayoutValueKey { static let defaultValue = false }

/// Children in lines that wrap at the proposed width, like a flex row with `flex-wrap: wrap`.
/// A growing child takes what its line has left; a trailing child is pushed to the line's right
/// end (`margin-left: auto`); with `between` the first and last child of a line sit at its two ends
/// (`justify-content: space-between`).
struct FlowLayout: Layout {
    var spacing: CGFloat
    var rowSpacing: CGFloat
    var between = false

    private struct Line { var items: [Int] = []; var width: CGFloat = 0; var height: CGFloat = 0 }

    private func lines(_ subviews: Subviews, in width: CGFloat) -> ([Line], [CGSize]) {
        let sizes = subviews.map { $0.sizeThatFits(.unspecified) }
        var out: [Line] = []
        var cur = Line()
        for i in subviews.indices {
            let w = min(sizes[i].width, width)
            let add = (cur.items.isEmpty ? 0 : spacing) + w
            if !cur.items.isEmpty, cur.width + add > width + 0.5 {
                out.append(cur)
                cur = Line()
            }
            cur.width += (cur.items.isEmpty ? 0 : spacing) + w
            cur.height = max(cur.height, sizes[i].height)
            cur.items.append(i)
        }
        if !cur.items.isEmpty { out.append(cur) }
        return (out, sizes)
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache _: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        let (ls, _) = lines(subviews, in: width)
        let h = ls.reduce(0) { $0 + $1.height } + rowSpacing * CGFloat(max(ls.count - 1, 0))
        let w = proposal.width ?? (ls.map(\.width).max() ?? 0)
        return CGSize(width: w, height: h)
    }

    func placeSubviews(in bounds: CGRect, proposal _: ProposedViewSize, subviews: Subviews, cache _: inout ()) {
        let (ls, sizes) = lines(subviews, in: bounds.width)
        var y = bounds.minY
        for line in ls {
            let free = max(bounds.width - line.width, 0)
            let growers = line.items.filter { subviews[$0][FlowGrow.self] }
            let grow = growers.isEmpty ? 0 : free / CGFloat(growers.count)
            var x = bounds.minX
            var pushed = false
            for (n, i) in line.items.enumerated() {
                var w = min(sizes[i].width, bounds.width)
                if subviews[i][FlowGrow.self] { w += grow }
                let apart = between && n == line.items.count - 1 && n > 0
                if growers.isEmpty, !pushed, subviews[i][FlowTrail.self] || apart {
                    x += free
                    pushed = true
                }
                subviews[i].place(at: CGPoint(x: x, y: y + (line.height - sizes[i].height) / 2), anchor: .topLeading,
                                  proposal: ProposedViewSize(width: w, height: sizes[i].height))
                x += w + spacing
            }
            y += line.height + rowSpacing
        }
    }
}
