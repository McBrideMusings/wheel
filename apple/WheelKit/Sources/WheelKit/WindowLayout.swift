import Foundation

/// One Mac window: an ordered list of space ids, an optional name and a sidebar flag (ADR-0014).
/// wheeld stores none of it.
public struct WindowRecord: Codable, Equatable, Identifiable, Sendable {
    public var id: String
    /// What a person called the window; nil titles it by its active space.
    public var name: String?
    public var spaces: [String]
    /// The space the window shows: one of `spaces`, nil only while it holds none.
    public var active: String?
    public var sidebar: Bool

    public init(id: String = UUID().uuidString, name: String? = nil, spaces: [String] = [], active: String? = nil, sidebar: Bool = true) {
        self.id = id
        self.name = name
        self.spaces = spaces
        self.active = active ?? spaces.first
        self.sidebar = sidebar
    }
}

/// Which window holds each space. A space is in one window or in the background, where it keeps
/// running with no window; the Mac app keeps this in its preferences and reconciles it against the
/// spaces wheeld reports.
public struct WindowLayout: Codable, Equatable, Sendable {
    public private(set) var windows: [WindowRecord] = []
    /// Spaces in no window, in the order they went there.
    public private(set) var background: [String] = []
    /// The window the person used last: the one a new space opens in.
    public var focused: String?

    public init() {}

    public init(windows: [WindowRecord], background: [String] = [], focused: String? = nil) {
        self.windows = windows
        self.background = background
        self.focused = focused ?? windows.first?.id
        normalize()
    }

    public func window(_ id: String) -> WindowRecord? { windows.first { $0.id == id } }
    public func window(holding space: String) -> WindowRecord? { windows.first { $0.spaces.contains(space) } }
    public var focusedWindow: WindowRecord? { focused.flatMap { id in windows.first { $0.id == id } } ?? windows.first }

    /// Every space the layout places anywhere.
    public var placed: Set<String> { Set(windows.flatMap(\.spaces)).union(background) }

    private func index(of id: String) -> Int? { windows.firstIndex { $0.id == id } }

    /// Each window's active space is one of its spaces; the focused window exists.
    private mutating func normalize() {
        for i in windows.indices {
            if windows[i].active.map(windows[i].spaces.contains) != true { windows[i].active = windows[i].spaces.first }
        }
        if focused.map({ id in windows.contains { $0.id == id } }) != true { focused = windows.first?.id }
    }

    /// Match the spaces wheeld reports. A space that no longer exists leaves every window and the
    /// background; one the layout has never placed goes to the background when `background` names
    /// it, and otherwise opens in the focused window, or the background when no window is open.
    /// The first time there is nothing saved at all, one window holds every space. Returns the
    /// spaces that appeared.
    @discardableResult
    public mutating func reconcile(_ spaces: [String], background unshown: Set<String> = []) -> [String] {
        let existing = Set(spaces)
        for i in windows.indices { windows[i].spaces.removeAll { !existing.contains($0) } }
        background.removeAll { !existing.contains($0) }
        let appeared = spaces.filter { !placed.contains($0) }
        let fresh = appeared.filter { !unshown.contains($0) }
        if windows.isEmpty, background.isEmpty, !fresh.isEmpty {
            // Nothing was ever saved: every space goes to one window rather than the background.
            windows = [WindowRecord(spaces: fresh)]
            focused = windows[0].id
        } else if let target = focusedWindow.map(\.id).flatMap(index) {
            windows[target].spaces += fresh
            if let first = fresh.first { windows[target].active = first }
        } else {
            background += fresh
        }
        background += appeared.filter(unshown.contains)
        normalize()
        return appeared
    }

    /// The background spaces whose last pane closed between two reports: each was in `old`, and
    /// now holds only an empty pane `old` did not have, which is what wheeld puts in a space when
    /// its last pane closes. A space that just appeared, or whose one pane became empty, is not one.
    public func emptied(from old: [SpaceInfo], to new: [SpaceInfo]) -> [String] {
        let before = Dictionary(old.map { ($0.id, Set($0.root.panes.map(\.id))) }) { a, _ in a }
        return new.compactMap { space in
            guard background.contains(space.id), let had = before[space.id],
                  case let .pane(id, .empty) = space.root, !had.contains(id) else { return nil }
            return space.id
        }
    }

    /// One window holding every space, with the sidebar flag and name of the focused one: what a
    /// launch with "Restore windows at launch" off opens.
    public mutating func collapse(_ spaces: [String]) {
        let keep = focusedWindow
        windows = [WindowRecord(id: keep?.id ?? UUID().uuidString, name: keep?.name, spaces: spaces, active: keep?.active, sidebar: keep?.sidebar ?? true)]
        background = []
        focused = windows[0].id
        normalize()
    }

    /// Open a window holding `spaces`, taking them from wherever they are. Returns its id.
    @discardableResult
    public mutating func newWindow(name: String? = nil, spaces: [String] = [], sidebar: Bool = true) -> String {
        for space in spaces { remove(space) }
        let record = WindowRecord(name: name, spaces: spaces, sidebar: sidebar)
        windows.append(record)
        focused = record.id
        normalize()
        return record.id
    }

    /// Close a window: its spaces go to the background and keep running.
    public mutating func close(window id: String) {
        guard let i = index(of: id) else { return }
        background += windows[i].spaces
        windows.remove(at: i)
        normalize()
    }

    private mutating func remove(_ space: String) {
        for i in windows.indices { windows[i].spaces.removeAll { $0 == space } }
        background.removeAll { $0 == space }
    }

    /// Put `space` in `window` at `position` (the end by default), or in the background when
    /// `window` is nil. The window shows it. Returns false when `window` does not exist.
    @discardableResult
    public mutating func place(_ space: String, in window: String?, at position: Int? = nil) -> Bool {
        if let window, index(of: window) == nil { return false }
        // Moving within a window shifts the others past the removed slot.
        var position = position
        if let window, let i = index(of: window), let was = windows[i].spaces.firstIndex(of: space), let p = position, was < p {
            position = p - 1
        }
        remove(space)
        if let window, let i = index(of: window) {
            let at = min(max(position ?? windows[i].spaces.count, 0), windows[i].spaces.count)
            windows[i].spaces.insert(space, at: at)
            windows[i].active = space
        } else {
            background.append(space)
        }
        normalize()
        return true
    }

    /// Open a new window holding only `space`. Returns its id.
    @discardableResult
    public mutating func tearOff(_ space: String) -> String { newWindow(spaces: [space]) }

    public mutating func show(_ space: String, in window: String) {
        guard let i = index(of: window), windows[i].spaces.contains(space) else { return }
        windows[i].active = space
    }

    public mutating func rename(window id: String, to name: String?) {
        guard let i = index(of: id) else { return }
        let trimmed = name?.trimmingCharacters(in: .whitespacesAndNewlines)
        windows[i].name = trimmed?.isEmpty == false ? trimmed : nil
    }

    public mutating func setSidebar(_ open: Bool, in id: String) {
        guard let i = index(of: id) else { return }
        windows[i].sidebar = open
    }

    /// What the window's title bar says: its name, else its active space's name.
    public func title(of window: WindowRecord, spaceName: (String) -> String?) -> String {
        window.name ?? window.active.flatMap(spaceName) ?? "Wheel"
    }
}
