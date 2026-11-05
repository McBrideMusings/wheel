import Foundation

/// One Mac window: the session it shows, the space of that session it shows, an optional name and a
/// sidebar flag (ADR-0014). Its tabs and sidebar list the session's spaces, which wheeld holds, so
/// the record holds none. Two windows may show the same session, each its own space of it.
public struct WindowRecord: Codable, Equatable, Identifiable, Sendable {
    public var id: String
    /// What a person called the window; nil titles it by its active space.
    public var name: String?
    /// The session shown; nil only before wheeld has reported any session, when the window takes the first.
    public var session: String?
    /// The space the window shows: one of the session's, nil only while the session holds none.
    public var active: String?
    /// The space the window last showed in each session, so switching back returns to it.
    public var lastSpace: [String: String]
    public var sidebar: Bool
    /// The window stacks above every unpinned window of any app, on every desktop.
    public var pinned: Bool

    public init(id: String = UUID().uuidString, name: String? = nil, session: String? = nil, active: String? = nil,
                lastSpace: [String: String] = [:], sidebar: Bool = true, pinned: Bool = false) {
        self.id = id
        self.name = name
        self.session = session
        self.active = active
        self.lastSpace = lastSpace
        self.sidebar = sidebar
        self.pinned = pinned
    }

    private enum CodingKeys: String, CodingKey { case id, name, session, active, lastSpace, sidebar, pinned }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decodeIfPresent(String.self, forKey: .name)
        session = try c.decodeIfPresent(String.self, forKey: .session)
        active = try c.decodeIfPresent(String.self, forKey: .active)
        lastSpace = try c.decodeIfPresent([String: String].self, forKey: .lastSpace) ?? [:]
        sidebar = try c.decode(Bool.self, forKey: .sidebar)
        pinned = try c.decodeIfPresent(Bool.self, forKey: .pinned) ?? false
    }
}

/// One little window: a Mac window showing a single space's pane grid under a header strip, with no
/// tabs and no sidebar (ADR-0014). It names the main window it belongs to, or none.
public struct LittleWindow: Codable, Equatable, Identifiable, Sendable {
    public var id: String
    public var space: String
    /// The main window whose tab for `space` this one dims; nil when it belongs to none.
    public var owner: String?
    /// The window stacks above every unpinned window of any app, on every desktop.
    public var pinned: Bool

    public init(id: String = UUID().uuidString, space: String, owner: String?, pinned: Bool = false) {
        self.id = id
        self.space = space
        self.owner = owner
        self.pinned = pinned
    }

    private enum CodingKeys: String, CodingKey { case id, space, owner, pinned }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        space = try c.decode(String.self, forKey: .space)
        owner = try c.decodeIfPresent(String.self, forKey: .owner)
        pinned = try c.decodeIfPresent(Bool.self, forKey: .pinned) ?? false
    }
}

/// Which session each main window shows. wheeld holds the sessions and their spaces; the Mac app
/// keeps the windows in its preferences and mirrors the sessions it was last sent, so a window's
/// spaces are always its session's. A session no window shows is a background session: its spaces
/// keep running with no window.
public struct WindowLayout: Codable, Equatable, Sendable {
    public private(set) var windows: [WindowRecord] = []
    /// The window the person used last: the one a new session or space opens in.
    public var focused: String?
    /// The little windows. A space in an owned one stays in its owner's session, which keeps its
    /// tab; a space in an ownerless one is in no window and not in `background`.
    public private(set) var littles: [LittleWindow] = []
    /// The sessions wheeld last reported, in order. Not saved: wheeld is where they live.
    public private(set) var sessions: [SessionInfo] = []

    private enum CodingKeys: String, CodingKey { case windows, focused, littles }

    public init() {}

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        windows = try c.decode([WindowRecord].self, forKey: .windows)
        focused = try c.decodeIfPresent(String.self, forKey: .focused)
        littles = try c.decodeIfPresent([LittleWindow].self, forKey: .littles) ?? []
    }

    public init(windows: [WindowRecord], sessions: [SessionInfo] = [], focused: String? = nil) {
        self.windows = windows
        self.sessions = sessions
        self.focused = focused ?? windows.first?.id
        normalize()
    }

    public func window(_ id: String) -> WindowRecord? { windows.first { $0.id == id } }
    public func little(_ id: String) -> LittleWindow? { littles.first { $0.id == id } }
    /// The little window showing `space`, if one is open.
    public func little(showing space: String) -> LittleWindow? { littles.first { $0.space == space } }
    public var focusedWindow: WindowRecord? { focused.flatMap { id in windows.first { $0.id == id } } ?? windows.first }

    public func session(_ id: String) -> SessionInfo? { sessions.first { $0.id == id } }
    /// The session `window` shows.
    public func session(of window: WindowRecord) -> SessionInfo? { window.session.flatMap(session) }

    /// The spaces `window`'s tabs and sidebar list, in the session's order.
    public func spaces(of window: WindowRecord) -> [String] { session(of: window)?.spaces ?? [] }

    /// The windows showing `session`, the focused one first.
    public func windows(showing session: String) -> [WindowRecord] {
        let shown = windows.filter { $0.session == session }
        return shown.filter { $0.id == focusedWindow?.id } + shown.filter { $0.id != focusedWindow?.id }
    }

    /// A window whose session holds `space`: the focused window when it does, else the first.
    public func window(holding space: String) -> WindowRecord? {
        windows.first { $0.id == focusedWindow?.id && spaces(of: $0).contains(space) }
            ?? windows.first { spaces(of: $0).contains(space) }
    }

    /// The sessions no main window shows, in the sessions' order.
    public var backgroundSessions: [SessionInfo] {
        let shown = Set(windows.compactMap(\.session))
        return sessions.filter { !shown.contains($0.id) }
    }

    /// The spaces of the background sessions, in order, less those a little window shows.
    public var background: [String] {
        backgroundSessions.flatMap(\.spaces).filter { little(showing: $0) == nil }
    }

    private func index(of id: String) -> Int? { windows.firstIndex { $0.id == id } }

    /// Each window shows a session that exists and one of its spaces; the focused window exists; a
    /// little window's owner is a window showing its space, else none.
    private mutating func normalize() {
        for i in windows.indices {
            guard !sessions.isEmpty else { continue }
            if windows[i].session.flatMap(session) == nil { windows[i].session = sessions[0].id }
            guard let held = session(of: windows[i])?.spaces else { continue }
            if windows[i].active.map(held.contains) != true { windows[i].active = windows[i].lastSpace[windows[i].session ?? ""].flatMap { held.contains($0) ? $0 : nil } ?? held.first }
            if let active = windows[i].active, let session = windows[i].session { windows[i].lastSpace[session] = active }
            windows[i].lastSpace = windows[i].lastSpace.filter { held in sessions.contains { $0.id == held.key && $0.spaces.contains(held.value) } }
        }
        if focused.map({ id in windows.contains { $0.id == id } }) != true { focused = windows.first?.id }
        for j in littles.indices {
            let space = littles[j].space
            if !(littles[j].owner.flatMap { self.window($0) }.map { spaces(of: $0).contains(space) } ?? false) {
                littles[j].owner = window(holding: space)?.id
            }
        }
    }

    /// Match the sessions wheeld reports. A window whose session is gone shows the first session;
    /// a window with none yet takes it. A little window whose space is gone closes. No window is
    /// opened: a person who closed every window keeps none. Returns the spaces that appeared since
    /// the last report, none on the first.
    @discardableResult
    public mutating func reconcile(_ now: [SessionInfo]) -> [String] {
        let before = Set(sessions.flatMap(\.spaces))
        let first = sessions.isEmpty
        sessions = now
        let existing = Set(now.flatMap(\.spaces))
        littles.removeAll { !existing.contains($0.space) }
        normalize()
        return first ? [] : now.flatMap(\.spaces).filter { !before.contains($0) }
    }

    /// The background spaces whose last pane closed between two reports: each was in `old`, and
    /// now holds only an empty pane `old` did not have, which is what wheeld puts in a space when
    /// its last pane closes. A space that just appeared, or whose one pane became empty, is not one.
    public func emptied(from old: [SpaceInfo], to new: [SpaceInfo]) -> [String] {
        let before = Dictionary(old.map { ($0.id, Set($0.root.panes.map(\.id))) }) { a, _ in a }
        let shown = background
        return new.compactMap { space in
            guard shown.contains(space.id), let had = before[space.id],
                  case let .pane(id, .empty) = space.root, !had.contains(id) else { return nil }
            return space.id
        }
    }

    /// One window showing the focused window's session, with its sidebar flag and name: what a launch
    /// with "Restore windows at launch" off opens. The little windows close.
    public mutating func collapse() {
        let keep = focusedWindow
        windows = [WindowRecord(id: keep?.id ?? UUID().uuidString, name: keep?.name, session: keep?.session, active: keep?.active,
                                lastSpace: keep?.lastSpace ?? [:], sidebar: keep?.sidebar ?? true, pinned: keep?.pinned ?? false)]
        littles = []
        focused = windows[0].id
        normalize()
    }

    /// Open a window showing `session` (the first when nil), at `space` when it is one of the
    /// session's. Returns its id.
    @discardableResult
    public mutating func newWindow(name: String? = nil, session: String? = nil, space: String? = nil, sidebar: Bool = true) -> String {
        var record = WindowRecord(name: name, session: session ?? sessions.first?.id, sidebar: sidebar)
        if let space, let held = record.session.flatMap(self.session)?.spaces, held.contains(space) { record.active = space }
        windows.append(record)
        focused = record.id
        normalize()
        return record.id
    }

    /// Close a window. Its session keeps running, shown by the other windows that show it or in
    /// the background; a little window it owned stays open and belongs to none, or to another
    /// window showing its space.
    public mutating func close(window id: String) {
        guard let i = index(of: id) else { return }
        windows.remove(at: i)
        for j in littles.indices where littles[j].owner == id { littles[j].owner = nil }
        normalize()
    }

    /// Open a little window showing `space`. A main window showing the space's session owns it;
    /// a space in a background session gets one that belongs to none. Returns the little window's
    /// id, the open one's when `space` already has one, or nil when no session holds `space`.
    @discardableResult
    public mutating func openLittle(_ space: String) -> String? {
        if let open = little(showing: space) { return open.id }
        guard sessions.contains(where: { $0.spaces.contains(space) }) else { return nil }
        let little = LittleWindow(space: space, owner: window(holding: space)?.id)
        littles.append(little)
        return little.id
    }

    /// Close a little window: its owner's tab shows the grid again, and an ownerless one's space
    /// is in the background again.
    public mutating func closeLittle(_ id: String) {
        littles.removeAll { $0.id == id }
    }

    /// Open a new window showing the session that holds `space`, at `space`. Returns its id.
    @discardableResult
    public mutating func tearOff(_ space: String) -> String? {
        guard let holder = sessions.session(of: space) else { return nil }
        return newWindow(session: holder.id, space: space)
    }

    /// Show `space` in `window`, one of its session's spaces.
    public mutating func show(_ space: String, in window: String) {
        guard let i = index(of: window), spaces(of: windows[i]).contains(space) else { return }
        windows[i].active = space
        normalize()
    }

    /// Show `session` in `window`: the space the window last showed in it while that is still
    /// there, else its first. Returns false when no such window or session exists.
    @discardableResult
    public mutating func show(session id: String, in window: String) -> Bool {
        guard let i = index(of: window), session(id) != nil else { return false }
        windows[i].session = id
        windows[i].active = nil
        normalize()
        return true
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

    public mutating func setPinned(_ pinned: Bool, window id: String) {
        guard let i = index(of: id) else { return }
        windows[i].pinned = pinned
    }

    public mutating func setPinned(_ pinned: Bool, little id: String) {
        guard let i = littles.firstIndex(where: { $0.id == id }) else { return }
        littles[i].pinned = pinned
    }

    /// What the window's title bar says: its name, else its active space's name.
    public func title(of window: WindowRecord, spaceName: (String) -> String?) -> String {
        window.name ?? window.active.flatMap(spaceName) ?? "Wheel"
    }
}
