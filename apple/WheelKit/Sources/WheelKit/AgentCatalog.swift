import Foundation

/// How a client names and offers the agents `status` lists.
extension AgentInfo {
    /// The name as a person reads it: "Claude" for `claude`.
    public var title: String { AgentInfo.title(of: name) }

    public static func title(of name: String) -> String {
        name.prefix(1).uppercased() + name.dropFirst()
    }

    /// Where the agent keeps its login and conversations when `agent.<name>.home` is unset.
    public var defaultHome: String {
        switch name {
        case "claude": "~/.claude"
        case "codex": "~/.codex"
        case "pi": "~/.pi/agent"
        default: "its own default"
        }
    }

    /// How to install the agent's program, for the agents wheel knows how to name.
    public var installLine: String? {
        switch name {
        case "claude": "https://docs.anthropic.com/en/docs/claude-code"
        case "codex": "brew install codex"
        case "pi": "curl -fsSL https://pi.dev/install.sh | sh"
        default: nil
        }
    }

    /// The agents an empty pane's picker offers: those whose program was found, in the order
    /// `status` listed them.
    public static func pickable(_ agents: [AgentInfo]) -> [AgentInfo] { agents.filter(\.found) }
}
