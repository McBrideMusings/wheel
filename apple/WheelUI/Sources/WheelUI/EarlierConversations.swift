import SwiftUI
import WheelKit

/// One conversation a pane showed before the one it shows now. `chat` holds its events once
/// wheeld has read them; `missing` says the agent's store no longer holds it.
public struct EarlierConversation: Identifiable {
    public let id: String
    public var chat: AgentChat?
    public var missing = false

    public init(id: String) { self.id = id }

    /// What its divider says: the first message the person sent in it, once read.
    @MainActor var title: String {
        if missing { return "Conversation no longer in the agent's store" }
        let first = chat?.rows.lazy.compactMap { row -> String? in
            if case let .user(text, _) = row.row.kind { text } else { nil }
        }.first
        let line = first.map { $0.split(whereSeparator: \.isWhitespace).joined(separator: " ") } ?? ""
        return line.isEmpty ? "Earlier conversation" : line
    }
}

/// The conversations a pane showed before its current one, oldest first, drawn dimmed above the
/// transcript, and what the app does for them: read one's events, put the agent back in one.
@MainActor public final class EarlierConversations: ObservableObject {
    @Published public private(set) var list: [EarlierConversation] = []
    /// Ask wheeld for conversation `id`'s events; the app answers with `loaded` or `gone`, or
    /// nothing when wheeld refuses, which leaves the divider titled generically and resumable.
    public var load: (String) -> Void = { _ in }
    public var resume: (String) -> Void = { _ in }
    private var asked = Set<String>()

    public init() {}

    /// Show these, oldest first; a conversation already read keeps its events.
    public func show(_ ids: [String]) {
        guard ids != list.map(\.id) else { return }
        let held = Dictionary(list.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        list = ids.map { held[$0] ?? EarlierConversation(id: $0) }
    }

    /// Read conversation `id` once, the first time its divider comes into view.
    func request(_ id: String) {
        guard let item = list.first(where: { $0.id == id }), item.chat == nil, !item.missing, asked.insert(id).inserted else { return }
        load(id)
    }

    public func loaded(_ id: String, agent: String, events: [StampedEvent]) {
        guard let index = list.firstIndex(where: { $0.id == id }) else { return }
        let chat = AgentChat(name: agent)
        chat.reset(events, daemonNow: Date())
        list[index].chat = chat
    }

    public func gone(_ id: String) {
        guard let index = list.firstIndex(where: { $0.id == id }) else { return }
        list[index].missing = true
    }

    /// The empty store a transcript with no earlier conversations draws from.
    public static let none = EarlierConversations()
}

/// The line above an earlier conversation: what it was about, and Resume, which puts the agent
/// back in it.
struct EarlierDivider: View {
    let item: EarlierConversation
    /// Why Resume cannot be pressed now; nil when it can.
    let block: String?
    let resume: () -> Void

    var body: some View {
        VStack(spacing: 6) {
            row
            Divider()
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("earlier \(item.id)")
    }

    private var row: some View {
        HStack(spacing: 10) {
            Text(item.title)
                .font(ChatFont.meta)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button("Resume", action: resume)
                .font(ChatFont.meta)
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(block != nil || item.missing)
                .help(block ?? "Continue this conversation")
                .accessibilityIdentifier("resume \(item.id)")
        }
    }
}
