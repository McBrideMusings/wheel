import Foundation

/// How far a usage reading can be trusted, as wheeld reports it.
public enum UsageKind: String, Sendable, Equatable, Decodable {
    case fresh, stale, missing
}

/// One limit besides the 5-hour and weekly ones (a per-model weekly limit).
public struct UsageLimit: Sendable, Equatable, Decodable {
    public let id: String
    public let label: String
    public let pct: Double
    public let resetsAt: String?

    private enum CodingKeys: String, CodingKey {
        case id, label, pct
        case resetsAt = "resets_at"
    }

    public init(id: String, label: String, pct: Double, resetsAt: String?) {
        self.id = id
        self.label = label
        self.pct = pct
        self.resetsAt = resetsAt
    }
}

/// One reading of an agent's usage. Percentages are of the plan's limit.
public struct UsageReading: Sendable, Equatable, Decodable {
    public let sessionPct: Double
    public let sessionResetsAt: String?
    public let weeklyPct: Double
    public let weeklyResetsAt: String?
    public let modelLimits: [UsageLimit]
    public let plan: String?
    public let extraUsage: Bool?
    public let email: String?

    private enum CodingKeys: String, CodingKey {
        case plan, email
        case sessionPct = "session_pct"
        case sessionResetsAt = "session_resets_at"
        case weeklyPct = "weekly_pct"
        case weeklyResetsAt = "weekly_resets_at"
        case modelLimits = "model_limits"
        case extraUsage = "extra_usage"
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        sessionPct = try c.decode(Double.self, forKey: .sessionPct)
        sessionResetsAt = try c.decodeIfPresent(String.self, forKey: .sessionResetsAt)
        weeklyPct = try c.decode(Double.self, forKey: .weeklyPct)
        weeklyResetsAt = try c.decodeIfPresent(String.self, forKey: .weeklyResetsAt)
        modelLimits = try c.decodeIfPresent([UsageLimit].self, forKey: .modelLimits) ?? []
        plan = try c.decodeIfPresent(String.self, forKey: .plan)
        extraUsage = try c.decodeIfPresent(Bool.self, forKey: .extraUsage)
        email = try c.decodeIfPresent(String.self, forKey: .email)
    }

    public init(sessionPct: Double, sessionResetsAt: String?, weeklyPct: Double, weeklyResetsAt: String?,
                modelLimits: [UsageLimit] = [], plan: String? = nil, extraUsage: Bool? = nil, email: String? = nil) {
        self.sessionPct = sessionPct
        self.sessionResetsAt = sessionResetsAt
        self.weeklyPct = weeklyPct
        self.weeklyResetsAt = weeklyResetsAt
        self.modelLimits = modelLimits
        self.plan = plan
        self.extraUsage = extraUsage
        self.email = email
    }
}

/// An agent's reading and how far to trust it.
public struct UsageInfo: Sendable, Equatable, Decodable {
    public let kind: UsageKind
    public let reading: UsageReading?
    public let ageSecs: UInt64?
    public let why: String?
    public let source: String

    private enum CodingKeys: String, CodingKey {
        case kind, reading, why, source
        case ageSecs = "age_secs"
    }

    public init(kind: UsageKind, reading: UsageReading?, ageSecs: UInt64?, why: String?, source: String = "") {
        self.kind = kind
        self.reading = reading
        self.ageSecs = ageSecs
        self.why = why
        self.source = source
    }
}

/// One account of a coding agent: several logins of one account are one entry.
public struct UsageAccount: Sendable, Equatable, Identifiable, Decodable {
    public let id: String
    /// The config directory its login lives in (`.claude-work`), or plan and masked email.
    public let name: String
    public let usage: UsageInfo

    public init(id: String, name: String, usage: UsageInfo) {
        self.id = id
        self.name = name
        self.usage = usage
    }
}

/// One coding agent as the usage views list it.
public struct UsageAgent: Sendable, Equatable, Identifiable, Decodable {
    public let id: String
    public let name: String
    /// The `usage.agents` setting lists it.
    public let shown: Bool
    /// Wheel can read this agent's usage; when not, `why` says why.
    public let readable: Bool
    public let why: String?
    /// One entry per account this Mac is logged in to, in name order.
    public let accounts: [UsageAccount]

    private enum CodingKeys: String, CodingKey { case id, name, shown, readable, why, accounts }

    public init(id: String, name: String, shown: Bool, readable: Bool, why: String?, accounts: [UsageAccount] = []) {
        self.id = id
        self.name = name
        self.shown = shown
        self.readable = readable
        self.why = why
        self.accounts = accounts
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        shown = try c.decode(Bool.self, forKey: .shown)
        readable = try c.decode(Bool.self, forKey: .readable)
        why = try c.decodeIfPresent(String.self, forKey: .why)
        accounts = try c.decodeIfPresent([UsageAccount].self, forKey: .accounts) ?? []
    }

    /// What the strip says in place of accounts: why there is none. Nil when there is one.
    public var missingNote: String? { accounts.isEmpty ? (why ?? "no reading") : nil }
}

/// One row of limit: the Usage window draws a bar for each, and a pin puts it in the strip.
public struct UsageRow: Sendable, Equatable, Identifiable {
    /// `<agent>:<limit>`, the id the `usage.pins` setting holds.
    public let id: String
    /// "Session", "Weekly", "Weekly Opus".
    public let label: String
    /// "5h", "wk", "opus": the strip's tag.
    public let tag: String
    public let pct: Double
    public let resetsAt: String?
}

extension UsageAccount {
    /// The account's limits under `agent`'s pin ids, session first, then weekly, then the
    /// per-model ones; empty without a reading.
    public func rows(agent: String) -> [UsageRow] {
        guard let r = usage.reading else { return [] }
        var out = [
            UsageRow(id: "\(agent):session", label: "Session", tag: "5h", pct: r.sessionPct, resetsAt: r.sessionResetsAt),
            UsageRow(id: "\(agent):weekly", label: "Weekly", tag: "7d", pct: r.weeklyPct, resetsAt: r.weeklyResetsAt),
        ]
        out += r.modelLimits.map {
            UsageRow(id: "\(agent):\($0.id)", label: $0.label, tag: $0.id, pct: $0.pct, resetsAt: $0.resetsAt)
        }
        return out
    }

    /// The reading is old or the last read failed.
    public var isStale: Bool { usage.kind == .stale }

    /// What the strip says in place of bars: why there is no reading. Nil when there is one.
    public var missingNote: String? { usage.reading == nil ? (usage.why ?? "no reading") : nil }
}

/// Everything the strip and the Usage window show.
public struct UsageBoard: Sendable, Equatable, Decodable {
    public var agents: [UsageAgent]
    /// `<agent>:<limit>` ids in the order the strip shows them.
    public var pins: [String]

    public init(agents: [UsageAgent] = [], pins: [String] = []) {
        self.agents = agents
        self.pins = pins
    }

    private enum CodingKeys: String, CodingKey { case agents, pins }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agents = try c.decodeIfPresent([UsageAgent].self, forKey: .agents) ?? []
        pins = try c.decodeIfPresent([String].self, forKey: .pins) ?? []
    }

    /// The agents the strip and the window show.
    public var shown: [UsageAgent] { agents.filter(\.shown) }

    /// The strip's rows for `account` of `agent`: the pinned rows it has a reading for, in pin order.
    public func stripRows(for account: UsageAccount, of agent: UsageAgent) -> [UsageRow] {
        let rows = account.rows(agent: agent.id)
        return pins.compactMap { pin in rows.first { $0.id == pin } }
    }

    public func isPinned(_ row: String) -> Bool { pins.contains(row) }

    /// The `usage.pins` setting value after pinning or unpinning `row`.
    public func pins(toggling row: String) -> String {
        (isPinned(row) ? pins.filter { $0 != row } : pins + [row]).joined(separator: ",")
    }

    /// The `usage.agents` setting value after listing or unlisting `agent`.
    public func agentsSetting(toggling agent: String) -> String {
        let listed = agents.filter(\.shown).map(\.id)
        return (listed.contains(agent) ? listed.filter { $0 != agent } : listed + [agent]).joined(separator: ",")
    }
}

/// How the usage views write times and percentages.
public enum UsageText {
    /// "resets 4h", "resets 3d", "resets 25m"; nil when the time is absent or unreadable.
    public static func resets(_ iso: String?, now: Date = Date()) -> String? {
        guard let iso, let at = parse(iso) else { return nil }
        let secs = Int(at.timeIntervalSince(now))
        if secs <= 0 { return "resets now" }
        if secs < 3600 { return "resets \(max(1, secs / 60))m" }
        if secs < 86_400 { return "resets \(secs / 3600)h" }
        return "resets \(secs / 86_400)d"
    }

    /// A percentage as the bars write it: whole numbers.
    public static func percent(_ pct: Double) -> String { "\(Int(pct.rounded()))%" }

    /// "4 min", "2 h": how old a stale reading is.
    public static func age(_ secs: UInt64) -> String {
        if secs < 60 { return "\(secs) s" }
        if secs < 3600 { return "\(secs / 60) min" }
        if secs < 86_400 { return "\(secs / 3600) h" }
        return "\(secs / 86_400) d"
    }

    private static func parse(_ iso: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: iso) ?? ISO8601DateFormatter().date(from: iso)
    }
}
