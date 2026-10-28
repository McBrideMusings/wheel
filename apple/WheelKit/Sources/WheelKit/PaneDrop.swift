import CoreGraphics

/// Where a dragged pane lands: the middle of a pane swaps the two, near an edge docks the dragged
/// pane on that side of the target. The client only previews this; the daemon owns the tree and
/// `pane_swap` / `pane_move` make it so.
public enum DropZone: Sendable, Equatable {
    case center
    case edge(Dir)

    /// `center`, or the edge's side: `left`, `right`, `up` or `down`.
    public var label: String {
        switch self {
        case .center: "center"
        case let .edge(dir): dir.rawValue
        }
    }
}

public struct PaneDrop: Sendable, Equatable {
    public let target: String
    public let zone: DropZone

    public init(target: String, zone: DropZone) {
        self.target = target
        self.zone = zone
    }
}

/// Where a dragged pane lands when it leaves its own space: the pointer is over another space's
/// pane (the pane takes one side of it), a tab, row or popover row (the end of that space), the
/// empty part of a tab strip or sidebar (a new space in that window), or the desktop (a new space in
/// a little window). The daemon moves the pane; the client only previews it.
public enum PaneAway: Sendable, Equatable {
    case beside(pane: String, dir: Dir, space: String)
    case end(space: String)
    case newSpace(window: String)
    case desktop

    /// What releasing does, in words, for the debug state.
    public func hint(pane: String, spaceName: (String) -> String?, windowTitle: (String) -> String) -> String {
        switch self {
        case let .beside(target, dir, space): "Dock “\(pane)” on the \(dir.rawValue) of \(target) in “\(spaceName(space) ?? space)”"
        case let .end(space): "Move “\(pane)” to the end of “\(spaceName(space) ?? space)”"
        case let .newSpace(window): "Move “\(pane)” into a new space in “\(windowTitle(window))”"
        case .desktop: "Release to move “\(pane)” into a new space in a little window"
        }
    }
}

public enum PaneDragging {
    /// How long a pane must be held over another space's tab before that space shows.
    public static let holdSeconds: Double = 0.55

    /// The side of a pane at `frame` that `point` is nearest: a pane dropped in another space always
    /// takes a side, never swaps.
    public static func nearestEdge(at point: CGPoint, in frame: CGRect) -> Dir {
        guard frame.width > 0, frame.height > 0 else { return .right }
        let fx = (point.x - frame.minX) / frame.width
        let fy = (point.y - frame.minY) / frame.height
        let near: [(CGFloat, Dir)] = [(fx, .left), (1 - fx, .right), (fy, .up), (1 - fy, .down)]
        return near.min(by: { $0.0 < $1.0 })?.1 ?? .right
    }

    /// The pane at `point` and the side it would take, for a pane arriving from another space.
    public static func away(at point: CGPoint, among panes: [GridLayout.Pane]) -> (pane: String, dir: Dir)? {
        guard let pane = panes.first(where: { $0.frame.contains(point) }) else { return nil }
        return (pane.id, nearestEdge(at: point, in: pane.frame))
    }

    /// How much of a pane, measured in from each edge, counts as that edge's drop zone.
    public static let edgeShare: CGFloat = 0.25

    /// The zone of a pane at `frame` that `point` is in; the nearest edge when it is in two.
    public static func zone(at point: CGPoint, in frame: CGRect) -> DropZone {
        guard frame.width > 0, frame.height > 0 else { return .center }
        let fx = (point.x - frame.minX) / frame.width
        let fy = (point.y - frame.minY) / frame.height
        let near: [(CGFloat, Dir)] = [(fx, .left), (1 - fx, .right), (fy, .up), (1 - fy, .down)]
        guard let best = near.min(by: { $0.0 < $1.0 }), best.0 < edgeShare else { return .center }
        return .edge(best.1)
    }

    /// The drop under `point`: the pane holding it, other than the one being dragged. Nil outside
    /// every pane and over the dragged pane itself, where releasing cancels.
    public static func drop(at point: CGPoint, among panes: [GridLayout.Pane], dragging: String) -> PaneDrop? {
        guard let pane = panes.first(where: { $0.frame.contains(point) }), pane.id != dragging else { return nil }
        return PaneDrop(target: pane.id, zone: zone(at: point, in: pane.frame))
    }

    /// The part of the target's frame that lights up for `zone`.
    public static func highlight(_ zone: DropZone, in frame: CGRect) -> CGRect {
        let w = frame.width * 0.5, h = frame.height * 0.5
        switch zone {
        case .center: return frame
        case .edge(.left): return CGRect(x: frame.minX, y: frame.minY, width: w, height: frame.height)
        case .edge(.right): return CGRect(x: frame.maxX - w, y: frame.minY, width: w, height: frame.height)
        case .edge(.up): return CGRect(x: frame.minX, y: frame.minY, width: frame.width, height: h)
        case .edge(.down): return CGRect(x: frame.minX, y: frame.maxY - h, width: frame.width, height: h)
        }
    }
}

extension Node {
    /// The tree as it would stand after dragging `pane` onto `drop`, to preview. It mirrors the
    /// daemon's `swap` and `move_pane` (daemon/src/spaces.rs); the daemon's tree is the one that
    /// counts. A drop that changes nothing returns the tree itself.
    public func applying(_ drop: PaneDrop, dragging pane: String) -> Node {
        guard pane != drop.target, leaf(pane) != nil, leaf(drop.target) != nil else { return self }
        switch drop.zone {
        case .center:
            return swapping(pane, drop.target)
        case let .edge(dir):
            guard let leaf = leaf(pane), let rest = removing(pane) else { return self }
            return rest.inserting(leaf, beside: drop.target, dir)
        }
    }

    private func leaf(_ pane: String) -> Node? {
        switch self {
        case .pane(let id, _): id == pane ? self : nil
        case let .split(_, _, _, a, b): a.leaf(pane) ?? b.leaf(pane)
        }
    }

    private func swapping(_ x: String, _ y: String) -> Node {
        guard let lx = leaf(x), let ly = leaf(y) else { return self }
        return mapLeaves { $0.id == x ? ly : $0.id == y ? lx : $0 }
    }

    private func mapLeaves(_ f: (Node) -> Node) -> Node {
        switch self {
        case .pane: f(self)
        case let .split(id, axis, ratio, a, b): .split(id: id, axis: axis, ratio: ratio, a: a.mapLeaves(f), b: b.mapLeaves(f))
        }
    }

    /// The tree without `pane`, its sibling taking the split's place; nil when it was the whole tree.
    private func removing(_ pane: String) -> Node? {
        switch self {
        case let .pane(id, _):
            return id == pane ? nil : self
        case let .split(id, axis, ratio, a, b):
            if a.leaf(pane) != nil {
                guard let rest = a.removing(pane) else { return b }
                return .split(id: id, axis: axis, ratio: ratio, a: rest, b: b)
            }
            guard let rest = b.removing(pane) else { return a }
            return .split(id: id, axis: axis, ratio: ratio, a: a, b: rest)
        }
    }

    private func inserting(_ leaf: Node, beside target: String, _ dir: Dir) -> Node {
        switch self {
        case let .pane(id, _) where id == target:
            let (axis, a, b): (Axis, Node, Node) = switch dir {
            case .right: (.horizontal, self, leaf)
            case .left: (.horizontal, leaf, self)
            case .down: (.vertical, self, leaf)
            case .up: (.vertical, leaf, self)
            }
            return .split(id: "drop-preview", axis: axis, ratio: 0.5, a: a, b: b)
        case .pane: return self
        case let .split(id, axis, ratio, a, b):
            return .split(id: id, axis: axis, ratio: ratio, a: a.inserting(leaf, beside: target, dir), b: b.inserting(leaf, beside: target, dir))
        }
    }
}
