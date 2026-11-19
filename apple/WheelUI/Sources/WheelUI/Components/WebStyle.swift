import SwiftUI

/// The page's style tokens: colours and sizes the components read. They default to the system's own
/// semantic colours, so a page follows light and dark and the person's accent colour like any native
/// window; a theme is another set of these values.
struct WebTheme {
    let bg, group, fill, separator, separatorStrong, label, secondary, tertiary: Color
    let red, orange, green, blue, purple, accentFill: Color

    static let radius: CGFloat = 10
    static let rowHeight: CGFloat = 28
    static let body: CGFloat = 13, callout: CGFloat = 12, caption: CGFloat = 11

    static let shared = WebTheme()

    private init() {
        #if os(macOS)
        bg = Color(nsColor: .windowBackgroundColor); group = Color(nsColor: .controlBackgroundColor)
        separator = Color(nsColor: .separatorColor); separatorStrong = Color(nsColor: .gridColor)
        tertiary = Color(nsColor: .tertiaryLabelColor)
        #else
        bg = Color(uiColor: .systemBackground); group = Color(uiColor: .secondarySystemBackground)
        separator = Color(uiColor: .separator); separatorStrong = Color(uiColor: .opaqueSeparator)
        tertiary = Color(uiColor: .tertiaryLabel)
        #endif
        fill = Color.secondary.opacity(0.12)
        label = .primary; secondary = .secondary
        red = .red; orange = .orange; green = .green; blue = .accentColor; purple = .purple
        accentFill = Color.accentColor.opacity(0.2)
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
