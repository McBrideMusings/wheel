import WheelKit

/// How the phone names a pane wherever it lists one.
enum PhoneCommands {
    /// What a pane is called in the list: its agent's name and state, or what it shows.
    static func paneName(_ kind: PaneKind, status: PaneStatus?) -> String? {
        func agent(_ name: String) -> String {
            guard let status else { return name }
            return "\(name) — \(stateWords(status))"
        }
        switch kind {
        case let .agent(name, _, _): return agent(name)
        case let .terminal(_, name?): return agent(name.name)
        case .terminal: return "Terminal"
        case let .web(artifact): return artifact.name
        case .browser: return "Browser"
        case .empty, .unknown: return nil
        }
    }

    static func stateWords(_ status: PaneStatus) -> String {
        switch status {
        case .needsYou: "needs you"
        case .stopped: "stopped"
        case .working: "working"
        case .done: "done"
        case .idle: "idle"
        }
    }
}
