import Foundation

/// Agent panes: each one owns an agent session the daemon names, and a client attaches to that
/// session to draw its conversation, the same way it attaches to the lead.
public enum AgentPanes {
    /// Every agent pane in the layout: pane id to the session it shows.
    public static func sessions(_ spaces: [SpaceInfo]) -> [String: String] {
        var out: [String: String] = [:]
        for space in spaces {
            for pane in space.root.panes {
                switch pane.kind {
                case let .agent(session): out[pane.id] = session
                case let .supervisor(supervisor): out[pane.id] = supervisor.session
                case let .worker(session): out[pane.id] = session
                default: break
                }
            }
        }
        return out
    }
}
