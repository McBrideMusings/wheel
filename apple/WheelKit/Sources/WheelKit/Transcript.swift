import Foundation

/// One row of a conversation as a client draws it.
public struct TranscriptRow: Identifiable, Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        /// A message the person sent; `queued` while it waits for its turn to start.
        case user(String, queued: Bool)
        /// The agent's reply text; streamed chunks are merged into one row.
        case agent(String)
        /// A tool call; later updates change its title, status and content in place.
        case tool(title: String, kind: String, status: String, content: [ToolContent])
        case plan([PlanEntry])
        /// A turn that stopped for a reason other than finishing.
        case notice(String)
        case error(String)
    }

    public let id: Int
    public var kind: Kind
}

/// Folds the lead's event stream into rows. The lead and every agent pane share this.
///
/// Messages still waiting for their turn stay at the bottom, in the order they will run; rows of
/// the turn in progress are inserted above them, and `turn_start` moves a message into place.
public struct Transcript: Sendable, Equatable {
    public private(set) var rows: [TranscriptRow] = []
    private var nextId = 0
    private var toolRows: [String: Int] = [:]
    /// A plan is always sent whole; the one plan row is replaced in place.
    private var planRow: Int?
    /// The turns of the queued rows at the end of `rows`, in the same order.
    private var queuedTurns: [UInt64] = []

    public init() {}

    /// Replace everything with a full transcript (`lead_attached`, sent on attach and on resync).
    public mutating func reset(_ events: [LeadEvent]) {
        rows = []
        toolRows = [:]
        planRow = nil
        queuedTurns = []
        for event in events { apply(event) }
    }

    public mutating func apply(_ event: LeadEvent) {
        switch event {
        case let .user(turn, text):
            rows.append(TranscriptRow(id: nextId, kind: .user(text, queued: true)))
            nextId += 1
            queuedTurns.append(turn)
        case let .turnStart(turn):
            guard let position = queuedTurns.firstIndex(of: turn) else { return }
            var row = rows.remove(at: settledCount + position)
            queuedTurns.remove(at: position)
            if case let .user(text, _) = row.kind { row.kind = .user(text, queued: false) }
            rows.insert(row, at: settledCount)
        case let .text(chunk):
            if let last = lastSettled, case let .agent(text) = rows[last].kind {
                rows[last].kind = .agent(text + chunk)
            } else {
                append(.agent(chunk))
            }
        case let .toolCall(id, title, kind, status, content):
            toolRows[id] = settledCount
            append(.tool(title: title, kind: kind, status: status, content: content))
        case let .toolUpdate(id, title, status, content):
            guard let index = toolRows[id], case let .tool(oldTitle, kind, oldStatus, oldContent) = rows[index].kind else { return }
            rows[index].kind = .tool(title: title ?? oldTitle, kind: kind, status: status ?? oldStatus, content: content ?? oldContent)
        case let .plan(entries):
            if let index = planRow {
                rows[index].kind = .plan(entries)
            } else {
                planRow = settledCount
                append(.plan(entries))
            }
        case let .turnEnd(_, stopReason):
            if stopReason != "end_turn" { append(.notice("Turn ended: \(stopReason.replacingOccurrences(of: "_", with: " "))")) }
        case let .error(_, message):
            append(.error(message))
        case .other:
            break
        }
    }

    /// Rows above the queued messages.
    private var settledCount: Int { rows.count - queuedTurns.count }

    private var lastSettled: Int? { settledCount > 0 ? settledCount - 1 : nil }

    /// Add a row of the turn in progress, above any queued messages.
    private mutating func append(_ kind: TranscriptRow.Kind) {
        rows.insert(TranscriptRow(id: nextId, kind: kind), at: settledCount)
        nextId += 1
    }
}
