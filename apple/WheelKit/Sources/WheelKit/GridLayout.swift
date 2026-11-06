import CoreGraphics

/// Where a split tree puts each pane and divider inside a rect. Clients lay panes out flat from
/// this, keyed by pane id, so a split or a ratio change moves a pane's view instead of rebuilding it.
public struct GridLayout: Sendable {
    public struct Pane: Identifiable, Sendable {
        public let id: String
        public let kind: PaneKind
        public let frame: CGRect
    }

    public struct Divider: Identifiable, Sendable {
        public let id: String
        public let axis: Axis
        /// The share of the split its first side takes, as drawn.
        public let ratio: Double
        /// The ratios a drag may reach: neither side drops below the minimum pane size.
        public let range: ClosedRange<Double>
        /// The whole split's area, which a drag position is measured against.
        public let area: CGRect
        /// The gap between the two sides, which is what the person grabs.
        public let frame: CGRect
    }

    public static let gap: CGFloat = 8
    /// The ratios the daemon accepts.
    public static let ratios: ClosedRange<Double> = 0.1 ... 0.9
    /// The smallest a pane is drawn while the area has room for it: below this its header and
    /// body no longer fit, so a divider stops here. An area too small for every pane's minimum
    /// shares its size among them in proportion to what each needs.
    public static let minimumPane = CGSize(width: 240, height: 160)

    public private(set) var panes: [Pane] = []
    public private(set) var dividers: [Divider] = []

    private let minimum: CGSize

    /// `ratios` overrides the tree's ratio for a split being dragged. `minimum` is the least a
    /// pane may get; a preview that draws the tree small leaves it zero.
    public init(_ root: Node, in rect: CGRect, ratios: [String: Double] = [:], minimum: CGSize = .zero) {
        self.minimum = minimum
        place(root, rect, ratios)
    }

    /// The size a subtree needs for every pane in it to get the minimum.
    private func need(_ node: Node) -> CGSize {
        switch node {
        case .pane: return minimum
        case let .split(_, axis, _, a, b):
            let na = need(a), nb = need(b)
            return switch axis {
            case .horizontal: CGSize(width: na.width + Self.gap + nb.width, height: max(na.height, nb.height))
            case .vertical: CGSize(width: max(na.width, nb.width), height: na.height + Self.gap + nb.height)
            }
        }
    }

    /// The ratios that leave each side its need out of `total`, or the single ratio that shares
    /// `total` in proportion to the needs when they do not both fit.
    private static func bounds(total: CGFloat, needA: CGFloat, needB: CGFloat) -> ClosedRange<Double> {
        guard total > 0, needA + needB > 0 else { return ratios }
        guard needA + needB <= total else {
            let shared = Double(needA / (needA + needB))
            return shared ... shared
        }
        // The needs fit, so `lo <= hi` on paper; rounding can put `hi` one step under `lo` when they fit exactly.
        let lo = Double(needA / total), hi = max(Double(1 - needB / total), lo)
        let (lo2, hi2) = (max(lo, ratios.lowerBound), min(hi, ratios.upperBound))
        return lo2 <= hi2 ? lo2 ... hi2 : lo ... hi
    }

    private mutating func place(_ node: Node, _ rect: CGRect, _ ratios: [String: Double]) {
        switch node {
        case let .pane(id, kind):
            panes.append(Pane(id: id, kind: kind, frame: rect))
        case let .split(id, axis, ratio, a, b):
            let gap = Self.gap
            let range = Self.bounds(total: max(axis == .horizontal ? rect.width - gap : rect.height - gap, 0),
                                    needA: axis == .horizontal ? need(a).width : need(a).height,
                                    needB: axis == .horizontal ? need(b).width : need(b).height)
            let ratio = min(max(ratios[id] ?? ratio, range.lowerBound), range.upperBound)
            let (ra, gapRect, rb): (CGRect, CGRect, CGRect)
            switch axis {
            case .horizontal:
                let w = max(rect.width - gap, 0) * ratio
                ra = CGRect(x: rect.minX, y: rect.minY, width: w, height: rect.height)
                gapRect = CGRect(x: rect.minX + w, y: rect.minY, width: gap, height: rect.height)
                rb = CGRect(x: rect.minX + w + gap, y: rect.minY, width: max(rect.width - w - gap, 0), height: rect.height)
            case .vertical:
                let h = max(rect.height - gap, 0) * ratio
                ra = CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: h)
                gapRect = CGRect(x: rect.minX, y: rect.minY + h, width: rect.width, height: gap)
                rb = CGRect(x: rect.minX, y: rect.minY + h + gap, width: rect.width, height: max(rect.height - h - gap, 0))
            }
            place(a, ra, ratios)
            place(b, rb, ratios)
            dividers.append(Divider(id: id, axis: axis, ratio: ratio, range: range, area: rect, frame: gapRect))
        }
    }

    /// The ratio a drag to `point` asks for, kept inside what the daemon accepts.
    public static func ratio(at point: CGPoint, in divider: Divider) -> Double {
        let r = switch divider.axis {
        case .horizontal: (point.x - divider.area.minX - gap / 2) / max(divider.area.width - gap, 1)
        case .vertical: (point.y - divider.area.minY - gap / 2) / max(divider.area.height - gap, 1)
        }
        return min(max(Double(r), divider.range.lowerBound), divider.range.upperBound)
    }
}
