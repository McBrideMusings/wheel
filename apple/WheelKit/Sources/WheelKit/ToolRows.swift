import Foundation

/// What a tool row shows: an icon for the kind of action, a sentence, the lines it added and
/// removed, and for a failed call the reason. Built from the row's fields alone, so the Mac app,
/// the iOS app and `/state` agree.
public struct ToolRowInfo: Equatable, Sendable {
    /// The SF Symbol for the kind of action.
    public var symbol: String
    /// The row as a sentence: `Read model.go`, `Searched for addSuggestions in model_test.go`.
    public var label: String
    /// The lines the call added and removed across its diffs; nil when it carries none.
    public var added: Int?
    public var removed: Int?
    /// A failed call's first line of output, cut short; nil otherwise.
    public var reason: String?

    public init(title: String, kind: String, status: String, content: [ToolContent], detail: String? = nil) {
        let diffs = content.compactMap { item -> (path: String, old: String?, new: String)? in
            if case let .diff(path, old, new) = item { (path, old, new) } else { nil }
        }
        let creates = diffs.first.map { $0.old == nil } ?? false
        symbol = Self.symbol(kind: kind, creates: creates)
        label = Self.label(title: title, kind: kind, creates: creates, described: detail != nil)
        if diffs.isEmpty || status == "failed" {
            added = nil
            removed = nil
        } else {
            var a = 0, r = 0
            for diff in diffs {
                let counts = LineDiff.counts(old: diff.old, new: diff.new)
                a += counts.added
                r += counts.removed
            }
            added = a
            removed = r
        }
        reason = status == "failed" ? Self.reason(content) : nil
    }

    static func symbol(kind: String, creates: Bool) -> String {
        switch kind {
        case "read": "doc.text"
        case "search": "magnifyingglass"
        case "edit": creates ? "doc.badge.plus" : "pencil"
        case "delete": "trash"
        case "move": "arrow.right.doc.on.clipboard"
        case "execute": "terminal"
        case "skill": "wand.and.stars"
        case "subagent": "person.2"
        case "fetch": "globe"
        case "mcp": "puzzlepiece.extension"
        case "think": "brain"
        default: "wrench"
        }
    }

    /// The sentence alone, with no line counting: what a title that only needs the words asks for.
    public static func sentence(title: String, kind: String, content: [ToolContent], detail: String?) -> String {
        let creates = content.contains { if case let .diff(_, old, _) = $0 { old == nil } else { false } }
        return label(title: title, kind: kind, creates: creates, described: detail != nil)
    }

    static func label(title: String, kind: String, creates: Bool, described: Bool) -> String {
        func rest(_ prefix: String) -> String? { title.hasPrefix(prefix) ? String(title.dropFirst(prefix.count)) : nil }
        func name(_ path: String) -> String { path.split(separator: "/").last.map(String.init) ?? path }
        switch kind {
        case "execute": return described ? title : "Ran \(title)"
        case "read": return rest("Read ").map { "Read \(name($0))" } ?? title
        case "edit":
            if let path = rest("Edit ") { return "Edited \(name(path))" }
            if let path = rest("Write ") { return "\(creates ? "Created" : "Edited") \(name(path))" }
            return title
        case "search":
            if let pattern = rest("grep ") {
                // `grep <pattern> in <path>`: the path is named by its last component.
                if let at = pattern.range(of: " in ", options: .backwards) {
                    return "Searched for \(pattern[..<at.lowerBound]) in \(name(String(pattern[at.upperBound...])))"
                }
                return "Searched for \(pattern)"
            }
            if let pattern = rest("Find ") { return "Searched for files matching \(pattern)" }
            return title
        case "fetch":
            if let url = rest("Fetch ") { return "Fetched \(url)" }
            if let query = rest("Search ") { return "Searched the web for \(query)" }
            return title
        case "skill": return rest("Skill ").map { "Used the \($0) skill" } ?? title
        case "mcp":
            // `mcp__server__tool` reads `server: tool`.
            let parts = title.components(separatedBy: "__")
            return parts.count >= 3 && parts[0] == "mcp" ? "\(parts[1]): \(parts[2...].joined(separator: "__"))" : title
        default: return title
        }
    }

    /// The first non-empty line of a failed call's output, at most 80 characters.
    static func reason(_ content: [ToolContent]) -> String? {
        for case let .text(text) in content {
            guard let line = text.split(whereSeparator: \.isNewline).map({ $0.trimmingCharacters(in: .whitespaces) }).first(where: { !$0.isEmpty }) else { continue }
            return line.count > 80 ? String(line.prefix(79)) + "…" : line
        }
        return nil
    }
}

extension LineDiff {
    private final class Entry { let added: Int, removed: Int; init(_ a: Int, _ r: Int) { added = a; removed = r } }
    nonisolated(unsafe) private static let cache: NSCache<NSString, Entry> = {
        let cache = NSCache<NSString, Entry>()
        cache.countLimit = 512
        return cache
    }()

    /// The lines added and removed going from `old` to `new`. A row redraws as its call streams, so
    /// the line-by-line table runs once per pair of texts.
    public static func counts(old: String?, new: String) -> (added: Int, removed: Int) {
        let key = "\(old.map { String($0.hashValue) } ?? "-"):\(new.hashValue):\(old?.utf8.count ?? -1):\(new.utf8.count)" as NSString
        if let hit = cache.object(forKey: key) { return (hit.added, hit.removed) }
        let changes = changes(old: old, new: new)
        let added = changes.filter(\.added).count
        cache.setObject(Entry(added, changes.count - added), forKey: key)
        return (added, changes.count - added)
    }
}
