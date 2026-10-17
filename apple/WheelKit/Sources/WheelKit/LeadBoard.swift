import Foundation

/// The lead's inbox (proto/src/lib.rs `InboxItem`): what an agent pane left for the lead to raise
/// with the person.
public enum InboxKind: String, Decodable, Sendable, Equatable {
    case turnEnded = "turn_ended"
    case permission
    case failed
}

/// An item moves forward only: `new` until the lead is told of it, `seen` once told, `surfaced`
/// once the lead raised it with the person, `closed` once nothing waits on it.
public enum InboxState: String, Decodable, Sendable, Equatable {
    case new, seen, surfaced, closed
}

public struct InboxSource: Decodable, Sendable, Equatable {
    public let space: String
    public let pane: String
    public let session: String

    public init(space: String, pane: String, session: String) {
        self.space = space
        self.pane = pane
        self.session = session
    }
}

public struct InboxItem: Decodable, Sendable, Equatable, Identifiable {
    public let id: String
    public let source: InboxSource
    public let kind: InboxKind
    /// Milliseconds since the Unix epoch.
    public let time: UInt64
    public let state: InboxState
    public let detail: String?
    public let perm: String?

    public init(id: String, source: InboxSource, kind: InboxKind, time: UInt64, state: InboxState, detail: String? = nil, perm: String? = nil) {
        self.id = id
        self.source = source
        self.kind = kind
        self.time = time
        self.state = state
        self.detail = detail
        self.perm = perm
    }

    /// What the row says the pane is waiting for.
    public var headline: String {
        switch kind {
        case .turnEnded: "Finished its turn"
        case .permission: "Asks permission"
        case .failed: "Failed"
        }
    }

    public var glyph: String {
        switch kind {
        case .turnEnded: "text.bubble"
        case .permission: "hand.raised.fill"
        case .failed: "exclamationmark.triangle.fill"
        }
    }
}

/// One supervisor as a line of the lead card: a dot for its state, the goal, how much has landed
/// and a state word (always beside its dot, never colour alone).
public struct SupervisorSummary: Sendable, Equatable, Identifiable {
    public var id: String { pane }
    public let pane: String
    public let space: String
    public let spaceName: String
    public let supervisor: Supervisor
    public let landed: Int
    public let total: Int
    public let workers: [WorkerInfo]
    public let forYou: Int

    public init(pane: String, space: String, spaceName: String, supervisor: Supervisor, landed: Int, total: Int, workers: [WorkerInfo], forYou: Int) {
        self.pane = pane
        self.space = space
        self.spaceName = spaceName
        self.supervisor = supervisor
        self.landed = landed
        self.total = total
        self.workers = workers
        self.forYou = forYou
    }

    /// What the line names: the goal bead, else the focus label.
    public var title: String { supervisor.scope }

    /// `2/5`, the pinned line's progress.
    public var progressText: String { "\(landed)/\(total)" }

    public var progress: Double { total == 0 ? 0 : Double(landed) / Double(total) }

    public var stateWord: String {
        switch supervisor.state {
        case .running: "Running"
        case .paused: "Paused"
        case .held: "Held"
        case .stopped: "Stopped"
        }
    }

    public var glyph: String {
        switch supervisor.state {
        case .running: "circle.fill"
        case .paused, .held: "pause.circle.fill"
        case .stopped: "stop.circle.fill"
        }
    }

    public var workersText: String { "\(workers.count) of \(supervisor.workers) workers" }
}

/// Everything the lead card's board draws: one summary per supervisor across spaces, and the
/// inbox items that wait on the person. Worked out from what the daemon sent.
public struct LeadBoard: Sendable, Equatable {
    public let supervisors: [SupervisorSummary]
    /// The open inbox items (everything not closed), oldest first.
    public let forYou: [InboxItem]
    /// The badge in the header: `Inbox N`.
    public let inboxOpen: Int

    public init(supervisors: [SupervisorSummary], forYou: [InboxItem], inboxOpen: Int) {
        self.supervisors = supervisors
        self.forYou = forYou
        self.inboxOpen = inboxOpen
    }

    public var inboxLabel: String { "Inbox \(inboxOpen)" }

    public static let noSupervisors = "No supervisors running"

    /// The inbox's items that are not closed, oldest first: what waits on the person, since the
    /// lead raises each with them.
    public static func waiting(_ items: [InboxItem]) -> [InboxItem] {
        items.filter { $0.state != .closed }.sorted { $0.time < $1.time }
    }

    /// The summaries of every supervisor pane in `spaces`, in layout order. A stopped supervisor
    /// is not running, so it is left out. `progress` gives (landed, total) from a pane's board.
    public static func summaries(_ spaces: [SpaceInfo], infos: [String: SupervisorInfo], progress: (String) -> (landed: Int, total: Int)?) -> [SupervisorSummary] {
        var out: [SupervisorSummary] = []
        for space in spaces {
            for pane in space.root.panes {
                guard case let .supervisor(supervisor) = pane.kind, supervisor.state != .stopped else { continue }
                let info = infos[pane.id]
                let counts = progress(pane.id) ?? (0, supervisor.queue?.count ?? 0)
                out.append(SupervisorSummary(
                    pane: pane.id, space: space.id, spaceName: space.name, supervisor: supervisor,
                    landed: counts.landed, total: counts.total, workers: info?.running ?? [], forYou: info?.inbox ?? 0))
            }
        }
        return out
    }
}
