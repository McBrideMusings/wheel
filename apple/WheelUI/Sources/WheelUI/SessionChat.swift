import SwiftUI
import WheelKit

/// What a message's actions depend on, observed by the user messages alone: a draft keystroke
/// changes `SessionChat` and must not redraw every message of the transcript.
@MainActor
public final class QueueState: ObservableObject {
    /// The messages waiting behind the running turn, in the order they will run.
    @Published public internal(set) var queued: [QueuedMessage] = []
    /// The turn that has started and produced no event yet: its message can still be taken back.
    @Published public internal(set) var unansweredTurn: UInt64?
    /// Whether the server can take a sent message back (`session_unsend`); an older one cannot, and
    /// Edit, Remove and Clear queue are not shown.
    @Published public var takeBackAvailable = true
}

/// One attached session's conversation: an agent pane's. Each is its own
/// object so a pane redraws only for its own session's events.
@MainActor
public final class SessionChat: ObservableObject {
    public let name: String
    @Published public var state: SessionState? {
        didSet { if isBusy != wasBusy { rebuildItems() } }
    }
    private var wasBusy = false
    /// How the transcript folds rows into lines and cards; set by the view from the space's
    /// `chat.*` settings.
    public var layout = TranscriptLayout.standard {
        didSet { if layout != oldValue { rebuildItems() } }
    }
    /// The folded conversation. Not published: views draw `rows`, so a streamed chunk redraws the
    /// one row it changes and not every view of the list.
    public private(set) var transcript = Transcript()
    /// One observable object per transcript row, in order. Reassigned only when a row is added,
    /// removed or moved; a chunk that changes a row's text changes that row's object alone.
    @Published public private(set) var rows: [RowModel] = []
    /// `rows` as the transcript lays them out under `layout`: calls folded into runs or a turn's
    /// work line, edits cards after finished turns. Rebuilt with `rows`, and whenever a tool call,
    /// a message, the layout or whether the session is busy changes.
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
    /// What the conversation is called, held by the daemon: its first
    /// message cut to one line. nil until it has a first message.
    @Published public var title: String?
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
    /// The items the transcript's list has built on screen, by id. Not published.
    public var shownItems = Set<Int>()
    /// The item at the top of the view when the transcript left the screen scrolled away from its
    /// end; it scrolls back there when it returns. Not published.
    public var returnTo: Int?
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
    /// Whether this chat's composer is on screen, set by the client that shows it. A key typed
    /// anywhere in the window goes to the composer only while it is.
    public var composerShown = false

    /// Whether `characters` is text a key types: no control characters, no function-key codes.
    public static func isTyped(_ characters: String) -> Bool {
        !characters.isEmpty
            && characters.unicodeScalars.allSatisfy { $0.value >= 0x20 && $0.value != 0x7F && !(0xF700...0xF8FF).contains($0.value) }
    }

    /// A character typed somewhere other than the composer: it joins the draft and
    /// the keyboard moves to the composer, so the next key lands there directly.
    public func typeIntoComposer(_ text: String) {
        UILog.info("transcript \(name): typed \(text.count) char(s) into the composer")
        draft += text
        composerFocusRequest += 1
    }
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
        syncQueue()
    }

    public func apply(_ event: StampedEvent) {
        transcript.apply(event)
        syncRows()
        syncQueue()
        if case let .retract(turn) = event.event, let text = editing.removeValue(forKey: turn) {
            restore(text)
        }
    }

    /// The messages waiting behind the running turn, in the order they will run. Published apart
    /// from `rows` so a queued message's actions appear and go as the queue changes, whatever the
    /// row's own text did.
    public let queue = QueueState()
    public var queued: [QueuedMessage] { queue.queued }
    public var unansweredTurn: UInt64? { queue.unansweredTurn }

    private func syncQueue() {
        let next = transcript.queuedMessages
        if next != queue.queued { queue.queued = next }
        if transcript.unansweredTurn != queue.unansweredTurn { queue.unansweredTurn = transcript.unansweredTurn }
    }

    /// Set by the client that owns the connection: ask the daemon to take back the message of a turn
    /// (`session_unsend`), or every queued one (`session_clear_queue`).
    public var sendUnsend: (UInt64) -> Void = { _ in }
    public var sendClearQueue: () -> Void = {}

    /// Texts waiting for the daemon to confirm their message is taken back, by turn. They go to the
    /// composer on the `retract` event, so a message the agent answered first is never duplicated
    /// there.
    private var editing: [UInt64: String] = [:]

    /// Take a message back and put its text in the composer to rewrite. A draft already there stays,
    /// the message's text goes after it on a line of its own.
    public func edit(turn: UInt64, text: String) {
        guard queue.takeBackAvailable else { return }
        UILog.info("transcript \(name): edit turn \(turn)")
        editing[turn] = text
        sendUnsend(turn)
    }

    /// Take a queued message back for good.
    public func remove(turn: UInt64) {
        guard queue.takeBackAvailable else { return }
        UILog.info("transcript \(name): remove queued turn \(turn)")
        sendUnsend(turn)
    }

    public func clearQueue() {
        guard queue.takeBackAvailable else { return }
        UILog.info("transcript \(name): clear queue of \(queued.count)")
        sendClearQueue()
    }

    /// Esc or Up in an empty composer: take back the message the agent has not answered. False
    /// when there is none, so the key does what it did before.
    public func takeBack() -> Bool {
        // A message sent to a session that is not busy has not reached its turn yet (the daemon's
        // `turn_start` is on its way): it is as unanswered as one whose turn started and said nothing.
        let target = unansweredTurn ?? (isBusy ? nil : queued.last?.turn)
        guard queue.takeBackAvailable, let turn = target, editing[turn] == nil,
              let row = transcript.rows.last(where: { $0.turn == turn }),
              case let .user(text, _) = row.kind else { return false }
        edit(turn: turn, text: text)
        return true
    }

    private func restore(_ text: String) {
        draft = draft.isEmpty ? text : draft + "\n" + text
        composerFocusRequest += 1
    }

    /// Each tool group by the id of a row in it, so a change to one call redraws its group's line.
    private var groupOf: [Int: ToolGroupModel] = [:]

    /// Bring `rows` in line with the transcript, keeping each row's object by its id.
    private func syncRows() {
        let source = transcript.rows
        if source.count == rows.count, zip(source, rows).allSatisfy({ $0.id == $1.id }) {
            var relayout = false
            for (row, model) in zip(source, rows) where model.update(row) {
                updated(model)
                // A tool call's status or diffs and a message leaving the queue decide where the
                // work lines and edits cards sit.
                switch row.kind {
                case .tool, .user: relayout = true
                default: break
                }
            }
            if relayout { rebuildItems() }
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
        rebuildItems()
    }

    private var editsModels: [Int: ToolGroupModel] = [:]
    /// The edits card each row of a turn with one belongs to, so a change to a row redraws the card.
    private var cardOf: [Int: ToolGroupModel] = [:]

    /// Lay `rows` out as `items` under the current layout. Groups keep their object, and so
    /// whether they are open, as calls join them.
    private func rebuildItems() {
        wasBusy = isBusy
        let next = rows
        let source = transcript.rows
        guard next.count == source.count else { return }
        // A group keeps its object as calls join it, keyed by its first call.
        let groups = Dictionary(groupOf.values.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        groupOf = [:]
        func group(_ indices: [Int]) -> ToolGroupModel {
            let members = indices.map { next[$0] }
            let group = groups[members[0].id] ?? ToolGroupModel(id: members[0].id)
            group.set(members)
            for member in members { groupOf[member.id] = group }
            return group
        }
        var cards: [Int: ToolGroupModel] = [:]
        cardOf = [:]
        items = Transcript.items(source, layout: layout, busy: isBusy).map { item in
            switch item {
            case let .row(index): return .row(next[index])
            case let .tools(indices): return .tools(group(indices))
            case let .work(indices): return .work(group(indices))
            case let .edits(turn):
                let first = next[turn.lowerBound].id
                let model = editsModels[first] ?? ToolGroupModel(id: ChatItem.editsBase - first)
                model.set(Array(next[turn]))
                cards[first] = model
                for member in model.rows { cardOf[member.id] = model }
                return .edits(model)
            }
        }
        editsModels = cards
        listUpdates += 1
    }

    private func updated(_ row: RowModel) {
        rowUpdates += 1
        groupOf[row.id]?.objectWillChange.send()
        cardOf[row.id]?.objectWillChange.send()
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
    /// A turn's tool calls and reasoning as one line.
    case work(ToolGroupModel)
    /// The card of the files a finished turn edited.
    case edits(ToolGroupModel)

    /// Edits cards count down from here, so their ids never name a row.
    static let editsBase = -(1 << 40)

    /// The first row's id, so a group keeps its identity as calls join it.
    public var id: Int {
        switch self {
        case let .row(row): row.id
        case let .tools(group), let .work(group), let .edits(group): group.id
        }
    }

    @MainActor public var isUser: Bool {
        if case let .row(row) = self, case .user = row.row.kind { return true }
        return false
    }

    /// What a row contributes to a copy: reply and message text as the person sees it, a tool
    /// line's title, a thought's text while the person has it opened. Nothing for a row that shows
    /// no text of its own.
    @MainActor public func prose(expanded: Set<Int>) -> RenderedProse {
        switch self {
        case let .row(row):
            switch row.row.kind.displayed {
            case let .user(text, _): return .plain(text)
            case let .agent(text): return .markdown(text)
            case let .thought(text): return .plain(expanded.contains(row.id) ? text : "")
            case let .tool(title, _, _, _): return .plain(title)
            case let .plan(entries): return .plain(entries.map(\.content).joined(separator: "\n"))
            case let .notice(text), let .error(text): return .plain(text)
            case let .mode(mode): return .plain("Mode: \(mode)")
            }
        case let .tools(group), let .work(group):
            return .plain(group.rows.compactMap { row in
                if case let .tool(title, _, _, _) = row.row.kind { title } else { nil }
            }.joined(separator: "\n"))
        case .edits:
            return .plain("")
        }
    }

    @MainActor public func plainText(expanded: Set<Int>) -> String { prose(expanded: expanded).text }

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
