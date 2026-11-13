import SwiftUI
import WheelKit

/// How many conversations each request for the list asks for.
public enum ConversationPaging {
    /// The empty conversation's shortcut: the newest three, and one more in case the agent's own
    /// conversation is among them.
    public static let recent = 4
    /// One page of the popover.
    public static let popover = 20
}

/// What the conversation list can ask of the app: list the agent's conversations a page at a time,
/// start a fresh one, resume a past one, open the full list. The app sends each to wheeld and
/// updates the chat's `conversationListing`, `conversationsMore` and `switchingTo` as the answers
/// arrive.
public struct ConversationActions {
    /// The newest page of at most this many, replacing what the chat holds.
    public var load: (Int) -> Void
    /// The page of at most this many that follows what the chat holds.
    public var loadMore: (Int) -> Void
    public var startNew: () -> Void
    public var resume: (String) -> Void
    public var showAll: () -> Void

    public init(load: @escaping (Int) -> Void, loadMore: @escaping (Int) -> Void, startNew: @escaping () -> Void, resume: @escaping (String) -> Void, showAll: @escaping () -> Void) {
        self.load = load
        self.loadMore = loadMore
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

/// What a conversation is called: the agent's own title with any tag the store kept in it made
/// readable (`<command-message>redesign</command-message>` reads `/redesign`) and the whitespace
/// collapsed.
func conversationName(_ info: ConversationInfo) -> String {
    guard var title = info.title else { return "Untitled conversation" }
    if let open = title.range(of: "<command-message>"), let close = title.range(of: "</command-message>", range: open.upperBound ..< title.endIndex) {
        title = "/" + title[open.upperBound ..< close.lowerBound]
    }
    title = title.replacingOccurrences(of: "<[^>]+>", with: " ", options: .regularExpression)
    title = title.split(whereSeparator: \.isWhitespace).joined(separator: " ")
    return title.isEmpty ? "Untitled conversation" : title
}

/// A reply as a card shows it: the inline Markdown drawn (bold, italic, code, links) and the
/// block marks a list item or heading starts with taken out. Text the parser refuses stays as
/// written.
func conversationReply(_ reply: String) -> AttributedString {
    let text = reply.replacingOccurrences(of: #"^\s*(#{1,6}\s+|[-*+]\s+|\d+[.)]\s+)"#, with: "", options: .regularExpression)
    let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
    return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
}

/// The day a conversation falls under in the popover's list.
enum ConversationDay: Int, CaseIterable {
    case today, yesterday, lastWeek, earlier

    var label: String {
        switch self {
        case .today: "Today"
        case .yesterday: "Yesterday"
        case .lastWeek: "Last week"
        case .earlier: "Earlier"
        }
    }

    init(_ date: Date?, now: Date = Date(), calendar: Calendar = .current) {
        guard let date else { self = .earlier; return }
        if calendar.isDate(date, inSameDayAs: now) { self = .today }
        else if let yesterday = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(date, inSameDayAs: yesterday) { self = .yesterday }
        else if let week = calendar.date(byAdding: .day, value: -7, to: now), date > week { self = .lastWeek }
        else { self = .earlier }
    }
}

/// The time a row shows under its day heading: the clock time for today and yesterday, the date
/// after that, so it never repeats the heading.
func conversationTime(_ info: ConversationInfo, day: ConversationDay) -> String? {
    guard let updated = info.updated else { return nil }
    switch day {
    case .today, .yesterday: return updated.formatted(date: .omitted, time: .shortened)
    case .lastWeek, .earlier: return updated.formatted(.dateTime.month(.abbreviated).day())
    }
}

/// The popover the header's conversations button opens: a search field, "New conversation", then
/// the conversations the agent holds for its directory under Today, Yesterday, Last week and
/// Earlier, newest first, the current one ticked. The list loads a page at a time as it is
/// scrolled; a search keeps loading pages, whether or not the end of the list is in view, until
/// the whole list is in.
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
            let text = (conversationName(info) + " " + (info.lastReply ?? "")).lowercased()
            return words.allSatisfy { text.contains($0) }
        }
    }

    public var body: some View {
        let block = conversationBlock(chat)
        VStack(alignment: .leading, spacing: 6) {
            TextField("Search conversations", text: $query)
                .textFieldStyle(.roundedBorder)
                .accessibilityLabel("Search conversations")
            Button(action: actions.startNew) {
                ConversationRow(title: "New conversation", symbol: "plus", time: nil, reply: nil, current: false, busy: chat.switchingTo == "", card: false)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(block != nil)
            .opacity(block == nil ? 1 : 0.4)
            .help(block ?? "Start a new conversation")
            Divider()
            list(block: block)
                .frame(height: 360)
            // One slot for both, so nothing moves when a refusal replaces the note.
            Text(chat.error ?? "Includes conversations started outside Wheel.")
                .font(ChatFont.meta)
                .foregroundStyle(chat.error == nil ? .tertiary : .secondary)
                .lineLimit(2, reservesSpace: true)
        }
        .padding(10)
        .frame(width: 340)
        .onAppear { actions.load(ConversationPaging.popover) }
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
            if shown.isEmpty, !chat.conversationsMore {
                Text(all.isEmpty ? "No conversations in this directory yet." : "Nothing matches.")
                    .font(ChatFont.meta)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                let days = Dictionary(grouping: shown) { ConversationDay($0.updated) }
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 6) {
                        ForEach(ConversationDay.allCases, id: \.self) { day in
                            if let rows = days[day] {
                                Text(day.label.uppercased())
                                    .font(ChatFont.meta)
                                    .foregroundStyle(.tertiary)
                                    .padding(.horizontal, 8)
                                    .padding(.top, 4)
                                ForEach(rows) { info in
                                    Button { actions.resume(info.id) } label: {
                                        ConversationRow(title: conversationName(info), symbol: nil, time: conversationTime(info, day: day), reply: info.lastReply, current: info.current, busy: chat.switchingTo == info.id, card: true)
                                            .contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                    .disabled(block != nil || info.current)
                                    .opacity(block == nil || info.current ? 1 : 0.4)
                                    .help(block ?? conversationName(info))
                                }
                            }
                        }
                        if chat.conversationsMore {
                            // Under the last row, so reaching it asks for the next page; the id
                            // changes with each page so it asks again while it is still in view.
                            ProgressView().controlSize(.small)
                                .frame(maxWidth: .infinity)
                                .id(all.count)
                                .onAppear { actions.loadMore(ConversationPaging.popover) }
                        }
                    }
                }
                // A search looks through every conversation, so it keeps asking for pages as
                // each arrives and as the words change, wherever the scroll is.
                .onChange(of: all.count) { if !query.isEmpty { actions.loadMore(ConversationPaging.popover) } }
                .onChange(of: query) { if !query.isEmpty { actions.loadMore(ConversationPaging.popover) } }
            }
        }
    }
}

/// One conversation in a list: its title, how long ago or when it was last worked in, and what
/// the agent last said under them, in the chat's own type (the title the `line` size in
/// semibold, the rest `meta`). A card has a soft fill that deepens under the pointer; the reply
/// always reserves two lines, so every card is the same height and nothing moves when the list
/// arrives. A row with a symbol or no reply (New conversation) is one plain line.
struct ConversationRow: View {
    @Environment(\.chatAppearance) private var look
    let title: String
    let symbol: String?
    let time: String?
    let reply: String?
    let current: Bool
    let busy: Bool
    let card: Bool
    @State private var hovering = false

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if busy { ProgressView().controlSize(.mini).frame(width: 14) }
                else if let symbol { Image(systemName: symbol).frame(width: 14) }
                else if current { Image(systemName: "checkmark").foregroundStyle(Color.accentColor).frame(width: 14) }
                Text(title).font(card ? look.line.weight(.semibold) : look.line).lineLimit(1).truncationMode(.tail)
                Spacer(minLength: 4)
                if let time {
                    Text(time)
                        .font(look.meta)
                        .foregroundStyle(.tertiary)
                        .monospacedDigit()
                }
            }
            if card {
                Text(reply.map(conversationReply) ?? AttributedString(" "))
                    .font(look.meta)
                    .foregroundStyle(.secondary)
                    .lineLimit(2, reservesSpace: true)
                    .multilineTextAlignment(.leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, card ? 7 : 4)
        .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(card ? (hovering ? 0.09 : 0.04) : (hovering ? 0.06 : 0))))
        .onHover { hovering = $0 }
    }
}

/// The empty conversation's shortcut into the same list: a "Recent" heading with "Show all…" on
/// its right, then the newest three conversations as cards, centred under the prompt. It always
/// takes the same space, so the line above it never moves when the list arrives.
struct RecentConversations: View {
    @ObservedObject var chat: AgentChat
    let actions: ConversationActions
    @Environment(\.chatAppearance) private var look

    private var recent: [ConversationInfo] {
        guard case let .loaded(all) = chat.conversationListing else { return [] }
        return Array(all.filter { !$0.current }.prefix(3))
    }

    var body: some View {
        let block = conversationBlock(chat)
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Recent")
                    .font(look.meta)
                    .foregroundStyle(.tertiary)
                Spacer()
                #if os(macOS)
                Button("Show all…", action: actions.showAll)
                    .buttonStyle(.link)
                    .font(look.meta)
                #else
                Button("Show all…", action: actions.showAll)
                    .buttonStyle(.borderless)
                    .font(look.meta)
                #endif
            }
            .padding(.horizontal, 8)
            ForEach(0 ..< 3, id: \.self) { index in
                if index < recent.count {
                    let info = recent[index]
                    Button { actions.resume(info.id) } label: {
                        ConversationRow(title: conversationName(info), symbol: nil, time: info.age(), reply: info.lastReply, current: false, busy: chat.switchingTo == info.id, card: true)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(block != nil)
                    .opacity(block == nil ? 1 : 0.4)
                    .help(block ?? conversationName(info))
                } else {
                    ConversationRow(title: " ", symbol: nil, time: nil, reply: nil, current: false, busy: false, card: true)
                }
            }
        }
        .frame(maxWidth: 360)
        .opacity(recent.isEmpty ? 0 : 1)
        .allowsHitTesting(!recent.isEmpty)
        .onAppear { actions.load(ConversationPaging.recent) }
    }
}
