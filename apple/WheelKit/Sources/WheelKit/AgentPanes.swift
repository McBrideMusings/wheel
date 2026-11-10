import Foundation

/// Agent panes: each one owns an agent the daemon names, and a client attaches to that
/// agent to draw its conversation.
public enum AgentPanes {
    /// Every agent pane in the layout: pane id to the agent it shows.
    public static func agents(_ spaces: [SpaceInfo]) -> [String: String] {
        var out: [String: String] = [:]
        for space in spaces {
            for pane in space.allPanes {
                switch pane.kind {
                case let .agent(agent, _, _): out[pane.id] = agent
                default: break
                }
            }
        }
        return out
    }

    /// The harness (`claude`, `codex`, `pi`) of the pane running `agent`; nil when no pane runs it.
    public static func harness(ofAgent agent: String, in spaces: [SpaceInfo]) -> String? {
        for space in spaces {
            for pane in space.allPanes {
                if case let .agent(name, harness, _) = pane.kind, name == agent { return harness }
            }
        }
        return nil
    }

    /// Whether the harness can fork a conversation at a turn (`agent_fork`): Claude Code and
    /// Codex can, Pi cannot.
    public static func canFork(harness: String?) -> Bool {
        harness == "claude" || harness == "codex"
    }

    /// The home directory the pane running `agent` saved for itself; nil when it saved none or no
    /// pane runs the agent.
    public static func home(ofAgent agent: String, in spaces: [SpaceInfo]) -> String? {
        for space in spaces {
            for pane in space.allPanes {
                if case let .agent(name, _, home) = pane.kind, name == agent { return home }
            }
        }
        return nil
    }

    /// Permission requests waiting in each space, by space id: a request belongs to the space
    /// whose agent pane runs the agent that asked, or whose web pane's refresh command asked
    /// (its agent is the pane id).
    public static func waiting(_ perms: [PermInfo], in spaces: [SpaceInfo]) -> [String: Int] {
        var out: [String: Int] = [:]
        for space in spaces {
            var agents = Set(agents([space]).values)
            for pane in space.allPanes {
                if case .web = pane.kind { agents.insert(pane.id) }
            }
            let count = perms.filter { agents.contains($0.agent) }.count
            if count > 0 { out[space.id] = count }
        }
        return out
    }
}
