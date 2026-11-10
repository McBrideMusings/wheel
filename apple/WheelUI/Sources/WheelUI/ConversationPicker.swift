import SwiftUI
import WheelKit

/// What the conversation list can ask of the app: list the agent's conversations, start a fresh
/// one, resume a past one, open the full list. The app sends each to wheeld and updates the
/// chat's `conversationListing` and `switchingTo` as the answers arrive.
public struct ConversationActions {
    public var load: () -> Void
    public var startNew: () -> Void
    public var resume: (String) -> Void
    public var showAll: () -> Void

    public init(load: @escaping () -> Void, startNew: @escaping () -> Void, resume: @escaping (String) -> Void, showAll: @escaping () -> Void) {
        self.load = load
        self.startNew = startNew
        self.resume = resume
        self.showAll = showAll
    }
}

/// Why a row cannot be picked now, as its tooltip says; nil when it can. wheeld refuses a switch
/// while a turn runs or a message is queued, so the rows are disabled rather than removed.
@MainActor func conversationBlock(_ chat: AgentChat) -> String? {
    if chat.switchingTo != nil { return "Changing conversation…" }
    if chat.isBusy || !chat.queued.isEmpty { return "Stop the turn first" }
    return nil
}

func conversationName(_ info: ConversationInfo) -> String {
    info.title ?? "Untitled conversation"
}

/// The popover the header's conversations button opens: a search field, "New conversation", then
/// every conversation the agent holds for its directory, newest first, the current one ticked.
public struct ConversationPicker: View {
    @ObservedObject var chat: AgentChat
    let actions: ConversationActions
    @State private var query = ""

    public init(chat: AgentChat, actions: ConversationActions) {
        self.chat = chat
        self.actions = actions
    }

    private func matches(_ list: [ConversationInfo]) -> [ConversationInfo] {
        let words = query.lowercased().split(separator: " ")
        return words.isEmpty ? list : list.filter { info in
            let name = conversationName(info).lowercased()
            return words.allSatisfy { name.contains($0) }
        }
    }

    public var body: some View {
        let block = conversationBlock(chat)
        VStack(alignment: .leading, spacing: 6) {
            TextField("Search conversations", text: $query)
                .textFieldStyle(.roundedBorder)
                .accessibilityLabel("Search conversations")
            Button(action: actions.startNew) {
                ConversationRow(title: "New conversation", symbol: "plus", age: nil, current: false, busy: chat.switchingTo == "")
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(block != nil)
            .opacity(block == nil ? 1 : 0.4)
            .help(block ?? "Start a new conversation")
            Divider()
            list(block: block)
                .frame(height: 220)
            // One slot for both, so nothing moves when a refusal replaces the note.
            Text(chat.error ?? "Includes conversations started outside Wheel.")
                .font(ChatFont.meta)
                .foregroundStyle(chat.error == nil ? .tertiary : .secondary)
                .lineLimit(2, reservesSpace: true)
        }
        .padding(10)
        .frame(width: 340)
        .onAppear { actions.load() }
    }

    @ViewBuilder private func list(block: String?) -> some View {
        switch chat.conversationListing {
        case .idle, .loading:
            ProgressView().controlSize(.small).frame(maxWidth: .infinity, maxHeight: .infinity)
        case let .failed(message):
            Text(message)
                .font(ChatFont.meta)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case let .loaded(all):
            let shown = matches(all)
            if shown.isEmpty {
                Text(all.isEmpty ? "No conversations in this directory yet." : "Nothing matches.")
                    .font(ChatFont.meta)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(shown) { info in
                            Button { actions.resume(info.id) } label: {
                                ConversationRow(title: conversationName(info), symbol: nil, age: info.age(), current: info.current, busy: chat.switchingTo == info.id)
                                    .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .disabled(block != nil || info.current)
                            .opacity(block == nil || info.current ? 1 : 0.4)
                            .help(block ?? conversationName(info))
                        }
                    }
                }
            }
        }
    }
}

/// One line of the list: a tick for the current conversation, its title cut to one line, and how
/// long ago it was last worked in, in a column wide enough that the titles never shift.
struct ConversationRow: View {
    let title: String
    let symbol: String?
    let age: String?
    let current: Bool
    let busy: Bool

    var body: some View {
        HStack(spacing: 6) {
            Group {
                if busy { ProgressView().controlSize(.mini) }
                else if let symbol { Image(systemName: symbol) }
                else if current { Image(systemName: "checkmark") }
                else { Color.clear }
            }
            .frame(width: 14)
            Text(title).font(ChatFont.line).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 4)
            Text(age ?? "")
                .font(ChatFont.meta)
                .foregroundStyle(.secondary)
                .monospacedDigit()
                .frame(minWidth: 64, alignment: .trailing)
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
    }
}

/// The empty conversation's shortcut into the same list: the last three conversations and a
/// "Show all…" link. It always takes the same space, so the line above it never moves when the
/// list arrives.
struct RecentConversations: View {
    @ObservedObject var chat: AgentChat
    let actions: ConversationActions

    private var recent: [ConversationInfo] {
        guard case let .loaded(all) = chat.conversationListing else { return [] }
        return Array(all.filter { !$0.current }.prefix(3))
    }

    var body: some View {
        let block = conversationBlock(chat)
        VStack(spacing: 0) {
            ForEach(0 ..< 3, id: \.self) { index in
                if index < recent.count {
                    let info = recent[index]
                    Button { actions.resume(info.id) } label: {
                        ConversationRow(title: conversationName(info), symbol: nil, age: info.age(), current: false, busy: chat.switchingTo == info.id)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(block != nil)
                    .opacity(block == nil ? 1 : 0.4)
                    .help(block ?? conversationName(info))
                } else {
                    ConversationRow(title: " ", symbol: nil, age: nil, current: false, busy: false)
                }
            }
            Button("Show all…", action: actions.showAll)
                .buttonStyle(.link)
                .font(ChatFont.meta)
                .padding(.top, 4)
                .opacity(recent.isEmpty ? 0 : 1)
                .disabled(recent.isEmpty)
        }
        .frame(maxWidth: 330)
        .opacity(recent.isEmpty ? 0 : 1)
        .allowsHitTesting(!recent.isEmpty)
        .onAppear { actions.load() }
    }
}
