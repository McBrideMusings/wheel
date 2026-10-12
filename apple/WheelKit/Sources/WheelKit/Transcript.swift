import Foundation

/// One row of a conversation as a client draws it.
public struct TranscriptRow: Identifiable, Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        /// A message the person sent.
        case user(String)
        /// The agent's reply text; streamed chunks are merged into one row.
        case agent(String)
        /// A tool call; later updates change its title and status in place.
        case tool(title: String, kind: String, status: String)
        case plan([PlanEntry])
        /// A turn that stopped for a reason other than finishing.
        case notice(String)
        case error(String)
    }

    public let id: Int
    public var kind: Kind
}

/// Folds the lead's event stream into rows. The lead and every agent pane share this.
public struct Transcript: Sendable, Equatable {
    public private(set) var rows: [TranscriptRow] = []
    private var nextId = 0
    private var toolRows: [String: Int] = [:]
    /// A plan is always sent whole; the one plan row is replaced in place.
    private var planRow: Int?

    public init() {}

    /// Replace everything with a full transcript (`lead_attached`, sent on attach and on resync).
    public mutating func reset(_ events: [LeadEvent]) {
        rows = []
        toolRows = [:]
        planRow = nil
        for event in events { apply(event) }
    }

    public mutating func apply(_ event: LeadEvent) {
        switch event {
        case let .user(_, text):
            append(.user(text))
        case let .text(chunk):
            if let last = rows.indices.last, case let .agent(text) = rows[last].kind {
                rows[last].kind = .agent(text + chunk)
            } else {
                append(.agent(chunk))
            }
        case let .toolCall(id, title, kind, status):
            toolRows[id] = rows.count
            append(.tool(title: title, kind: kind, status: status))
        case let .toolUpdate(id, title, status):
            guard let index = toolRows[id], case let .tool(oldTitle, kind, oldStatus) = rows[index].kind else { return }
            rows[index].kind = .tool(title: title ?? oldTitle, kind: kind, status: status ?? oldStatus)
        case let .plan(entries):
            if let index = planRow {
                rows[index].kind = .plan(entries)
            } else {
                planRow = rows.count
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

    private mutating func append(_ kind: TranscriptRow.Kind) {
        rows.append(TranscriptRow(id: nextId, kind: kind))
        nextId += 1
    }
}
