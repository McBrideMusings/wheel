import SwiftUI
import WheelKit

/// One attached session's conversation: an agent pane's. Each is its own
/// object so a pane redraws only for its own session's events.
@MainActor
public final class SessionChat: ObservableObject {
    public let name: String
    @Published public var state: SessionState?
    /// The folded conversation. Not published: views draw `rows`, so a streamed chunk redraws the
    /// one row it changes and not every view of the list.
    public private(set) var transcript = Transcript()
    /// One observable object per transcript row, in order. Reassigned only when a row is added,
    /// removed or moved; a chunk that changes a row's text changes that row's object alone.
    @Published public private(set) var rows: [RowModel] = []
    /// `rows` as the transcript lays them out: runs of tool calls grouped. Rebuilt with `rows`;
    /// a row never changes between tool and not-tool, so nothing else moves a group's edges.
    @Published public private(set) var items: [ChatItem] = []
    /// How many times `rows` was reassigned and how many row objects were told to redraw, for the
    /// state query: a streamed chunk adds one to `rowUpdates` and nothing to `listUpdates`.
    public private(set) var listUpdates = 0
    public private(set) var rowUpdates = 0
    /// The last thing the daemon refused for this session, shown under its composer until the
    /// next send or attach.
    @Published public var error: String?
    /// A status the daemon holds over this session, drawn as a dismissible banner above its
    /// composer: a conversation that would not reopen. Never part of the transcript.
    @Published public var notice: String?
    /// How full the conversation's context window is, as the agent last reported it; nil while it
    /// has reported none, and the header then shows nothing.
    @Published public var context: ContextUse?
    /// The slash commands the agent accepts, by name; nil until it has listed them.
    @Published public var commands: Set<String>?
    /// `offered` without the commands this session's agent does not accept. All of them while the
    /// agent has listed none.
    public func accepted(_ offered: [SkillInfo]) -> [SkillInfo] {
        guard let commands else { return offered }
        return offered.filter { commands.contains($0.name) }
    }
    /// The text in this session's composer, not yet sent.
    @Published public var draft = ""
    /// The transcript follows new output: true until the person scrolls away from the end, and
    /// again once they scroll back to it. Not published: the view reads it when content changes.
    public var pinned = true
    /// A person's scroll of the transcript is under way (dragging, or coasting after it).
    public var userScrolling = false
    /// When a scroll-wheel event or a scrolling key last reached this transcript. A notched mouse
    /// wheel and the keys report no scroll phase, so this is how the view tells its move from layout settling.
    public var wheelAt = Date.distantPast
    /// Counts the End key (and a Down that lands at the end): the view re-pins and goes to its last
    /// item by instruction, since a scroll view's own end shifts while rows near it are measured.
    @Published public var endRequest = 0
    /// The platform's scroll-wheel and key monitor that stamps `wheelAt`, held while the view is on screen.
    public var wheelMonitor: Any?
    /// How far the transcript's view sits above its end, in points, for the state query.
    public var fromEnd: CGFloat = 0
    /// Where the transcript's scroll view sits in its window, top-left origin, so the debug
    /// endpoint can find that scroll view and move it the way a person's scroll does.
    public var frameInWindow: CGRect = .zero
    /// The selection that spans this transcript's rows.
    public let selection = TranscriptSelection()
    /// Tool lines, tool groups and thoughts the person opened, by row id (a group by its first
    /// call's). Held here rather than in the views so a row rebuilt when its pane is rebuilt stays
    /// open, and so the debug endpoint can open one the way a click does.
    @Published public private(set) var expanded: Set<Int> = []

    public func toggle(_ id: Int) {
        if expanded.remove(id) == nil { expanded.insert(id) }
        UILog.info("transcript \(name): \(expanded.contains(id) ? "opened" : "closed") \(id)")
    }
    /// Counts requests to put the keyboard in this session's composer; the debug endpoint is the
    /// only sender, standing in for a click the program cannot make.
    @Published public var composerFocusRequest = 0
    /// The controls the agent offers over this session, in the order they are drawn. Empty while
    /// the agent starts, and for an agent that offers none.
    @Published public private(set) var config: [ConfigOption] = []

    /// Store the agent's options in draw order, so the card and the state query can never
    /// disagree about which control comes first.
    public func setConfig(_ options: [ConfigOption]) {
        // Swift's sort is not documented as stable, so options the agent gave no category to keep
        // the order it sent them in only if their position is part of the comparison.
        config = options.enumerated()
            .sorted { ($0.element.category.map(Self.rank) ?? Self.uncategorised, $0.offset) < ($1.element.category.map(Self.rank) ?? Self.uncategorised, $1.offset) }
            .map(\.element)
    }

    /// The agent's own categories say what an option is for, so the row can order them without this
    /// wheel knowing any option's name: what answers, then how hard it thinks, then what it may
    /// do without asking. Anything uncategorised — an agent's own extra control — sorts last,
    /// keeping the order the agent sent it in.
    private static let uncategorised = 4

    private static func rank(_ category: String) -> Int {
        switch category {
        case "model": return 0
        case "thought_level": return 1
        case "mode": return 2
        case "model_config": return 3
        default: return uncategorised
        }
    }

    public init(name: String) {
        self.name = name
    }

    public var isBusy: Bool {
        if case .busy = state { return true }
        return false
    }

    public func reset(_ events: [StampedEvent], daemonNow: Date) {
        transcript.reset(events, daemonNow: daemonNow)
        syncRows()
    }

    public func apply(_ event: StampedEvent) {
        transcript.apply(event)
        syncRows()
    }

    /// Each tool group by the id of a row in it, so a change to one call redraws its group's line.
    private var groupOf: [Int: ToolGroupModel] = [:]

    /// Bring `rows` in line with the transcript, keeping each row's object by its id.
    private func syncRows() {
        let source = transcript.rows
        if source.count == rows.count, zip(source, rows).allSatisfy({ $0.id == $1.id }) {
            for (row, model) in zip(source, rows) where model.update(row) { updated(model) }
            return
        }
        let existing = Dictionary(rows.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        var next: [RowModel] = []
        for (index, row) in source.enumerated() {
            if let model = existing[row.id] {
                model.index = index
                if model.update(row) { updated(model) }
                next.append(model)
            } else {
                next.append(RowModel(row, index: index, chat: self))
            }
        }
        rows = next
        // A group keeps its object as calls join it, keyed by its first call.
        let groups = Dictionary(groupOf.values.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        groupOf = [:]
        items = Transcript.items(source).map { item in
            switch item {
            case let .row(index):
                return .row(next[index])
            case let .tools(range):
                let members = Array(next[range])
                let group = groups[members[0].id] ?? ToolGroupModel(id: members[0].id)
                group.set(members)
                for member in members { groupOf[member.id] = group }
                return .tools(group)
            }
        }
        listUpdates += 1
    }

    private func updated(_ row: RowModel) {
        rowUpdates += 1
        groupOf[row.id]?.objectWillChange.send()
    }
}

/// A run of tool calls drawn as one group. Its own object, so the group's line redraws when any
/// of its calls changes, and keeps its identity — and whether it is open — as calls join it.
@MainActor
public final class ToolGroupModel: ObservableObject, Identifiable {
    public let id: Int
    public private(set) var rows: [RowModel] = []

    /// The group's entry in `SessionChat.expanded`: below zero, so it never names the same thing
    /// as its first call's row id.
    public var openKey: Int { -1 - id }

    init(id: Int) {
        self.id = id
    }

    func set(_ members: [RowModel]) {
        guard members.map(\.id) != rows.map(\.id) else { return }
        objectWillChange.send()
        rows = members
    }
}

/// One entry of the laid-out transcript: a row, or a run of tool calls drawn as one group.
public enum ChatItem: Identifiable {
    case row(RowModel)
    case tools(ToolGroupModel)

    /// The first row's id, so a group keeps its identity as calls join it.
    public var id: Int {
        switch self {
        case let .row(row): row.id
        case let .tools(group): group.id
        }
    }

    @MainActor public var isUser: Bool {
        if case let .row(row) = self, case .user = row.row.kind { return true }
        return false
    }

    /// What a row contributes to a copy: reply and message text, a tool line's title, a thought's
    /// text while the person has it opened. Nothing for a row that shows no text of its own.
    @MainActor public func plainText(expanded: Set<Int>) -> String {
        switch self {
        case let .row(row):
            switch row.row.kind.displayed {
            case let .user(text, _): return text
            case let .agent(text): return text
            case let .thought(text): return expanded.contains(row.id) ? text : ""
            case let .tool(title, _, _, _): return title
            case let .plan(entries): return entries.map(\.content).joined(separator: "\n")
            case let .notice(text), let .error(text): return text
            case let .mode(mode): return "Mode: \(mode)"
            }
        case let .tools(group):
            return group.rows.compactMap { row in
                if case let .tool(title, _, _, _) = row.row.kind { title } else { nil }
            }.joined(separator: "\n")
        }
    }

    /// Prose a row draws as text of its own, whose highlight follows the characters; any other
    /// row is highlighted whole.
    @MainActor public var isProse: Bool {
        guard case let .row(row) = self else { return false }
        switch row.row.kind {
        case .user, .agent, .thought: return true
        default: return false
        }
    }
}

/// One transcript row as a view observes it. Its views redraw when this row changes, not when
/// another row does. The row's contents are read from the transcript and never stored here: a
/// second owner of a streaming reply's string would make every appended chunk copy the reply.
@MainActor
public final class RowModel: ObservableObject, Identifiable {
    public let id: Int
    /// The row's position in the chat's transcript.
    fileprivate(set) var index: Int
    /// The transcript row's revision when this object last told its views to redraw.
    private var revision: Int
    private weak var chat: SessionChat?
    /// Times a view built this row's body, for the state query.
    public private(set) var draws = 0

    init(_ row: TranscriptRow, index: Int, chat: SessionChat) {
        id = row.id
        revision = row.revision
        self.index = index
        self.chat = chat
    }

    /// The row as the transcript holds it now.
    /// A row the transcript no longer holds (a view still drawing it as the list is replaced)
    /// reads as an empty notice.
    public var row: TranscriptRow {
        guard let rows = chat?.transcript.rows else { return TranscriptRow(id: id, kind: .notice("")) }
        if index < rows.count, rows[index].id == id { return rows[index] }
        return rows.first { $0.id == id } ?? TranscriptRow(id: id, kind: .notice(""))
    }

    /// Note the row's new revision and redraw its views; false (and no redraw) when it is unchanged.
    func update(_ new: TranscriptRow) -> Bool {
        guard new.revision != revision else { return false }
        revision = new.revision
        objectWillChange.send()
        return true
    }

    func drew() { draws += 1 }
}
