import Foundation

/// What a pane or space asks of the person (proto/src/lib.rs PaneStatus): the daemon computes it,
/// so every client draws the same thing.
public enum PaneStatus: Sendable, Equatable, Decodable {
    /// `waiting` permission requests wait on a person; a space adds up its panes'.
    case needsYou(waiting: Int)
    case stopped
    case working
    case idle

    private enum K: String, CodingKey { case kind, waiting }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "needs_you": self = .needsYou(waiting: try c.decodeIfPresent(Int.self, forKey: .waiting) ?? 1)
        case "stopped": self = .stopped
        case "working": self = .working
        case "idle": self = .idle
        case let other:
            throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "unknown status \(other)")
        }
    }

    /// The rank this status belongs to, which `status.dots` lists.
    public var rank: StatusRank {
        switch self {
        case .needsYou: .needsYou
        case .stopped: .stopped
        case .working: .working
        case .idle: .idle
        }
    }

    /// What VoiceOver reads for the dot: "Needs you, 2 waiting", "Stopped", "Working", "Idle".
    public var label: String {
        switch self {
        case let .needsYou(waiting): "Needs you, \(waiting) waiting"
        case .stopped: "Stopped"
        case .working: "Working"
        case .idle: "Idle"
        }
    }

    /// The words a sidebar row carries beside its name, for the two ranks that have any.
    public var caption: String? {
        switch self {
        case let .needsYou(waiting): "\(waiting) waiting"
        case .stopped: "stopped"
        case .working, .idle: nil
        }
    }
}

/// The four ranks, highest first, as `status.dots` names them.
public enum StatusRank: String, CaseIterable, Sendable, Identifiable {
    case needsYou = "needs_you", stopped, working, idle

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .needsYou: "Needs you"
        case .stopped: "Stopped"
        case .working: "Working"
        case .idle: "Idle"
        }
    }

    /// The ranks a `status.dots` value lists; a name that is not a rank is ignored, an empty value
    /// lists none.
    public static func listed(_ value: String) -> Set<StatusRank> {
        Set(value.split(separator: ",").compactMap { StatusRank(rawValue: $0.trimmingCharacters(in: .whitespaces)) })
    }

    /// The `status.dots` value for `ranks`, in rank order.
    public static func setting(_ ranks: Set<StatusRank>) -> String {
        allCases.filter(ranks.contains).map(\.rawValue).joined(separator: ",")
    }
}

extension [SpaceInfo] {
    /// The status the daemon sent for a pane, whichever space holds it.
    public func status(ofPane pane: String) -> PaneStatus? {
        lazy.compactMap { $0.paneStatuses[pane] }.first
    }
}

extension SpaceInfo {
    /// The most agent marks a space draws before it says how many more there are.
    public static let agentDotLimit = 4

    /// One status per agent pane, most urgent first (needs you, stopped, working, idle), panes of
    /// the same rank in layout order, leaving out the ranks `shown` leaves out. `marks` holds at
    /// most `limit` of them and `more` counts the rest.
    public func agentStatuses(shown: Set<StatusRank>, limit: Int = SpaceInfo.agentDotLimit)
        -> (marks: [PaneStatus], more: Int)
    {
        let order = Dictionary(uniqueKeysWithValues: StatusRank.allCases.enumerated().map { ($1, $0) })
        let layout = Dictionary(uniqueKeysWithValues: root.panes.enumerated().map { ($1.id, $0) })
        let all = paneStatuses
            .filter { shown.contains($0.value.rank) }
            .sorted { a, b in
                let (ra, rb) = (order[a.value.rank] ?? 0, order[b.value.rank] ?? 0)
                return ra != rb ? ra < rb : (layout[a.key] ?? .max, a.key) < (layout[b.key] ?? .max, b.key)
            }
            .map(\.value)
        return (Array(all.prefix(limit)), max(0, all.count - limit))
    }

    /// This layout with the statuses the daemon sent. A `spaces` broadcast carries none (they
    /// change without the layout changing), so a client keeps what it holds for a pane or space
    /// that is still there.
    public func keepingStatuses(of old: SpaceInfo?) -> SpaceInfo {
        guard let old else { return self }
        var out = self
        if status == nil { out.status = old.status }
        let held = Set(root.panes.map(\.id))
        for (pane, status) in old.paneStatuses where paneStatuses[pane] == nil && held.contains(pane) {
            out.paneStatuses[pane] = status
        }
        return out
    }

    /// Apply `status_changed`: `pane` is nil when only the space's status changed.
    public func applying(pane: String?, status: PaneStatus?, spaceStatus: PaneStatus?) -> SpaceInfo {
        var out = self
        out.status = spaceStatus
        if let pane { out.paneStatuses[pane] = status }
        return out
    }
}
