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
                case let .agent(session, _, _): out[pane.id] = session
                default: break
                }
            }
        }
        return out
    }

    /// The home directory the pane running `session` saved for itself; nil when it saved none or no
    /// pane runs the session.
    public static func home(ofSession session: String, in spaces: [SpaceInfo]) -> String? {
        for space in spaces {
            for pane in space.root.panes {
                if case let .agent(name, _, home) = pane.kind, name == session { return home }
            }
        }
        return nil
    }

    /// Permission requests waiting in each space, by space id: a request belongs to the space
    /// whose agent pane runs the session that asked, or whose web pane's refresh command asked
    /// (its session is the pane id).
    public static func waiting(_ perms: [PermInfo], in spaces: [SpaceInfo]) -> [String: Int] {
        var out: [String: Int] = [:]
        for space in spaces {
            var sessions = Set(sessions([space]).values)
            for pane in space.root.panes {
                if case .web = pane.kind { sessions.insert(pane.id) }
            }
            let count = perms.filter { sessions.contains($0.session) }.count
            if count > 0 { out[space.id] = count }
        }
        return out
    }
}
