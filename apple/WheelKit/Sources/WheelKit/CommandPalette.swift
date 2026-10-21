import Foundation

/// One built-in action the palette offers. The app's registry makes these; the palette lists them
/// and hands the chosen `id` back.
public struct PaletteAction: Sendable, Equatable {
    /// The registry id that opens a folder picker; the palette lists it as `.openFolder`.
    public static let openFolderID = "open-folder"

    public let id: String
    public let title: String
    /// The key combination as the menus print it, or nil.
    public let shortcut: String?

    public init(id: String, title: String, shortcut: String? = nil) {
        self.id = id
        self.title = title
        self.shortcut = shortcut
    }
}

/// What a palette row runs.
public enum PaletteItem: Sendable, Equatable, Identifiable {
    case action(id: String, title: String, shortcut: String?)
    /// A space that is open now.
    case space(id: String, name: String, dir: String)
    /// A directory a space was made for and is not open now.
    case recent(dir: String)
    /// Ask for a folder with the system picker.
    case openFolder

    public var id: String {
        switch self {
        case let .action(id, _, _): "action:\(id)"
        case let .space(id, _, _): "space:\(id)"
        case let .recent(dir): "recent:\(dir)"
        case .openFolder: "action:\(PaletteAction.openFolderID)"
        }
    }

    /// `action`, `space`, `recent` or `open_folder`: what the log and the debug state call it.
    public var kind: String {
        switch self {
        case .action: "action"
        case .space: "space"
        case .recent: "recent"
        case .openFolder: "open_folder"
        }
    }

    public var title: String {
        switch self {
        case let .action(_, title, _): title
        case let .space(_, name, _): name
        case let .recent(dir): CommandPalette.lastComponent(of: dir)
        case .openFolder: "Open Folder…"
        }
    }

    /// The second line of the row: the shortcut or the path.
    public var detail: String {
        switch self {
        case let .action(_, _, shortcut): shortcut ?? ""
        case let .space(_, _, dir): dir
        case let .recent(dir): dir
        case .openFolder: ""
        }
    }
}

/// The command palette: one search field over the open spaces, recent directories and the app's
/// actions. Pure state, so the window draws it and the debug endpoint reads it back as one.
public struct CommandPalette: Sendable, Equatable {
    public struct Row: Sendable, Equatable, Identifiable {
        public var id: String { item.id }
        public let item: PaletteItem
        public let score: Int
    }

    public struct OpenSpace: Sendable, Equatable {
        public let id: String
        public let name: String
        public let dir: String

        public init(id: String, name: String, dir: String) {
            self.id = id
            self.name = name
            self.dir = dir
        }
    }

    public private(set) var query = ""
    /// Index into `rows`; 0 when there are none.
    public private(set) var selected = 0
    public private(set) var rows: [Row] = []

    private var spaces: [OpenSpace] = []
    private var recents: [RecentDir] = []
    private var actions: [PaletteAction] = []

    public init() {}

    public var selectedItem: PaletteItem? { rows.indices.contains(selected) ? rows[selected].item : nil }

    /// Replace what the palette lists from; the query stays and the selection returns to the top.
    public mutating func list(spaces: [OpenSpace], recents: [RecentDir], actions: [PaletteAction]) {
        self.spaces = spaces
        self.recents = recents
        self.actions = actions
        refilter()
    }

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

    public mutating func select(_ index: Int) {
        if rows.indices.contains(index) { selected = index }
    }

    private mutating func refilter() {
        rows = Self.filter(spaces: spaces, recents: recents, actions: actions, query: query)
        selected = 0
    }

    /// With no query: the open spaces, then the recent directories not open now (newest first),
    /// then the actions. With one: every item it matches, best score first, ties in that order.
    public static func filter(spaces: [OpenSpace], recents: [RecentDir], actions: [PaletteAction], query: String) -> [Row] {
        let open = Set(spaces.map(\.dir))
        let closed = recents.filter { !open.contains($0.dir) }.sorted { $0.openedMs > $1.openedMs }
        var items: [(item: PaletteItem, texts: [String])] = []
        items += spaces.map { (.space(id: $0.id, name: $0.name, dir: $0.dir), [$0.name, $0.dir]) }
        items += closed.map { (.recent(dir: $0.dir), [$0.dir]) }
        items += actions.map { action in
            let item: PaletteItem = action.id == PaletteAction.openFolderID
                ? .openFolder : .action(id: action.id, title: action.title, shortcut: action.shortcut)
            return (item, [action.title])
        }
        let query = query.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return items.map { Row(item: $0.item, score: 0) } }
        let scored: [Row] = items.compactMap { entry in
            let best = entry.texts.compactMap { score(query, in: $0) }.max()
            return best.map { Row(item: entry.item, score: $0) }
        }
        // `sorted` is not stable, so the original position breaks ties.
        return scored.enumerated()
            .sorted { $0.element.score != $1.element.score ? $0.element.score > $1.element.score : $0.offset < $1.offset }
            .map(\.element)
    }

    public static func lastComponent(of path: String) -> String {
        let trimmed = path.hasSuffix("/") && path.count > 1 ? String(path.dropLast()) : path
        return trimmed.split(separator: "/").last.map(String.init) ?? path
    }

    private static func isSeparator(_ c: Character) -> Bool {
        c == "/" || c == "-" || c == "_" || c == "." || c == " "
    }

    /// The best score of the query's letters appearing in order in `text`, ignoring case, or nil
    /// when they do not. Each matched letter earns 1; 8 more at a word start (the text's start,
    /// after `/ - _ .` or a space, or a capital after a lowercase letter); 5 more when it follows
    /// the previous matched letter directly; 3 more inside the path's last component. A gap
    /// between two matched letters costs 1 per skipped letter, capped at 3.
    public static func score(_ query: String, in text: String) -> Int? {
        let want = Array(query.lowercased())
        let chars = Array(text)
        let lower = Array(text.lowercased())
        // Lowercasing can change the length (`İ`); positions would then point at other letters.
        guard lower.count == chars.count, !want.isEmpty, want.count <= chars.count else { return nil }
        let lastStart = (chars.lastIndex(where: { $0 == "/" }).map { $0 + 1 }) ?? 0
        func base(_ j: Int) -> Int {
            var s = 1
            if j == 0 || isSeparator(chars[j - 1]) || (chars[j].isUppercase && chars[j - 1].isLowercase) { s += 8 }
            if j >= lastStart && lastStart > 0 { s += 3 }
            return s
        }
        // best[j]: the best score with the query's letters so far matched, the latest at text position j.
        var best = [Int?](repeating: nil, count: chars.count)
        for j in chars.indices where lower[j] == want[0] { best[j] = base(j) }
        for letter in want.dropFirst() {
            var next = [Int?](repeating: nil, count: chars.count)
            for j in chars.indices where lower[j] == letter {
                var top: Int?
                for i in 0 ..< j {
                    guard let before = best[i] else { continue }
                    let step = i == j - 1 ? 5 : -min(3, j - i - 1)
                    top = max(top ?? Int.min, before + step)
                }
                if let top { next[j] = top + base(j) }
            }
            best = next
        }
        return best.compactMap { $0 }.max()
    }
}
