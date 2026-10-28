import Foundation

/// The `chat.*` settings that decide which rows a transcript folds into lines and cards.
public struct TranscriptLayout: Equatable, Sendable {
    /// `chat.tools.summary`: a line per call, per run of calls, or per turn.
    public enum ToolSummary: String, Sendable, CaseIterable { case each, runs, turn }
    /// `chat.thinking`: reasoning streams in view, shows as a "Thought for" line, or is not drawn.
    public enum Thinking: String, Sendable, CaseIterable { case live, collapsed, hidden }

    public var tools: ToolSummary = .turn
    public var thinking: Thinking = .collapsed
    /// `chat.edits.card`: a finished turn that edited files ends with a card of them.
    public var editsCard = true

    public static let standard = TranscriptLayout()

    public init() {}
}

/// One file a turn changed, with the lines that changed in it.
public struct TurnEditedFile: Equatable, Sendable {
    public var path: String
    public var added: Int
    public var removed: Int
    public var changes: [DiffLine]
}

/// One changed line of a file: removed from the old text, or added in the new.
public struct DiffLine: Equatable, Sendable {
    public var text: String
    public var added: Bool
}

public enum Edits {
    /// The files the tool calls in `rows` changed, in the order they were first changed. Several
    /// edits to one file add up to one entry. A failed call changed nothing.
    public static func files(in rows: [TranscriptRow]) -> [TurnEditedFile] {
        var files: [TurnEditedFile] = []
        for row in rows {
            guard case let .tool(_, _, status, content) = row.kind, status != "failed" else { continue }
            for case let .diff(path, old, new) in content {
                let changes = LineDiff.changes(old: old, new: new)
                if changes.isEmpty { continue }
                let added = changes.filter(\.added).count
                if let at = files.firstIndex(where: { $0.path == path }) {
                    files[at].added += added
                    files[at].removed += changes.count - added
                    files[at].changes += changes
                } else {
                    files.append(TurnEditedFile(path: path, added: added, removed: changes.count - added, changes: changes))
                }
            }
        }
        return files
    }

    /// Whether any call in `rows` changed a file; the cheap question a layout asks, with no diff computed.
    public static func any(in rows: ArraySlice<TranscriptRow>) -> Bool {
        rows.contains { row in
            guard case let .tool(_, _, status, content) = row.kind, status != "failed" else { return false }
            return content.contains { if case let .diff(_, old, new) = $0 { old != new } else { false } }
        }
    }

    /// `Edited 2 files`, `Edited 1 file`.
    public static func title(count: Int) -> String { "Edited \(count) \(count == 1 ? "file" : "files")" }
}

public enum LineDiff {
    /// Above this many cells the line-by-line table is not built and the whole old text counts as
    /// removed and the whole new text as added.
    private static let tableLimit = 4_000_000

    /// The lines that differ between `old` (nil for a new file) and `new`, in file order, with the
    /// lines both share dropped.
    public static func changes(old: String?, new: String) -> [DiffLine] {
        let a = lines(old ?? "")
        let b = lines(new)
        var start = 0
        while start < a.count, start < b.count, a[start] == b[start] { start += 1 }
        var endA = a.count, endB = b.count
        while endA > start, endB > start, a[endA - 1] == b[endB - 1] { endA -= 1; endB -= 1 }
        let x = Array(a[start..<endA]), y = Array(b[start..<endB])
        if x.count * y.count > tableLimit {
            return x.map { DiffLine(text: $0, added: false) } + y.map { DiffLine(text: $0, added: true) }
        }
        // Longest common subsequence over the middle that differs.
        var table = [[Int]](repeating: [Int](repeating: 0, count: y.count + 1), count: x.count + 1)
        for i in stride(from: x.count - 1, through: 0, by: -1) {
            for j in stride(from: y.count - 1, through: 0, by: -1) {
                table[i][j] = x[i] == y[j] ? table[i + 1][j + 1] + 1 : max(table[i + 1][j], table[i][j + 1])
            }
        }
        var out: [DiffLine] = []
        var i = 0, j = 0
        while i < x.count || j < y.count {
            if i < x.count, j < y.count, x[i] == y[j] {
                i += 1; j += 1
            } else if i < x.count, j == y.count || table[i + 1][j] >= table[i][j + 1] {
                out.append(DiffLine(text: x[i], added: false)); i += 1
            } else {
                out.append(DiffLine(text: y[j], added: true)); j += 1
            }
        }
        return out
    }

    private static func lines(_ text: String) -> [String] {
        if text.isEmpty { return [] }
        var parts = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        if text.hasSuffix("\n") { parts.removeLast() }
        return parts
    }
}

public enum ToolWords {
    /// What a run of tool calls did, from their kinds: `Ran 1 command, read 2 files`.
    public static func summary(of rows: [TranscriptRow]) -> String {
        var order: [String] = []
        var counts: [String: Int] = [:]
        var editedPaths = Set<String>()
        for row in rows {
            guard case let .tool(_, kind, _, content) = row.kind else { continue }
            let key = ["execute", "read", "edit", "delete", "move", "search", "fetch"].contains(kind) ? kind : "other"
            if counts[key] == nil { order.append(key) }
            if key == "edit" {
                let paths = content.compactMap { item -> String? in if case let .diff(path, _, _) = item { path } else { nil } }
                if paths.isEmpty { counts[key, default: 0] += 1 } else {
                    for path in paths where editedPaths.insert(path).inserted { counts[key, default: 0] += 1 }
                }
            } else {
                counts[key, default: 0] += 1
            }
        }
        let words = order.map { key -> String in
            let n = counts[key] ?? 0
            func noun(_ one: String, _ many: String) -> String { n == 1 ? one : many }
            switch key {
            case "execute": return "ran \(n) \(noun("command", "commands"))"
            case "read": return "read \(n) \(noun("file", "files"))"
            case "edit": return "edited \(n) \(noun("file", "files"))"
            case "delete": return "deleted \(n) \(noun("file", "files"))"
            case "move": return "moved \(n) \(noun("file", "files"))"
            case "search": return "searched \(n) \(noun("time", "times"))"
            case "fetch": return "fetched \(n) \(noun("page", "pages"))"
            default: return "used \(n) \(noun("tool", "tools"))"
            }
        }
        guard let first = words.first else { return "Used 0 tools" }
        return (first.prefix(1).uppercased() + first.dropFirst() + words.dropFirst().map { ", " + $0 }.joined())
    }
}
