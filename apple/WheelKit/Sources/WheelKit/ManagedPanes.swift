import Foundation

/// What a client draws for a pane the lead or a supervisor manages (ADR-0010): a locked
/// dashboard, the person's unlock and the hand-back. The daemon owns `created_by` and `prior`;
/// everything here is worked out from what it sent.
public enum ManagedPane {
    /// Where a pane stands with the person.
    public enum Lock: Sendable, Equatable {
        /// The person's own pane; nothing was ever managing it.
        case yours
        /// The lead or a supervisor manages it: clients send no input to it.
        case locked
        /// The person took it over and the earlier manager is remembered, so it can go back.
        case unlocked
    }

    /// `locking` is the daemon's switch (`status.pane_locking`): off, every pane is the person's.
    public static func lock(createdBy: CreatedBy, prior: CreatedBy?, locking: Bool) -> Lock {
        if !locking { return .yours }
        if createdBy != .person { return .locked }
        return prior == nil ? .yours : .unlocked
    }

    /// The manager as a header names it: the lead, or the supervisor's goal (its id without one).
    public static func managerName(_ by: CreatedBy, supervisors: [String: Supervisor]) -> String {
        switch by {
        case .person: "you"
        case .lead: "the lead"
        case let .supervisor(id): supervisors[id]?.scope ?? id
        }
    }

    public static func badge(_ by: CreatedBy, supervisors: [String: Supervisor]) -> String {
        "Managed by \(managerName(by, supervisors: supervisors))"
    }

    /// Whether the daemon would take a hand-back: a supervisor that has stopped (left the layout)
    /// has no one to give the pane to.
    public static func canHandBack(prior: CreatedBy?, supervisors: [String: Supervisor]) -> Bool {
        switch prior {
        case .lead: true
        case let .supervisor(id): supervisors[id] != nil
        case .person, nil: false
        }
    }

    public static func handBackText(_ prior: CreatedBy, supervisors: [String: Supervisor]) -> String {
        "Hand back to \(managerName(prior, supervisors: supervisors))"
    }

    public static let unlockText = "Unlock for direct access…"
    public static let transcriptText = "Transcript"
    public static let yoursBadge = "Yours"
    public static let lockedComposerHint = "Transcript is locked. Unlock to type."
    public static let lockedComposerPlaceholder = "Locked"
    public static let confirmButton = "Unlock and take over"

    public static func confirmTitle(_ what: String) -> String { "Unlock \(what) for direct access?" }
    public static let confirmBody = "This pane stops being managed. It becomes yours."
    public static func confirmBullets(isWorker: Bool) -> [String] {
        [
            isWorker ? "You can type to the worker and read its full conversation." : "You can type into it and read what it shows.",
            "The supervisor and the lead must ask you before they type into it, replace it or close it.",
            isWorker ? "The worker keeps running its bead." : "Whatever is running in it keeps running.",
        ]
    }

    /// `manager` is the name `managerName` gave: the lead, or a supervisor's goal.
    public static func readOnlyNotice(_ manager: String, isWorker: Bool) -> String {
        let who = isWorker && manager != "the lead" ? "Supervisor \(manager)" : manager.prefix(1).uppercased() + manager.dropFirst()
        return "Read-only. \(who) manages this \(isWorker ? "worker" : "pane"); typing is off."
    }

    /// `time` is when this window saw the unlock; a window opened after it knows no time.
    public static func unlockedBanner(at time: String?) -> String {
        "You took this pane over\(time.map { " at \($0)" } ?? ""). The supervisor and the lead now ask before acting on it."
    }
}

/// One line of a worker's inbox traffic: who sent what to whom.
public struct TrafficRow: Sendable, Equatable, Identifiable {
    public enum Direction: Sendable, Equatable { case up, down }
    public let id: String
    public let direction: Direction
    /// `daemon backlog to wheel-bda`
    public let route: String
    /// The loop's word for it: `continued`, `blocked`.
    public let kind: String
    public let at: Date
    public let detail: String
    /// Waiting on the supervisor's agent: nobody has answered it yet.
    public let isNew: Bool

    /// One loop event as a line of traffic: what a worker sends its supervisor goes up, what the
    /// supervisor sends a worker goes down, and an escalation goes up to the lead.
    public init(event: LoopEvent, worker: String?, supervisor: String) {
        let worker = worker ?? event.bead ?? "a worker"
        switch event.kind {
        case .blocked, .landed, .followUp, .filed, .stalled:
            direction = .up
            route = "\(worker) to \(supervisor)"
        case .escalated, .pendingForPerson:
            direction = .up
            route = "\(supervisor) to lead"
        default:
            direction = .down
            route = "\(supervisor) to \(worker)"
        }
        id = "e\(event.kind.name)-\(event.at)"
        kind = event.kind.name
        at = Date(timeIntervalSince1970: Double(event.at) / 1000)
        detail = event.detail
        isNew = false
    }

    public init(id: String, direction: Direction, route: String, kind: String, at: Date, detail: String, isNew: Bool) {
        self.id = id
        self.direction = direction
        self.route = route
        self.kind = kind
        self.at = at
        self.detail = detail
        self.isNew = isNew
    }
}

/// A tool call a worker made, with how long ago.
public struct LatestAction: Sendable, Equatable, Identifiable {
    public let id: Int
    public let title: String
    public let age: String
    public let running: Bool
}

/// What a managed worker pane's dashboard shows, worked out from the supervisor's board, the
/// worker's own transcript and the loop's events.
public struct WorkerDashboard: Sendable, Equatable {
    public enum Tone: Sendable, Equatable { case working, stalled, quiet }

    public let bead: String
    public let title: String
    public let supervisorName: String
    public let statusWord: String
    public let tone: Tone
    public let stageText: String
    public let stageFilled: Int
    public let stageCount: Int
    /// `14m 32s`; nil while the loop has not claimed the bead.
    public let elapsed: String?
    public let continuesText: String
    public let restartsText: String
    public let model: String
    public let branch: String
    public let actions: [LatestAction]
    public let traffic: [TrafficRow]

    /// `worker` may have left the loop's running set (it blocked, landed or was given up on); the
    /// board's inbox and events then say where it ended up.
    public init(worker: WorkerInfo, board: SupervisorBoard, rows: [TranscriptRow], supervisorName: String, now: Date) {
        let line = board.line(for: worker, lane: 1, now: now)
        let running = board.info.running.contains { $0.session == worker.session }
        let landed = board.events.contains { $0.bead == worker.bead && $0.kind == .landed }
        let mine = board.events.filter { $0.bead == worker.bead }
        let waiting = board.inbox.filter { $0.bead == worker.bead }
        bead = worker.bead
        title = worker.title
        self.supervisorName = supervisorName
        stageText = line.stageText
        stageFilled = line.stageFilled
        stageCount = WorkerLine.stageCount
        continuesText = "\(worker.continues) of \(SupervisorBoard.maxContinues)"
        restartsText = "\(worker.restarts)"
        model = worker.model
        branch = worker.branch
        if let claimed = mine.first(where: { $0.kind == .claimed }) {
            elapsed = Self.clock(now.timeIntervalSince1970 - Double(claimed.at) / 1000)
        } else {
            elapsed = nil
        }
        if !running {
            statusWord = landed ? "Landed" : "Finished"
            tone = .quiet
        } else {
            statusWord = line.word
            tone = line.alert ? .stalled : line.word == "Working" ? .working : .quiet
        }
        actions = Array(rows.compactMap { row -> LatestAction? in
            guard case let .tool(title, _, _, _) = row.kind else { return nil }
            let age = row.started.map { Self.age(now.timeIntervalSince($0)) } ?? ""
            return LatestAction(id: row.id, title: title, age: row.open ? "now" : age, running: row.open)
        }.suffix(4).reversed())
        var traffic = mine.map { TrafficRow(event: $0, worker: worker.bead, supervisor: supervisorName) }
        for item in waiting {
            traffic.removeAll { $0.id == "e\(item.kind.name)-\(item.at)" }
            traffic.append(TrafficRow(id: "i\(item.id)", direction: .up, route: "\(worker.bead) to \(supervisorName)", kind: item.kind.name,
                                      at: Date(timeIntervalSince1970: Double(item.at) / 1000), detail: item.detail, isNew: true))
        }
        self.traffic = traffic.sorted { $0.at > $1.at }
    }

    /// `14m 32s`, `1h 2m` past an hour.
    static func clock(_ seconds: TimeInterval) -> String {
        let s = max(0, Int(seconds))
        if s >= 3600 { return "\(s / 3600)h \(s % 3600 / 60)m" }
        return "\(s / 60)m \(s % 60)s"
    }

    /// `now`, `41s`, `2m`, `1h`.
    static func age(_ seconds: TimeInterval) -> String {
        let s = max(0, Int(seconds))
        if s < 5 { return "now" }
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m" }
        return "\(s / 3600)h"
    }
}
