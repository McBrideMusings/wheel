import Foundation

/// A named, ordered group of spaces (proto/src/lib.rs SessionInfo, ADR-0021). wheeld holds them
/// with the layout; every space is in exactly one session.
public struct SessionInfo: Decodable, Sendable, Equatable, Identifiable {
    public let id: String
    public let name: String
    /// Its spaces' ids, in order.
    public let spaces: [String]

    public init(id: String, name: String, spaces: [String]) {
        self.id = id
        self.name = name
        self.spaces = spaces
    }
}

extension Array where Element == SessionInfo {
    /// The session holding `space`.
    public func session(of space: String) -> SessionInfo? { first { $0.spaces.contains(space) } }
}

/// Which session and space one client shows (ADR-0022). wheeld keeps no view: the client works it
/// out from the layout it holds, so a switch draws at once and moves no other client.
public struct SessionView: Equatable, Sendable {
    /// The session shown; it may hold no space.
    public private(set) var session: String?
    /// The space shown, always one of `session`'s; nil when the session holds none.
    public private(set) var space: String?
    /// The space this client last showed in each session, so switching back returns to it.
    public private(set) var lastSpace: [String: String] = [:]

    public init(session: String? = nil, space: String? = nil) {
        self.session = session
        self.space = space
    }

    /// Show `space`; the shown session becomes the one holding it. A space no session holds is ignored.
    public mutating func show(space: String, in sessions: [SessionInfo]) {
        guard let holder = sessions.session(of: space) else { return }
        session = holder.id
        self.space = space
        lastSpace[holder.id] = space
    }

    /// Show `session`: the space last shown in it while that space is still there, else its first.
    public mutating func show(session id: String, in sessions: [SessionInfo]) {
        guard let holder = sessions.first(where: { $0.id == id }) else { return }
        session = id
        if let last = lastSpace[id], holder.spaces.contains(last) { space = last } else { space = holder.spaces.first }
        if let space { lastSpace[id] = space }
    }

    /// A new layout arrived. The shown space stays shown wherever it is now, its session following
    /// it (a space moved to another session). A shown space that is gone gives way to the space
    /// beside it in the same session, the next one, else the one before; a session left empty stays
    /// shown with no space. A session that is gone gives way to the first session.
    public mutating func layoutChanged(from old: [SessionInfo], to new: [SessionInfo]) {
        lastSpace = lastSpace.filter { new.session(of: $0.value)?.id == $0.key }
        if let space, new.session(of: space) != nil { return show(space: space, in: new) }
        guard let session, let now = new.first(where: { $0.id == session }) else {
            space = nil
            self.session = nil
            if let first = new.first { show(session: first.id, in: new) }
            return
        }
        guard let space, let was = old.first(where: { $0.id == session }), let at = was.spaces.firstIndex(of: space) else {
            self.space = now.spaces.first
            if let first = now.spaces.first { lastSpace[session] = first }
            return
        }
        let after = was.spaces[(at + 1)...].first { now.spaces.contains($0) }
        let before = was.spaces[..<at].last { now.spaces.contains($0) }
        self.space = after ?? before ?? now.spaces.first
        if let shown = self.space { lastSpace[session] = shown } else { lastSpace[session] = nil }
    }

    /// Start from a view saved at the last launch: its space when it is still there, else its
    /// session's first space, else the first session. Says which it took, for the log.
    public static func restored(session: String?, space: String?, in sessions: [SessionInfo]) -> (view: SessionView, took: String) {
        var view = SessionView()
        if let space, sessions.session(of: space) != nil {
            view.show(space: space, in: sessions)
            return (view, "space")
        }
        if let session, sessions.contains(where: { $0.id == session }) {
            view.show(session: session, in: sessions)
            return (view, "session")
        }
        if let first = sessions.first { view.show(session: first.id, in: sessions) }
        return (view, "first session")
    }
}

/// The words the title menu's rows carry under each name.
public enum SessionCaption {
    /// "4 panes · 2 working · 1 needs you": the space's pane count, then how many of its agent
    /// panes are at each rank `shown` lists, most urgent first. Idle panes add nothing.
    public static func space(_ space: SpaceInfo, shown: Set<StatusRank>) -> String {
        let panes = space.root.panes.count
        return ([panes == 1 ? "1 pane" : "\(panes) panes"] + counts(space.paneStatuses.values.map(\.rank), shown: shown))
            .joined(separator: " · ")
    }

    /// "3 spaces · 1 needs you · 2 working": the session's space count, then how many of its spaces
    /// are at each rank `shown` lists; a session with none of those says "idle".
    public static func session(_ session: SessionInfo, spaces: [SpaceInfo], shown: Set<StatusRank>) -> String {
        let held = spaces.filter { session.spaces.contains($0.id) }
        let count = session.spaces.count
        let ranks = counts(held.compactMap { $0.status?.rank }, shown: shown)
        return ([count == 1 ? "1 space" : "\(count) spaces"] + (ranks.isEmpty ? ["idle"] : ranks)).joined(separator: " · ")
    }

    /// The highest rank among a session's spaces, for its dot; nil when none has a status.
    public static func rank(of session: SessionInfo, spaces: [SpaceInfo]) -> StatusRank? {
        let held = Set(spaces.filter { session.spaces.contains($0.id) }.compactMap { $0.status?.rank })
        return StatusRank.allCases.first(where: held.contains)
    }

    private static func counts(_ ranks: [StatusRank], shown: Set<StatusRank>) -> [String] {
        StatusRank.allCases.filter { $0 != .idle && shown.contains($0) }.compactMap { rank in
            let n = ranks.filter { $0 == rank }.count
            guard n > 0 else { return nil }
            switch rank {
            case .needsYou: return "\(n) needs you"
            case .stopped: return "\(n) stopped"
            case .working: return "\(n) working"
            case .idle: return nil
            }
        }
    }

    /// The question asked before closing a space whose agents are working, or nil when none is:
    /// "Close wheel? 2 agents are working. Closing ends them and every terminal in it."
    public static func closeQuestion(_ space: SpaceInfo) -> String? {
        let working = space.paneStatuses.values.filter { $0 == .working }.count
        guard working > 0 else { return nil }
        return working == 1
            ? "Close \(space.name)? 1 agent is working. Closing ends it and every terminal in it."
            : "Close \(space.name)? \(working) agents are working. Closing ends them and every terminal in it."
    }
}
