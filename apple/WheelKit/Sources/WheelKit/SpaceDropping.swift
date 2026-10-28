import CoreGraphics
import Foundation

/// Where a space dragged by its row or title chip would land, decided from the pointer alone. The
/// Mac app measures each window's sidebar rows and the menu bar popover's cards, hands them here
/// in screen coordinates (origin bottom left, as AppKit has them), and acts on the answer: an
/// insertion bar in a sidebar, an outlined window, an outlined card, or the whole desktop.
public enum SpaceDropTarget: Equatable, Sendable {
    /// Between a window's sidebar rows: `index` counts the rows above the insertion bar, in the
    /// list as it was before the drag.
    case sidebar(window: String, index: Int)
    /// Between a window's tabs, when its sidebar is hidden: `index` counts the tabs left of the
    /// insertion bar, in the list as it was before the drag.
    case tabs(window: String, index: Int)
    /// A window's body: the space goes to the end of the window.
    case window(String)
    /// A card in the menu bar popover: a window's, or the Background card (`nil`).
    case card(window: String?)
    /// No Wheel window under the pointer: the space opens in a little window.
    case desktop
}

/// A window as the pointer meets it.
public struct DropWindow: Equatable, Sendable {
    public var id: String
    public var frame: CGRect
    /// The sidebar's rectangle when the window shows one.
    public var sidebar: CGRect?
    /// The sidebar's rows, top to bottom.
    public var rows: [CGRect]
    /// The tab strip's rectangle when the window shows tabs instead of a sidebar.
    public var tabStrip: CGRect?
    /// The tabs, left to right.
    public var tabs: [CGRect]

    public init(id: String, frame: CGRect, sidebar: CGRect? = nil, rows: [CGRect] = [], tabStrip: CGRect? = nil, tabs: [CGRect] = []) {
        self.id = id
        self.frame = frame
        self.sidebar = sidebar
        self.rows = rows
        self.tabStrip = tabStrip
        self.tabs = tabs
    }
}

/// The menu bar popover while it is open: its window and the cards in it.
public struct DropPopover: Sendable {
    public var frame: CGRect
    /// Each card's rectangle, by the window it stands for; the Background card is `nil`.
    public var cards: [(window: String?, frame: CGRect)]

    public init(frame: CGRect, cards: [(window: String?, frame: CGRect)]) {
        self.frame = frame
        self.cards = cards
    }
}

public enum SpaceDropping {
    /// What releasing at `point` would do. `windows` runs front to back; the first one under the
    /// pointer decides. The popover, when open, is in front of every window.
    public static func target(at point: CGPoint, windows: [DropWindow], popover: DropPopover? = nil) -> SpaceDropTarget {
        if let popover, popover.frame.contains(point) {
            if let card = popover.cards.first(where: { $0.frame.contains(point) }) { return .card(window: card.window) }
            // Between the cards the nearer one is the intent.
            if let nearest = popover.cards.min(by: { abs($0.frame.midY - point.y) < abs($1.frame.midY - point.y) }) {
                return .card(window: nearest.window)
            }
        }
        guard let window = windows.first(where: { $0.frame.contains(point) }) else { return .desktop }
        if let sidebar = window.sidebar, sidebar.contains(point) {
            // The bar goes above the first row whose middle is below the pointer.
            let index = window.rows.firstIndex { point.y > $0.midY } ?? window.rows.count
            return .sidebar(window: window.id, index: index)
        }
        if let strip = window.tabStrip, strip.contains(point) {
            // The bar goes left of the first tab whose middle is right of the pointer.
            let index = window.tabs.firstIndex { point.x < $0.midX } ?? window.tabs.count
            return .tabs(window: window.id, index: index)
        }
        return .window(window.id)
    }

    /// The insertion bar's place for a drop at `index` while `dragged` is held in `held`, or nil
    /// where the drop changes nothing: just before or just after the dragged item itself.
    public static func barIndex(_ index: Int, held: [String], dragged: String) -> Int? {
        guard let at = held.firstIndex(of: dragged) else { return index }
        return index == at || index == at + 1 ? nil : index
    }

    /// The words under a drag, naming what releasing will do.
    public static func hint(_ target: SpaceDropTarget, space: String, windowTitle: (String) -> String) -> String {
        switch target {
        case let .sidebar(window, _), let .tabs(window, _): "Place “\(space)” here in “\(windowTitle(window))”"
        case let .window(window): "Move “\(space)” to “\(windowTitle(window))”"
        case let .card(window?): "Move “\(space)” to “\(windowTitle(window))”"
        case .card(nil): "Run “\(space)” in the background, with no window"
        case .desktop: "Release to open “\(space)” in a little window"
        }
    }
}
