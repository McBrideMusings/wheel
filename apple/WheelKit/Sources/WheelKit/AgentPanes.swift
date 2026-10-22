import Foundation

/// Agent panes: each one owns an agent session the daemon names, and a client attaches to that
/// session to draw its conversation.
public enum AgentPanes {
    /// Every agent pane in the layout: pane id to the session it shows.
    public static func sessions(_ spaces: [SpaceInfo]) -> [String: String] {
        var out: [String: String] = [:]
        for space in spaces {
            for pane in space.root.panes {
                switch pane.kind {
                case let .agent(session, _): out[pane.id] = session
                default: break
                }
            }
        }
        return out
    }

    /// Permission requests waiting in each space, by space id: a request belongs to the space
    /// whose agent pane runs the session that asked.
    public static func waiting(_ perms: [PermInfo], in spaces: [SpaceInfo]) -> [String: Int] {
        var out: [String: Int] = [:]
        for space in spaces {
            let sessions = Set(sessions([space]).values)
            let count = perms.filter { sessions.contains($0.session) }.count
            if count > 0 { out[space.id] = count }
        }
        return out
    }
}
