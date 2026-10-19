import Foundation

extension ConversationInfo {
    /// The longest title a row shows before it is cut: a title is the conversation's first prompt,
    /// which can be a whole paragraph.
    public static let titleLimit = 90

    /// The first line of the title, cut to `titleLimit`; a conversation with none is untitled.
    public var rowTitle: String {
        let line = (title ?? "").split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
        if line.isEmpty { return "Untitled conversation" }
        return line.count > Self.titleLimit ? String(line.prefix(Self.titleLimit - 1)) + "…" : line
    }
}

/// The lead's conversation picker: the listing the agent returned, what is typed into the search
/// field and which row is selected. Pure state, so the window draws it and the debug endpoint reads
/// it back as one. Filtering works on a lowercased key built once per listing, so a keystroke over
/// 10,000 conversations is one substring test per row and no allocation.
public struct ConversationPicker: Sendable, Equatable {
    public private(set) var all: [ConversationInfo] = []
    private var keys: [String] = []
    /// Indexes into `all` that the query keeps, in listing order (newest first).
    public private(set) var rows: [Int] = []
    public private(set) var query = ""
    /// Index into `rows`; 0 when there are none.
    public private(set) var selected = 0
    /// A listing request is out and unanswered.
    public private(set) var isLoading = true
    /// Why the last listing failed; the rows from before stay.
    public private(set) var error: String?
    /// Milliseconds from opening to the list's first row on screen; nil until it paints.
    public var firstPaintMs: Int?

    public init() {}

    public static let placeholder = "Search by title or first message"
    public static let loadingNote = "Loading conversations…"
    public static let emptyNote = "No past conversations"
    public static let emptyDetail = "Conversations you have with the lead appear here."

    /// Index into `all` of the selected row.
    public var selectedIndex: Int? { rows.indices.contains(selected) ? rows[selected] : nil }

    public var selectedConversation: ConversationInfo? { rows.indices.contains(selected) ? all[rows[selected]] : nil }

    /// What the footer counts.
    public var footer: String {
        if isLoading && all.isEmpty { return "Listing conversations" }
        let noun = all.count == 1 ? "conversation" : "conversations"
        return query.isEmpty ? "\(all.count) \(noun)" : "\(rows.count) of \(all.count)"
    }

    public mutating func relisting() { isLoading = true }

    public mutating func listed(_ list: [ConversationInfo]) {
        let keep = selectedConversation?.id
        all = list
        keys = list.map { Self.key($0) }
        isLoading = false
        error = nil
        refilter(keeping: keep)
    }

    public mutating func failed(_ message: String) {
        isLoading = false
        error = message
    }

    public mutating func type(_ text: String) {
        guard text != query else { return }
        query = text
        refilter(keeping: nil)
    }

    /// ↑ and ↓ stop at the first and last row.
    public mutating func move(_ by: Int) {
        guard !rows.isEmpty else { return }
        selected = min(rows.count - 1, max(0, selected + by))
    }

    public mutating func select(_ id: String) {
        if let i = rows.firstIndex(where: { all[$0].id == id }) { selected = i }
    }

    private mutating func refilter(keeping id: String?) {
        let needle = query.lowercased()
        if needle.isEmpty {
            rows = Array(all.indices)
        } else {
            rows = all.indices.filter { keys[$0].contains(needle) }
        }
        selected = id.flatMap { id in rows.firstIndex { all[$0].id == id } } ?? 0
    }

    /// Title and first message are the same text: the agent titles a conversation with its first
    /// prompt, so one key covers both.
    private static func key(_ c: ConversationInfo) -> String { (c.title ?? "").lowercased() }
}
