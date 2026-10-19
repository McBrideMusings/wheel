import CoreGraphics

/// Where a split tree puts each pane and divider inside a rect. Clients lay panes out flat from
/// this, keyed by pane id, so a split or a ratio change moves a pane's view instead of rebuilding it.
public struct GridLayout: Sendable {
    public struct Pane: Identifiable, Sendable {
        public let id: String
        public let kind: PaneKind
        public let createdBy: CreatedBy
        /// Who managed the pane before the person unlocked it.
        public let prior: CreatedBy?
        public let frame: CGRect
    }

    public struct Divider: Identifiable, Sendable {
        public let id: String
        public let axis: Axis
        /// The share of the split its first side takes, as drawn.
        public let ratio: Double
        /// The whole split's area, which a drag position is measured against.
        public let area: CGRect
        /// The gap between the two sides, which is what the person grabs.
        public let frame: CGRect
    }

    public static let gap: CGFloat = 8
    /// The ratios the daemon accepts.
    public static let ratios: ClosedRange<Double> = 0.1 ... 0.9

    public private(set) var panes: [Pane] = []
    public private(set) var dividers: [Divider] = []

    /// `ratios` overrides the tree's ratio for a split being dragged.
    public init(_ root: Node, in rect: CGRect, ratios: [String: Double] = [:]) {
        place(root, rect, ratios)
    }

    private mutating func place(_ node: Node, _ rect: CGRect, _ ratios: [String: Double]) {
        switch node {
        case let .pane(id, kind, createdBy, prior):
            panes.append(Pane(id: id, kind: kind, createdBy: createdBy, prior: prior, frame: rect))
        case let .split(id, axis, ratio, a, b):
            let ratio = ratios[id] ?? ratio
            let gap = Self.gap
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
            dividers.append(Divider(id: id, axis: axis, ratio: ratio, area: rect, frame: gapRect))
        }
    }

    /// The ratio a drag to `point` asks for, kept inside what the daemon accepts.
    public static func ratio(at point: CGPoint, in divider: Divider) -> Double {
        let r = switch divider.axis {
        case .horizontal: (point.x - divider.area.minX - gap / 2) / max(divider.area.width - gap, 1)
        case .vertical: (point.y - divider.area.minY - gap / 2) / max(divider.area.height - gap, 1)
        }
        return min(max(Double(r), ratios.lowerBound), ratios.upperBound)
    }
}
