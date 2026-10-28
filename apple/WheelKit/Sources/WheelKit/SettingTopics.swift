import Foundation

/// The page of the Settings window a key belongs to. The daemon names every key; a topic is chosen
/// by the segment before its first dot, and a key no topic claims is General.
public enum SettingTopic: String, CaseIterable, Identifiable, Sendable {
    case general, agents, usage, keys, chat, terminal

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .general: "General"
        case .agents: "Agents"
        case .usage: "Usage"
        case .keys: "Keys"
        case .chat: "Chat"
        case .terminal: "Terminal"
        }
    }

    /// The SF Symbol drawn above the topic's toolbar button.
    public var symbol: String {
        switch self {
        case .general: "gearshape"
        case .agents: "sparkles"
        case .usage: "gauge.with.dots.needle.33percent"
        case .keys: "keyboard"
        case .chat: "bubble.left.and.text.bubble.right"
        case .terminal: "terminal"
        }
    }

    public static func of(_ key: String) -> SettingTopic {
        if key == "terminal.shell" { return .general }
        return switch key.split(separator: ".").first {
        case "agent": .agents
        case "usage": .usage
        case "keybind": .keys
        case "chat", "code": .chat
        case "terminal": .terminal
        default: .general
        }
    }

    public func filter(_ settings: [SettingInfo]) -> [SettingInfo] {
        settings.filter { SettingTopic.of($0.key) == self }
    }
}
