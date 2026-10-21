import Foundation

/// How a client names and offers the agents `status` lists.
extension AgentInfo {
    /// The name as a person reads it: "Claude" for `claude`.
    public var title: String { AgentInfo.title(of: name) }

    public static func title(of name: String) -> String {
        name.prefix(1).uppercased() + name.dropFirst()
    }

    /// How to install the agent's program, for the agents wheel knows how to name.
    public var installLine: String? {
        switch name {
        case "claude": "https://docs.anthropic.com/en/docs/claude-code"
        case "codex": "brew install codex"
        default: nil
        }
    }

    /// The agents an empty pane's picker offers: those whose program was found, in the order
    /// `status` listed them.
    public static func pickable(_ agents: [AgentInfo]) -> [AgentInfo] { agents.filter(\.found) }
}
