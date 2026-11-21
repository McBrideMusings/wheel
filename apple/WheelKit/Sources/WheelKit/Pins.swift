import CoreGraphics
import Foundation

/// Where the iPhone's pin sheet sits over the carousel: everything under the top bar, two partial heights, or its
/// header alone at the bottom edge. It never leaves the screen while a pane is pinned.
public enum SheetPosition: String, CaseIterable, Sendable {
    case full, high, low, header

    /// The sheet's height as a share of the area under the top bar.
    public var fraction: CGFloat {
        switch self {
        case .full: 1
        case .high: 0.7
        case .low: 0.38
        case .header: 0
        }
    }

    /// The position whose height is nearest `height` in an area `total` points tall.
    public static func nearest(to height: CGFloat, in total: CGFloat) -> SheetPosition {
        allCases.min { abs($0.fraction * total - height) < abs($1.fraction * total - height) } ?? .low
    }
}

/// The iPhone's pinned panes, by space id, each list in the order they were pinned. The phone holds them alone
/// (ADR-0022): wheeld and the Mac never hear of a pin.
public struct Pins: Equatable, Sendable {
    public private(set) var bySpace: [String: [String]]

    public init(_ bySpace: [String: [String]] = [:]) { self.bySpace = bySpace.filter { !$0.value.isEmpty } }

    public func pinned(in space: String) -> [String] { bySpace[space] ?? [] }

    public func isPinned(_ pane: String, in space: String) -> Bool { pinned(in: space).contains(pane) }

    public mutating func pin(_ pane: String, in space: String) {
        guard !isPinned(pane, in: space) else { return }
        bySpace[space, default: []].append(pane)
    }

    public mutating func unpin(_ pane: String, in space: String) {
        bySpace[space]?.removeAll { $0 == pane }
        if bySpace[space]?.isEmpty == true { bySpace[space] = nil }
    }

    /// The pins left once panes no longer in their space leave the lists. A space missing from `spaces` keeps its
    /// pins, so a layout that has not named it yet loses nothing.
    public func kept(in spaces: [SpaceInfo]) -> Pins {
        var kept = self
        for space in spaces {
            guard let list = bySpace[space.id] else { continue }
            let held = Set(space.allPanes.map(\.id))
            kept.bySpace[space.id] = list.filter(held.contains)
            if kept.bySpace[space.id]?.isEmpty == true { kept.bySpace[space.id] = nil }
        }
        return kept
    }
}
