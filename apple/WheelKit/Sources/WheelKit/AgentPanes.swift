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
                case let .agent(session): out[pane.id] = session
                default: break
                }
            }
        }
        return out
    }
}
