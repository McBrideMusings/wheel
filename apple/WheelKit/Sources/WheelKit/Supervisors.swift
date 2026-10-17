import Foundation

/// Supervisors (proto/src/lib.rs `Supervisor`, `SupervisorInfo`, `LoopEvent`; ADR-0007). A
/// supervisor is a pane kind whose whole saved state rides in the layout; what it is running now,
/// the decisions its loop made and what waits on it come from `supervisor_*` replies.

/// Where a supervisor stands. `held` is the usage cap holding it: running workers finish and
/// nothing new starts. `stopped` is final.
public enum SupervisorRun: String, Decodable, Sendable, Equatable {
    case running, paused, held, stopped
}

/// One bead in a supervisor's queue and the model its worker runs on.
public struct QueueEntry: Decodable, Sendable, Equatable {
    public let bead: String
    public let model: String
}

/// A supervisor as its pane saves it.
public struct Supervisor: Decodable, Sendable, Equatable {
    public let id: String
    public let goal: String?
    public let focus: String?
    public let workers: Int
    /// The share, 1 to 100, of the week's usage it may spend before it holds; nil for no cap.
    public let cap: Int?
    public let state: SupervisorRun
    public let session: String
    public let queue: [QueueEntry]?
    public let planError: String?

    private enum CodingKeys: String, CodingKey {
        case id, goal, focus, workers, cap, state, session, queue
        case planError = "plan_error"
    }

    public init(id: String, goal: String?, focus: String? = nil, workers: Int, cap: Int?, state: SupervisorRun, session: String,
                queue: [QueueEntry]? = nil, planError: String? = nil) {
        self.id = id
        self.goal = goal
        self.focus = focus
        self.workers = workers
        self.cap = cap
        self.state = state
        self.session = session
        self.queue = queue
        self.planError = planError
    }

    /// What the header names: the goal bead, else the focus label.
    public var scope: String { goal ?? focus ?? id }
}

/// A worker a supervisor is running.
public struct WorkerInfo: Decodable, Sendable, Equatable {
    public let session: String
    public let bead: String
    public let title: String
    public let model: String
    public let worktree: String
    public let branch: String
    public let continues: Int
    public let restarts: Int
    /// The pane the person pushed it to, if any.
    public let pane: String?

    public init(session: String, bead: String, title: String, model: String, worktree: String = "", branch: String = "",
                continues: Int = 0, restarts: Int = 0, pane: String? = nil) {
        self.session = session
        self.bead = bead
        self.title = title
        self.model = model
        self.worktree = worktree
        self.branch = branch
        self.continues = continues
        self.restarts = restarts
        self.pane = pane
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        session = try c.decode(String.self, forKey: .session)
        bead = try c.decode(String.self, forKey: .bead)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        model = try c.decode(String.self, forKey: .model)
        worktree = try c.decodeIfPresent(String.self, forKey: .worktree) ?? ""
        branch = try c.decodeIfPresent(String.self, forKey: .branch) ?? ""
        continues = try c.decodeIfPresent(Int.self, forKey: .continues) ?? 0
        restarts = try c.decodeIfPresent(Int.self, forKey: .restarts) ?? 0
        pane = try c.decodeIfPresent(String.self, forKey: .pane)
    }

    private enum CodingKeys: String, CodingKey { case session, bead, title, model, worktree, branch, continues, restarts, pane }
}

/// A supervisor with the pane that shows it and the live state of its loop.
public struct SupervisorInfo: Decodable, Sendable, Equatable {
    public let space: String
    public let pane: String
    public let supervisor: Supervisor
    public let agent: SessionState?
    public let running: [WorkerInfo]
    /// How many items wait in its inbox.
    public let inbox: Int

    private enum CodingKeys: String, CodingKey { case space, pane, agent, running, inbox }

    public init(space: String, pane: String, supervisor: Supervisor, agent: SessionState? = nil, running: [WorkerInfo] = [], inbox: Int = 0) {
        self.space = space
        self.pane = pane
        self.supervisor = supervisor
        self.agent = agent
        self.running = running
        self.inbox = inbox
    }

    /// The daemon flattens the supervisor's own fields beside the pane's.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        space = try c.decode(String.self, forKey: .space)
        pane = try c.decode(String.self, forKey: .pane)
        supervisor = try Supervisor(from: decoder)
        agent = try c.decodeIfPresent(SessionState.self, forKey: .agent)
        running = try c.decodeIfPresent([WorkerInfo].self, forKey: .running) ?? []
        inbox = try c.decodeIfPresent(Int.self, forKey: .inbox) ?? 0
    }
}

/// What a supervisor's loop decided (proto `LoopEventKind`).
public enum LoopEventKind: Sendable, Equatable, Decodable {
    case claimed, started, continued, blocked, stalled, restarted, landed, sentBack, escalated, held, resumed
    case approvedByPolicy, pendingForPerson, bdFailed, released
    case other(String)

    public init(from decoder: Decoder) throws {
        switch try decoder.singleValueContainer().decode(String.self) {
        case "claimed": self = .claimed
        case "started": self = .started
        case "continued": self = .continued
        case "blocked": self = .blocked
        case "stalled": self = .stalled
        case "restarted": self = .restarted
        case "landed": self = .landed
        case "sent_back": self = .sentBack
        case "escalated": self = .escalated
        case "held": self = .held
        case "resumed": self = .resumed
        case "approved_by_policy": self = .approvedByPolicy
        case "pending_for_person": self = .pendingForPerson
        case "bd_failed": self = .bdFailed
        case "released": self = .released
        case let other: self = .other(other)
        }
    }

    /// The protocol's name for it, as the events list prints it.
    public var name: String {
        switch self {
        case .claimed: "claimed"
        case .started: "started"
        case .continued: "continued"
        case .blocked: "blocked"
        case .stalled: "stalled"
        case .restarted: "restarted"
        case .landed: "landed"
        case .sentBack: "sent back"
        case .escalated: "escalated"
        case .held: "held"
        case .resumed: "resumed"
        case .approvedByPolicy: "approved by policy"
        case .pendingForPerson: "waiting for you"
        case .bdFailed: "bd failed"
        case .released: "released"
        case let .other(name): name
        }
    }
}

/// One decision of a supervisor's loop.
public struct LoopEvent: Decodable, Sendable, Equatable, Identifiable {
    public let id: UInt64
    /// Unix milliseconds.
    public let at: UInt64
    public let supervisor: String
    public let worker: String?
    public let bead: String?
    public let kind: LoopEventKind
    public let detail: String

    public init(id: UInt64, at: UInt64, supervisor: String, worker: String? = nil, bead: String? = nil, kind: LoopEventKind, detail: String = "") {
        self.id = id
        self.at = at
        self.supervisor = supervisor
        self.worker = worker
        self.bead = bead
        self.kind = kind
        self.detail = detail
    }
}

/// Something the loop cannot settle: it waits on the supervisor's agent.
public struct SupervisorInboxItem: Decodable, Sendable, Equatable, Identifiable {
    public let id: String
    public let at: UInt64
    public let supervisor: String
    public let worker: String
    public let bead: String
    public let kind: LoopEventKind
    public let detail: String

    public init(id: String, at: UInt64, supervisor: String, worker: String, bead: String, kind: LoopEventKind, detail: String) {
        self.id = id
        self.at = at
        self.supervisor = supervisor
        self.worker = worker
        self.bead = bead
        self.kind = kind
        self.detail = detail
    }
}

/// How a worker's line reads: a state word, always beside a glyph so colour never carries it alone.
public struct WorkerLine: Sendable, Equatable, Identifiable {
    public var id: String { worker.session }
    public let lane: Int
    public let worker: WorkerInfo
    public let glyph: String
    public let word: String
    /// A stalled worker: its line draws in red, beside its glyph.
    public var alert = false
}

extension WorkerLine {
    /// What the row says where its Open button was once the worker has a pane of its own.
    public static let inAPaneText = "In a pane"
    /// The header of a supervisor pane showing one worker.
    public var backText: String { "← Supervisor" }
    public var laneText: String { "Worker L\(lane)" }
    public static let pushText = "Push to pane"
    /// Under the composer of a pane that shows a worker.
    public static let composerPlaceholder = "Message this worker"
    /// Beside the pushed pane's header: closing it only stops showing the worker.
    public static let closingText = "closing keeps it running"
    public var inPane: Bool { worker.pane != nil }
}

/// What the supervisor pane draws for a worker that asks for something its supervisor's policy
/// does not cover: the request's own words in the card's copy.
public enum WorkerRequest {
    public static let wantsToRun = "wants to run"
    public static let outsidePolicy = "not covered by this supervisor's policy"

    /// What "Always allow … here" names: the command a request runs, without the tool's own name
    /// or the quotes around it. A title with no command in it names itself.
    public static func subject(_ title: String) -> String {
        var words = title.split(separator: " ", omittingEmptySubsequences: true).map(String.init)
        if words.count > 1, words[0] == "Bash" || words[0] == "Run" { words.removeFirst() }
        let first = (words.first ?? title).trimmingCharacters(in: CharacterSet(charactersIn: "`'\""))
        return first.isEmpty ? title : first
    }

    /// The requests waiting on a person that `session` asked for, oldest first.
    public static func pending(_ perms: [PermInfo], session: String) -> [PermInfo] {
        perms.filter { $0.session == session }
    }
}

/// A landed bead, with the loop's own words about it.
public struct LandedBead: Sendable, Equatable, Identifiable {
    public var id: String { bead }
    public let bead: String
    public let detail: String

    public init(bead: String, detail: String) {
        self.bead = bead
        self.detail = detail
    }
}

/// Everything the supervisor pane draws, worked out from what the daemon sent.
public struct SupervisorBoard: Sendable, Equatable {
    public let info: SupervisorInfo
    public let events: [LoopEvent]
    /// Beads seen landing in an earlier answer: the daemon keeps only the latest events, so a
    /// landing can roll off the list while the bead stays landed.
    public let landedBefore: [LandedBead]
    /// What waits on the supervisor's agent; those beads are still in scope.
    public let inbox: [SupervisorInboxItem]
    /// The most continue-then-send-back rounds the loop allows a bead (proto/daemon `MAX_*`).
    public static let maxSendBacks = 2
    public static let maxRestarts = 3

    public init(info: SupervisorInfo, events: [LoopEvent], landedBefore: [LandedBead] = [], inbox: [SupervisorInboxItem] = []) {
        self.info = info
        self.events = events
        self.landedBefore = landedBefore
        self.inbox = inbox
    }

    public var supervisor: Supervisor { info.supervisor }

    public var landed: [LandedBead] {
        var seen = Set<String>()
        let fresh = events.compactMap { event -> LandedBead? in
            guard event.kind == .landed, let bead = event.bead else { return nil }
            return LandedBead(bead: bead, detail: event.detail)
        }
        return (landedBefore + fresh).filter { seen.insert($0.bead).inserted }
    }

    /// The queue, minus anything already running or landed.
    public var queued: [QueueEntry] {
        let taken = Set(info.running.map(\.bead)).union(landed.map(\.bead))
        return (supervisor.queue ?? []).filter { !taken.contains($0.bead) }
    }

    /// Every bead the supervisor has landed, is running, has queued or has waiting in its inbox.
    public var total: Int {
        Set(landed.map(\.bead)).union(info.running.map(\.bead)).union(queued.map(\.bead)).union(inbox.map(\.bead)).count
    }

    public var progressText: String { "\(landed.count) of \(total) landed" }

    public var progress: Double { total == 0 ? 0 : Double(landed.count) / Double(total) }

    /// The state words and glyphs of design copy: "Working", "Landing · sent back 1 of 2",
    /// "Stalled 6m · restarted 2 of 3", "Finishing, then held", "Paused after its turn".
    public func line(for worker: WorkerInfo, lane: Int, now: Date) -> WorkerLine {
        let mine = events.filter { $0.bead == worker.bead }
        let sentBack = mine.filter { $0.kind == .sentBack }.count
        switch supervisor.state {
        case .paused: return WorkerLine(lane: lane, worker: worker, glyph: "pause.circle", word: "Paused after its turn")
        case .held: return WorkerLine(lane: lane, worker: worker, glyph: "pause.circle", word: "Finishing, then held")
        default: break
        }
        if let last = mine.last {
            switch last.kind {
            case .stalled, .restarted:
                let stalledAt = mine.last { $0.kind == .stalled }?.at ?? last.at
                let minutes = max(0, Int(now.timeIntervalSince1970 * 1000 - Double(stalledAt)) / 60_000)
                return WorkerLine(lane: lane, worker: worker, glyph: "exclamationmark.triangle.fill",
                                  word: "Stalled \(minutes)m · restarted \(worker.restarts) of \(Self.maxRestarts)", alert: true)
            case .sentBack:
                return WorkerLine(lane: lane, worker: worker, glyph: "arrow.left.arrow.right",
                                  word: "Landing · sent back \(sentBack) of \(Self.maxSendBacks)")
            default: break
            }
        }
        return WorkerLine(lane: lane, worker: worker, glyph: "circle.fill", word: "Working")
    }

    public func lines(now: Date) -> [WorkerLine] {
        info.running.enumerated().map { line(for: $1, lane: $0 + 1, now: now) }
    }

    /// The line of the worker whose session is `session`; nil once the worker is no longer running.
    public func line(session: String, now: Date) -> WorkerLine? {
        lines(now: now).first { $0.worker.session == session }
    }

    /// The week meter: the cap the supervisor holds at, as a fraction; nil without a cap.
    public var weekCap: Double? { supervisor.cap.map { Double($0) / 100 } }

    public var holdText: String? {
        guard supervisor.state == .held, let cap = supervisor.cap else { return nil }
        return "Holding at \(cap)% of the week: running workers finish their bead, nothing new starts."
    }

    /// What the Queued section says when it has no rows, and what a queued row says while the
    /// loop is not refilling.
    public var queueEmptyText: String { "Nothing ready in this scope" }
    public var notRefilling: Bool { supervisor.state == .paused || supervisor.state == .held }

    public var forYou: Int { info.inbox }
}

/// What a space's tab shows of the supervisors in it: `▶ n` for running workers, `‖` paused,
/// `‖ 90%` held at its cap, and "N for you" for what waits on the person.
public struct SpaceMarks: Sendable, Equatable {
    public var running = 0
    public var paused = false
    public var heldCap: Int?
    public var held = false
    public var forYou = 0

    public var isEmpty: Bool { running == 0 && !paused && !held && forYou == 0 }

    /// `▶ 2`, `‖`, `‖ 90%`, in that order of precedence: held beats paused beats running.
    public var stateText: String? {
        if held { return heldCap.map { "‖ \($0)%" } ?? "‖" }
        if paused { return "‖" }
        if running > 0 { return "▶ \(running)" }
        return nil
    }

    public var forYouText: String? { forYou > 0 ? "\(forYou) for you" : nil }

    /// The marks of space `space`, from the layout's supervisor panes and what the polls found.
    public static func of(_ space: SpaceInfo, infos: [String: SupervisorInfo]) -> SpaceMarks {
        var marks = SpaceMarks()
        for pane in space.root.panes {
            guard case let .supervisor(supervisor) = pane.kind else { continue }
            let info = infos[pane.id]
            switch supervisor.state {
            case .running: marks.running += info?.running.count ?? 0
            case .paused: marks.paused = true
            case .held: marks.held = true; marks.heldCap = supervisor.cap
            case .stopped: break
            }
            marks.forYou += info?.inbox ?? 0
        }
        return marks
    }
}

public enum SupervisorPanes {
    /// Every supervisor pane in the layout: pane id to the supervisor it holds.
    public static func all(_ spaces: [SpaceInfo]) -> [String: Supervisor] {
        var out: [String: Supervisor] = [:]
        for space in spaces {
            for pane in space.root.panes {
                if case let .supervisor(supervisor) = pane.kind { out[pane.id] = supervisor }
            }
        }
        return out
    }
}
