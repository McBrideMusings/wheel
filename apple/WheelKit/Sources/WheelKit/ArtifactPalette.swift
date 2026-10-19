import Foundation

/// One artifact as `artifact_ls` lists it (proto/src/lib.rs `ArtifactInfo`). The daemon keeps the
/// folder and the page to itself; a client picks by scope and name.
public struct ArtifactInfo: Sendable, Equatable, Identifiable, Decodable {
    public var id: String { "\(scope.rawValue)/\(name)" }
    public let scope: ArtifactScope
    public let name: String
    /// The entry's `title`, or the name when it declares none.
    public let title: String
    /// Whether the entry says `pane = true`.
    public let pane: Bool
    /// What is wrong with it, one line each. A problem artifact still opens: the pane serves what
    /// its root holds (ADR-0004).
    public let problems: [String]

    public init(scope: ArtifactScope, name: String, title: String? = nil, pane: Bool = true, problems: [String] = []) {
        self.scope = scope
        self.name = name
        self.title = title ?? name
        self.pane = pane
        self.problems = problems
    }

    private enum CodingKeys: String, CodingKey { case scope, name, title, pane, problems }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        scope = try c.decode(ArtifactScope.self, forKey: .scope)
        name = try c.decode(String.self, forKey: .name)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? name
        pane = try c.decodeIfPresent(Bool.self, forKey: .pane) ?? false
        problems = try c.decodeIfPresent([String].self, forKey: .problems) ?? []
    }

    /// The artifact a pane opens when this row is chosen.
    public var ref: ArtifactRef { ArtifactRef(scope: scope, name: name) }

    /// The title is a line of its own only when it says something the name does not.
    public var hasTitle: Bool { title != name }
}

/// The artifact palette inside one pane: what it lists, what is typed into its field and which row
/// is selected. Pure state, so the window draws it and the debug endpoint reads it back as one.
public struct ArtifactPalette: Sendable, Equatable {
    public enum Listing: Sendable, Equatable {
        /// `artifact_ls` is out and unanswered.
        case loading
        /// The daemon's `pane = true` artifacts for this pane's space.
        case ready([ArtifactInfo])
        /// The daemon refused; its message, verbatim.
        case failed(String)
    }

    /// One listed row, with the characters of its title and name the query matched.
    public struct Row: Sendable, Equatable, Identifiable {
        public var id: String { info.id }
        public let info: ArtifactInfo
        public let titleMatch: [Int]
        public let nameMatch: [Int]
    }

    public struct Key: Sendable, Equatable {
        public let symbol: String
        public let word: String
        public let enabled: Bool
    }

    public private(set) var listing: Listing = .loading
    public private(set) var query = ""
    /// Index into `rows`; 0 when there are none.
    public private(set) var selected = 0
    /// Every row the query keeps: project first, then machine, each by name.
    public private(set) var rows: [Row] = []

    public init() {}

    public static let placeholder = "Search artifacts by title or name"
    public static let noArtifactsField = "No artifacts to search"
    public static let unavailableField = "Search unavailable"
    public static let emptyTitle = "No artifacts for this space"
    public static let emptyHelp = "An artifact is offered here when its wheel.toml entry says pane = true."
    public static let emptyAsk = "Ask the lead to make one, or run"
    public static let emptyCommand = "wheel artifact new dash --pane"
    public static let failedTitle = "Couldn’t list artifacts"
    public static let failedLead = "wheeld refused the request and said:"
    public static let retry = "Try Again"

    /// Every artifact the daemon listed, whatever the query.
    public var all: [ArtifactInfo] {
        if case let .ready(list) = listing { list } else { [] }
    }

    public var failure: String? {
        if case let .failed(message) = listing { message } else { nil }
    }

    public var isLoading: Bool { listing == .loading }

    /// The field takes typing only when there is something to search.
    public var searchable: Bool { isLoading || !all.isEmpty }

    public var fieldPlaceholder: String {
        switch listing {
        case .loading: Self.placeholder
        case .failed: Self.unavailableField
        case let .ready(list): list.isEmpty ? Self.noArtifactsField : Self.placeholder
        }
    }

    public var footer: String {
        switch listing {
        case .loading: "Listing artifacts"
        case .failed: "Listing failed"
        case let .ready(list):
            if query.isEmpty { "\(list.count) \(list.count == 1 ? "artifact" : "artifacts")" } else { "\(rows.count) of \(list.count)" }
        }
    }

    public var noMatch: Bool {
        if case let .ready(list) = listing { !list.isEmpty && rows.isEmpty } else { false }
    }

    public var noMatchDetail: String { "Searched the titles and names of \(all.count) artifacts." }

    /// The footer's key hints, each with whether pressing it does anything now.
    public var keys: [Key] {
        switch listing {
        case .loading:
            return [Key(symbol: "esc", word: "close", enabled: true)]
        case .failed:
            return [Key(symbol: "↩", word: "try again", enabled: true), Key(symbol: "esc", word: "close", enabled: true)]
        case let .ready(list):
            // With nothing listed, Return has nothing to open.
            if list.isEmpty { return [Key(symbol: "↩", word: "open", enabled: false), Key(symbol: "esc", word: "close", enabled: true)] }
            return [Key(symbol: "↑↓", word: "move", enabled: true), Key(symbol: "↩", word: "open", enabled: !rows.isEmpty),
                    Key(symbol: "esc", word: "close", enabled: true)]
        }
    }

    /// The row ↩ opens, if any.
    public var selectedRow: Row? { rows.indices.contains(selected) ? rows[selected] : nil }

    public var projectCount: Int { rows.filter { $0.info.scope == .project }.count }
    public var machineCount: Int { rows.filter { $0.info.scope == .machine }.count }

    public mutating func listed(_ list: [ArtifactInfo]) {
        listing = .ready(list)
        refilter()
    }

    public mutating func failed(_ message: String) {
        listing = .failed(message)
        rows = []
        selected = 0
    }

    public mutating func relisting() {
        listing = .loading
        rows = []
        selected = 0
    }

    /// Typing resets the selection to the first row of the first non-empty group.
    public mutating func type(_ text: String) {
        guard text != query else { return }
        query = text
        refilter()
    }

    /// ↑ and ↓ stop at the first and last row.
    public mutating func move(_ by: Int) {
        guard !rows.isEmpty else { return }
        selected = min(rows.count - 1, max(0, selected + by))
    }

    public mutating func select(_ id: String) {
        if let i = rows.firstIndex(where: { $0.id == id }) { selected = i }
    }

    /// Esc: the query first, then the palette. Returns whether the palette should close.
    public mutating func escape() -> Bool {
        if query.isEmpty { return true }
        type("")
        return false
    }

    private mutating func refilter() {
        rows = Self.filter(all, query)
        selected = 0
    }

    /// Case-insensitive substring over title and name. Scattered-letter matching runs only when no
    /// row has a substring hit.
    public static func filter(_ list: [ArtifactInfo], _ query: String) -> [Row] {
        let ordered = list.sorted { a, b in
            if a.scope != b.scope { return a.scope == .project }
            return a.name < b.name
        }
        guard !query.isEmpty else { return ordered.map { Row(info: $0, titleMatch: [], nameMatch: []) } }
        for scattered in [false, true] {
            let hits: [Row] = ordered.compactMap { info in
                let title = info.hasTitle ? match(query, in: info.title, scattered: scattered) : nil
                let name = match(query, in: info.name, scattered: scattered)
                if title == nil, name == nil { return nil }
                return Row(info: info, titleMatch: title ?? [], nameMatch: name ?? [])
            }
            if !hits.isEmpty { return hits }
        }
        return []
    }

    /// The character offsets of `text` the query matched, or nil.
    static func match(_ query: String, in text: String, scattered: Bool) -> [Int]? {
        let chars = Array(text.lowercased())
        let want = Array(query.lowercased())
        // Lowercasing can change the length (`İ`); a match then points at the wrong characters.
        guard chars.count == text.count else { return nil }
        if let start = firstRange(of: want, in: chars) { return Array(start ..< start + want.count) }
        guard scattered else { return nil }
        var out: [Int] = []
        var from = 0
        for c in want {
            guard let i = chars[from...].firstIndex(of: c) else { return nil }
            out.append(i)
            from = i + 1
        }
        return out
    }

    private static func firstRange(of want: [Character], in chars: [Character]) -> Int? {
        guard !want.isEmpty, want.count <= chars.count else { return nil }
        for start in 0 ... chars.count - want.count where Array(chars[start ..< start + want.count]) == want { return start }
        return nil
    }
}
