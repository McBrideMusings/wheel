import Foundation

/// One row of a conversation as a client draws it.
public struct TranscriptRow: Identifiable, Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        /// A message the person sent; `queued` while it waits for its turn to start.
        case user(String, queued: Bool)
        /// The agent's reply text; streamed chunks are merged into one row.
        case agent(String)
        /// The agent's reasoning; streamed chunks are merged into one row.
        case thought(String)
        /// A tool call; later updates change its title, status and content in place.
        case tool(title: String, kind: String, status: String, content: [ToolContent])
        case plan([PlanEntry])
        /// The agent switched its own mode and has no `mode` option to show it in; the value is the mode's id.
        case mode(String)
        /// A turn that stopped for a reason other than finishing.
        case notice(String)
        case error(String)
    }

    public let id: Int
    public var kind: Kind {
        didSet { revision &+= 1 }
    }
    /// The agent's call id of a `.tool` row; nil for every other kind.
    public var toolCall: String? = nil {
        didSet { revision &+= 1 }
    }
    /// When the daemon recorded the row's first event; nil for replayed history.
    public var started: Date? = nil {
        didSet { revision &+= 1 }
    }
    /// When a tool call or a thought ended: the tool's final status, or for a thought the next
    /// event of its turn. Nil while it runs, and for replayed history.
    public var ended: Date? = nil {
        didSet { revision &+= 1 }
    }
    /// A tool call or a thought still in progress: no final status yet and its turn has not
    /// ended. Kept apart from `ended` because replayed history ends with no time.
    public var open = false {
        didSet { revision &+= 1 }
    }
    /// Counts changes to the fields above, so a client learns that a row changed without holding a copy of
    /// its text: a second owner of a streaming reply's string makes every appended chunk copy it.
    public private(set) var revision = 0

    public init(id: Int, kind: Kind, toolCall: String? = nil) {
        self.id = id
        self.kind = kind
        self.toolCall = toolCall
    }

    public static func == (a: TranscriptRow, b: TranscriptRow) -> Bool {
        a.id == b.id && a.kind == b.kind && a.toolCall == b.toolCall && a.started == b.started && a.ended == b.ended && a.open == b.open
    }

    /// How long the row ran: to `ended`, or to `now` while it is still open. Nil when the row has
    /// no start time, or ended with none.
    public func elapsed(now: Date) -> TimeInterval? {
        guard let started else { return nil }
        if let ended { return max(0, ended.timeIntervalSince(started)) }
        return open ? max(0, now.timeIntervalSince(started)) : nil
    }
}

/// Statuses after which a tool call does no more work.
private let finalStatuses: Set<String> = ["completed", "failed"]

/// How a client lays the rows out, top to bottom. Each case names rows by index into
/// `Transcript.rows`.
public enum TranscriptItem: Equatable, Sendable {
    /// One row.
    case row(Int)
    /// A run of tool calls drawn as one line (`chat.tools.summary` runs).
    case tools([Int])
    /// A turn's tool calls and reasoning, and any text between them, drawn as one line (`turn`).
    case work([Int])
    /// The card of the files a finished turn edited; the range is the turn's rows.
    case edits(Range<Int>)
}

/// Folds one agent pane's session event stream into rows.
///
/// Messages still waiting for their turn stay at the bottom, in the order they will run; rows of
/// the turn in progress are inserted above them, and `turn_start` moves a message into place.
public struct Transcript: Sendable, Equatable {
    public private(set) var rows: [TranscriptRow] = []
    private var nextId = 0
    /// Client clock minus daemon clock, as of the last `reset`.
    private var clockOffset: TimeInterval = 0
    private var toolRows: [String: Int] = [:]
    /// A plan is always sent whole; the one plan row is replaced in place.
    private var planRow: Int?
    /// The turns of the queued rows at the end of `rows`, in the same order.
    private var queuedTurns: [UInt64] = []

    public init() {}

    /// Replace everything with a full transcript (`session_attached`, sent on attach and on resync).
    ///
    /// `daemonNow` is the daemon's clock when it sent the events, `received` this client's clock when
    /// they arrived. Their difference shifts every stamp onto the client's clock, so a running
    /// line measures from the daemon's now whatever the two clocks read.
    public mutating func reset(_ events: [StampedEvent], daemonNow: Date, received: Date = Date()) {
        clockOffset = received.timeIntervalSince(daemonNow)
        rows = []
        toolRows = [:]
        planRow = nil
        queuedTurns = []
        for event in events { apply(event) }
    }

    public mutating func apply(_ stamped: StampedEvent) {
        apply(stamped.event, at: stamped.at.map { $0.addingTimeInterval(clockOffset) })
    }

    public mutating func apply(_ event: SessionEvent, at: Date? = nil) {
        // A thought ends when its turn moves on to something else: a reply, a tool call, a plan, or
        // the turn's end. An update to an earlier tool call, or a later message being queued, is
        // not the turn moving on.
        switch event {
        case .text, .toolCall, .plan, .mode, .turnEnd, .error:
            if let last = lastSettled, case .thought = rows[last].kind, rows[last].open { close(last, at: at) }
        case .user, .turnStart, .thought, .toolUpdate, .other:
            break
        }
        switch event {
        case let .user(turn, text):
            rows.append(TranscriptRow(id: nextId, kind: .user(text, queued: true)))
            rows[rows.count - 1].started = at
            nextId += 1
            queuedTurns.append(turn)
        case let .turnStart(turn):
            guard let position = queuedTurns.firstIndex(of: turn) else { return }
            var row = rows.remove(at: settledCount + position)
            queuedTurns.remove(at: position)
            if case let .user(text, _) = row.kind { row.kind = .user(text, queued: false) }
            rows.insert(row, at: settledCount)
        case let .text(chunk):
            if let last = lastSettled, case .agent(var text) = rows[last].kind {
                // Empty the row first so `text` is the string's only owner and the append is
                // amortised O(chunk) instead of copying the whole reply every time.
                rows[last].kind = .agent("")
                text += chunk
                rows[last].kind = .agent(text)
            } else {
                append(.agent(chunk), at: at)
            }
        case let .thought(chunk):
            if let last = lastSettled, case let .thought(text) = rows[last].kind, rows[last].open {
                rows[last].kind = .thought(text + chunk)
            } else {
                let index = append(.thought(chunk), at: at)
                rows[index].open = true
            }
        case let .toolCall(id, title, kind, status, content):
            // A second call with the same id takes over the id; the earlier row gets no more updates.
            if let earlier = toolRows[id], rows[earlier].open { close(earlier, at: at) }
            let index = append(.tool(title: title, kind: kind, status: status, content: content), toolCall: id, at: at)
            toolRows[id] = index
            if finalStatuses.contains(status) { rows[index].ended = at } else { rows[index].open = true }
        case let .toolUpdate(id, title, status, content):
            guard let index = toolRows[id], case let .tool(oldTitle, kind, oldStatus, oldContent) = rows[index].kind else { return }
            rows[index].kind = .tool(title: title ?? oldTitle, kind: kind, status: status ?? oldStatus, content: content ?? oldContent)
            if let status, finalStatuses.contains(status), rows[index].open { close(index, at: at) }
        case let .plan(entries):
            if let index = planRow {
                rows[index].kind = .plan(entries)
            } else {
                planRow = append(.plan(entries), at: at)
            }
        case let .mode(mode):
            append(.mode(mode), at: at)
        case let .turnEnd(_, stopReason):
            closeAll(at: at)
            if let words = StopReason.words(for: stopReason) { append(.notice("Turn ended: \(words)"), at: at) }
        case let .error(_, message):
            // A failed turn, or an agent that stopped: nothing it started is still running.
            closeAll(at: at)
            append(.error(message), at: at)
        case .other:
            break
        }
    }

    /// The rows as a client lays them out, top to bottom. A turn runs from one message to the next;
    /// it is finished when a message that has started follows it, or nothing does and the session
    /// is not `busy`.
    public static func items(_ rows: [TranscriptRow], layout: TranscriptLayout = .standard, busy: Bool = false) -> [TranscriptItem] {
        func isUser(_ row: TranscriptRow) -> Bool { if case .user = row.kind { true } else { false } }
        func queued(_ row: TranscriptRow) -> Bool { if case .user(_, true) = row.kind { true } else { false } }
        func hidden(_ row: TranscriptRow) -> Bool { if case .thought = row.kind { layout.thinking == .hidden } else { false } }
        func isWork(_ row: TranscriptRow) -> Bool {
            switch row.kind {
            case .tool: true
            case .thought: !hidden(row)
            default: false
            }
        }
        var items: [TranscriptItem] = []
        var index = 0
        while index < rows.count {
            if isUser(rows[index]) {
                items.append(.row(index))
                index += 1
                continue
            }
            var end = index
            while end < rows.count, !isUser(rows[end]) { end += 1 }
            let turn = index..<end
            func alone(_ range: Range<Int>) {
                for i in range where !hidden(rows[i]) { items.append(.row(i)) }
            }
            switch layout.tools {
            case .each:
                alone(turn)
            case .runs:
                var i = turn.lowerBound
                while i < turn.upperBound {
                    guard case .tool = rows[i].kind else {
                        alone(i..<(i + 1))
                        i += 1
                        continue
                    }
                    var j = i
                    var run: [Int] = []
                    while j < turn.upperBound {
                        if case .tool = rows[j].kind { run.append(j) } else if !hidden(rows[j]) { break }
                        j += 1
                    }
                    items.append(.tools(run))
                    i = j
                }
            case .turn:
                // Every row that is not tool work is drawn alone, the agent's messages included; each
                // stretch of tool calls and thoughts between two of them is one section.
                var section: [Int] = []
                for i in turn {
                    if isWork(rows[i]) {
                        section.append(i)
                    } else if !hidden(rows[i]) {
                        if !section.isEmpty { items.append(.work(section)); section = [] }
                        items.append(.row(i))
                    }
                }
                if !section.isEmpty { items.append(.work(section)) }
            }
            let finished = end < rows.count ? !queued(rows[end]) : !busy
            if layout.editsCard, finished, Edits.any(in: rows[turn]) { items.append(.edits(turn)) }
            index = end
        }
        return items
    }

    /// Rows above the queued messages.
    private var settledCount: Int { rows.count - queuedTurns.count }

    private var lastSettled: Int? { settledCount > 0 ? settledCount - 1 : nil }

    private mutating func close(_ index: Int, at: Date?) {
        rows[index].open = false
        if rows[index].started != nil { rows[index].ended = at }
    }

    /// End every tool call and thought still open: its turn is over.
    private mutating func closeAll(at: Date?) {
        for index in rows.indices where rows[index].open { close(index, at: at) }
    }

    /// Add a row of the turn in progress, above any queued messages; returns its index.
    @discardableResult
    private mutating func append(_ kind: TranscriptRow.Kind, toolCall: String? = nil, at: Date?) -> Int {
        let index = settledCount
        var row = TranscriptRow(id: nextId, kind: kind, toolCall: toolCall)
        row.started = at
        rows.insert(row, at: index)
        nextId += 1
        // Inserting above the queued messages moves nothing a tool or plan index points at: those
        // rows are all settled, above the insertion point.
        return index
    }
}
