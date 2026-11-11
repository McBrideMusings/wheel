import Foundation

/// How far a usage reading can be trusted, as wheeld reports it.
public enum UsageKind: String, Sendable, Equatable, Decodable {
    case fresh, stale, missing
}

/// One rate-limit window of a reading, as the provider reported it. Its id names the window in
/// `usage.meters`: `5h`, `weekly`, `weekly-fable`.
public struct UsageWindow: Sendable, Equatable, Identifiable, Decodable {
    public let id: String
    /// "5-hour", "Weekly", "Weekly Fable".
    public let label: String
    public let pct: Double
    public let resetsAt: String?
    public let lengthSecs: UInt64?

    private enum CodingKeys: String, CodingKey {
        case id, label, pct
        case resetsAt = "resets_at"
        case lengthSecs = "length_secs"
    }

    public init(id: String, label: String, pct: Double, resetsAt: String? = nil, lengthSecs: UInt64? = nil) {
        self.id = id
        self.label = label
        self.pct = pct
        self.resetsAt = resetsAt
        self.lengthSecs = lengthSecs
    }

    /// The share of the window that has passed by its reset time, 0 to 1; nil without a length
    /// or a reset time. A reset time already past is 1.
    public func pace(now: Date = Date()) -> Double? {
        UsageText.pace(resetsAt: resetsAt, lengthSecs: lengthSecs, now: now)
    }
}

/// One reading of an account's usage: every window the provider's answer carried. Percentages are
/// of the plan's limit.
public struct UsageReading: Sendable, Equatable, Decodable {
    public let windows: [UsageWindow]
    /// The plan's name ("Max 20x", "Plus") or the billing kind ("API billing", "Bedrock").
    public let billing: String?
    public let extraUsage: Bool?
    public let email: String?

    private enum CodingKeys: String, CodingKey {
        case windows, billing, email
        case extraUsage = "extra_usage"
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        windows = try c.decodeIfPresent([UsageWindow].self, forKey: .windows) ?? []
        billing = try c.decodeIfPresent(String.self, forKey: .billing)
        extraUsage = try c.decodeIfPresent(Bool.self, forKey: .extraUsage)
        email = try c.decodeIfPresent(String.self, forKey: .email)
    }

    public init(windows: [UsageWindow], billing: String? = nil, extraUsage: Bool? = nil, email: String? = nil) {
        self.windows = windows
        self.billing = billing
        self.extraUsage = extraUsage
        self.email = email
    }

    /// The window with `id`, when the reading has it.
    public func window(_ id: String) -> UsageWindow? { windows.first { $0.id == id } }
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
    /// Every config directory whose login belongs to this account.
    public let dirs: [String]
    public let usage: UsageInfo

    public init(id: String, name: String, dirs: [String] = [], usage: UsageInfo) {
        self.id = id
        self.name = name
        self.dirs = dirs
        self.usage = usage
    }

    private enum CodingKeys: String, CodingKey { case id, name, dirs, usage }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        dirs = try c.decodeIfPresent([String].self, forKey: .dirs) ?? []
        usage = try c.decode(UsageInfo.self, forKey: .usage)
    }

    /// The reading is old or the last read failed.
    public var isStale: Bool { usage.kind == .stale }

    /// What the strip says in place of bars: why there is no reading. Nil when there is one.
    public var missingNote: String? { usage.reading == nil ? (usage.why ?? "no reading") : nil }
}

/// The folder an agent pane takes its login from and the usage account read from it.
public struct PaneLogin: Sendable, Equatable, Decodable {
    public let dir: String
    /// nil while no account's login lives in `dir`.
    public let account: UsageAccount?

    public init(dir: String, account: UsageAccount?) {
        self.dir = dir
        self.account = account
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

/// How much of a window the strip draws: the `usage.meters` level. A window with no meter is off.
public enum UsageMeterLevel: String, Sendable, Equatable, Decodable, CaseIterable {
    /// The bar and its percentage.
    case meter
    /// The bar, its percentage, the pace tick and the reset time.
    case full
}

/// One `usage.meters` entry: a window of every login of one harness, and how much of it to draw.
public struct UsageMeter: Sendable, Equatable, Decodable {
    public let agent: String
    public let window: String
    public let level: UsageMeterLevel

    public init(agent: String, window: String, level: UsageMeterLevel) {
        self.agent = agent
        self.window = window
        self.level = level
    }

    /// The setting's text for this entry.
    public var text: String { "\(agent):\(window):\(level.rawValue)" }
}

/// One row the strip draws for an account: a metered window and its level.
public struct UsageStripRow: Sendable, Equatable, Identifiable {
    public let window: UsageWindow
    public let level: UsageMeterLevel

    public var id: String { window.id }
}

/// The labels `usage.labels` may name, in the order Settings lists them.
public enum UsageLabel: String, Sendable, CaseIterable {
    case billing, folder, email

    public var title: String {
        switch self {
        case .billing: return "Billing"
        case .folder: return "Login folder"
        case .email: return "Account email"
        }
    }
}

/// Everything the strip and the Usage window show.
public struct UsageBoard: Sendable, Equatable, Decodable {
    public var agents: [UsageAgent]
    /// The `usage.meters` entries in the order the strip draws them.
    public var meters: [UsageMeter]
    /// The `usage.labels` entries.
    public var labels: [String]

    public init(agents: [UsageAgent] = [], meters: [UsageMeter] = [], labels: [String] = []) {
        self.agents = agents
        self.meters = meters
        self.labels = labels
    }

    private enum CodingKeys: String, CodingKey { case agents, meters, labels }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agents = try c.decodeIfPresent([UsageAgent].self, forKey: .agents) ?? []
        meters = try c.decodeIfPresent([UsageMeter].self, forKey: .meters) ?? []
        labels = try c.decodeIfPresent([String].self, forKey: .labels) ?? []
    }

    /// The agents the strip and the window show.
    public var shown: [UsageAgent] { agents.filter(\.shown) }

    /// The level `usage.meters` gives `window` of `agent`; nil when the window is off.
    public func level(agent: String, window: String) -> UsageMeterLevel? {
        meters.first { $0.agent == agent && $0.window == window }?.level
    }

    /// The strip's rows for `account` of `agent`: its metered windows in meter order. A metered
    /// window the reading lacks draws nothing; nothing stands in for it.
    public func stripRows(for account: UsageAccount, of agent: UsageAgent) -> [UsageStripRow] {
        guard let reading = account.usage.reading else { return [] }
        return meters.filter { $0.agent == agent.id }.compactMap { meter in
            reading.window(meter.window).map { UsageStripRow(window: $0, level: meter.level) }
        }
    }

    /// The `usage.meters` setting value after giving `window` of `agent` the level `level`: a
    /// changed entry keeps its place in the strip, a new one goes last, nil removes it (off).
    public func metersSetting(agent: String, window: String, level: UsageMeterLevel?) -> String {
        var out = meters
        if let at = out.firstIndex(where: { $0.agent == agent && $0.window == window }) {
            if let level { out[at] = UsageMeter(agent: agent, window: window, level: level) } else { out.remove(at: at) }
        } else if let level {
            out.append(UsageMeter(agent: agent, window: window, level: level))
        }
        return out.map(\.text).joined(separator: ",")
    }

    public func shows(_ label: UsageLabel) -> Bool { labels.contains(label.rawValue) }

    /// The `usage.labels` setting value after turning `label` on or off.
    public func labelsSetting(toggling label: UsageLabel) -> String {
        (shows(label) ? labels.filter { $0 != label.rawValue } : labels + [label.rawValue]).joined(separator: ",")
    }

    /// Whether the strip prints `agent`'s login folders: the folder label is on and the agent has
    /// two or more logins on the board.
    public func printsFolder(of agent: UsageAgent) -> Bool { shows(.folder) && agent.accounts.count >= 2 }

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

    /// The reset time as a clock: "7:00 PM" within the next day, else "Tue 7:00 PM"; nil when
    /// absent or unreadable.
    public static func resetClock(_ iso: String?, now: Date = Date(), calendar: Calendar = .current) -> String? {
        guard let iso, let at = parse(iso) else { return nil }
        let sameDay = at.timeIntervalSince(now) < 86_400
        return clockLock.withLock {
            let formatter = sameDay ? clockToday : clockLater
            if formatter.calendar != calendar {
                formatter.calendar = calendar
                formatter.timeZone = calendar.timeZone
                formatter.locale = calendar.locale ?? .current
                formatter.setLocalizedDateFormatFromTemplate(sameDay ? "jmm" : "EEE jmm")
            }
            // The formatter puts a narrow no-break space before AM/PM; the strip wants a plain one.
            return formatter.string(from: at).replacingOccurrences(of: "\u{202F}", with: " ")
        }
    }

    /// Formatters are costly to make, so the views share these. Foundation's date formatters
    /// are thread-safe; the lock covers re-setting a clock formatter's calendar, which only the
    /// tests do. Each is set up for a calendar on first use and again only when it changes.
    private static let clockLock = NSLock()
    nonisolated(unsafe) private static let clockToday = DateFormatter()
    nonisolated(unsafe) private static let clockLater = DateFormatter()
    nonisolated(unsafe) private static let isoFractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    nonisolated(unsafe) private static let isoPlain = ISO8601DateFormatter()

    /// The share of a window that has passed by `now`, from its reset time and length: 0 at the
    /// window's start, 1 at its reset; nil without a length or a reset time.
    public static func pace(resetsAt: String?, lengthSecs: UInt64?, now: Date = Date()) -> Double? {
        guard let resetsAt, let at = parse(resetsAt), let lengthSecs, lengthSecs > 0 else { return nil }
        let left = at.timeIntervalSince(now)
        let elapsed = Double(lengthSecs) - left
        return min(max(elapsed / Double(lengthSecs), 0), 1)
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
        isoFractional.date(from: iso) ?? isoPlain.date(from: iso)
    }
}
