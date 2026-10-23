import CoreGraphics

/// Moving around a space's split tree from the keyboard: which pane is next, which is beside the
/// focused one, and which divider a resize moves. Pure, so the keys and the debug endpoint share it.
public enum PaneNavigation {
    /// The pane `by` steps from `pane` in the tree's order (left to right, top to bottom),
    /// wrapping at either end. Nil when the tree has no other pane.
    public static func cycle(from pane: String, in root: Node, by step: Int) -> String? {
        let ids = root.panes.map(\.id)
        guard ids.count > 1, let i = ids.firstIndex(of: pane) else { return nil }
        return ids[((i + step) % ids.count + ids.count) % ids.count]
    }

    /// The pane beside `pane` toward `dir`: of the panes wholly on that side that share some of
    /// its span across, the nearest, then the one whose middle is closest to its middle.
    public static func neighbor(of pane: String, in layout: GridLayout, toward dir: Dir) -> String? {
        guard let from = layout.panes.first(where: { $0.id == pane })?.frame else { return nil }
        // A pane beside another sits one gap away; anything nearer than half a gap past the edge counts.
        let slack = GridLayout.gap / 2
        func gap(_ to: CGRect) -> CGFloat? {
            switch dir {
            case .right: to.minX >= from.maxX - slack ? to.minX - from.maxX : nil
            case .left: to.maxX <= from.minX + slack ? from.minX - to.maxX : nil
            case .down: to.minY >= from.maxY - slack ? to.minY - from.maxY : nil
            case .up: to.maxY <= from.minY + slack ? from.minY - to.maxY : nil
            }
        }
        func across(_ to: CGRect) -> (overlap: CGFloat, offset: CGFloat) {
            switch dir {
            case .left, .right: (min(from.maxY, to.maxY) - max(from.minY, to.minY), abs(from.midY - to.midY))
            case .up, .down: (min(from.maxX, to.maxX) - max(from.minX, to.minX), abs(from.midX - to.midX))
            }
        }
        return layout.panes
            .filter { $0.id != pane }
            .compactMap { p -> (id: String, gap: CGFloat, offset: CGFloat)? in
                guard let g = gap(p.frame) else { return nil }
                let a = across(p.frame)
                return a.overlap > 0 ? (p.id, g, a.offset) : nil
            }
            .min { ($0.gap, $0.offset) < ($1.gap, $1.offset) }?
            .id
    }

    /// The divider a resize toward `dir` moves and the ratio it moves to: the nearest split above
    /// `pane` that lies across `dir`, its divider moved `step` that way. Nil when there is none or
    /// it is already as far as it goes.
    public static func resize(_ pane: String, in root: Node, toward dir: Dir, step: Double) -> (split: String, ratio: Double)? {
        let axis: Axis = dir == .left || dir == .right ? .horizontal : .vertical
        let sign = dir == .right || dir == .down ? 1.0 : -1.0
        guard let (split, ratio) = path(to: pane, in: root)?.last(where: { $0.axis == axis }).map({ ($0.id, $0.ratio) }) else { return nil }
        let moved = min(max(ratio + sign * step, GridLayout.ratios.lowerBound), GridLayout.ratios.upperBound)
        return abs(moved - ratio) < 0.0001 ? nil : (split, moved)
    }

    /// Every split whose ratio differs from the one that gives each pane along its axis the same
    /// share: a split's first side takes as many shares as it has panes side by side that way.
    public static func equalized(_ root: Node) -> [(split: String, ratio: Double)] {
        guard case let .split(id, axis, ratio, a, b) = root else { return [] }
        let first = Double(count(a, along: axis))
        let want = first / (first + Double(count(b, along: axis)))
        let own: [(split: String, ratio: Double)] = abs(want - ratio) < 0.0001 ? [] : [(id, want)]
        return own + equalized(a) + equalized(b)
    }

    /// How many panes `node` puts side by side along `axis`.
    static func count(_ node: Node, along axis: Axis) -> Int {
        switch node {
        case .pane: 1
        case let .split(_, own, _, a, b):
            own == axis ? count(a, along: axis) + count(b, along: axis) : max(count(a, along: axis), count(b, along: axis))
        }
    }

    /// The splits from the root down to `pane`, root first; nil when the tree does not hold it.
    static func path(to pane: String, in node: Node) -> [(id: String, axis: Axis, ratio: Double)]? {
        switch node {
        case let .pane(id, _): return id == pane ? [] : nil
        case let .split(id, axis, ratio, a, b):
            guard let rest = path(to: pane, in: a) ?? path(to: pane, in: b) else { return nil }
            return [(id, axis, ratio)] + rest
        }
    }
}
