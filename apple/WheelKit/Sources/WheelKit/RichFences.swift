import Foundation

/// One line of a ```diff or ```patch fence.
public struct DiffFenceLine: Equatable, Sendable {
    public enum Kind: Sendable { case file, hunk, added, removed, context }
    public var kind: Kind
    public var text: String
}

public enum DiffFence {
    /// Whether a fence's language hint names a diff.
    public static func handles(_ language: String?) -> Bool {
        guard let language = language?.lowercased() else { return false }
        return language == "diff" || language == "patch"
    }

    /// The lines of a unified diff, each tagged by what it is. `---`/`+++` headers and `diff --git`
    /// lines are file headers, not removals and additions.
    public static func parse(_ text: String) -> [DiffFenceLine] {
        var lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        if lines.last == "" { lines.removeLast() }
        var inHunk = false
        return lines.map { line in
            if line.hasPrefix("diff ") { inHunk = false }
            if inHunk, line.hasPrefix("+") || line.hasPrefix("-") || line.hasPrefix(" ") {
                return DiffFenceLine(kind: line.hasPrefix("+") ? .added : line.hasPrefix("-") ? .removed : .context, text: line)
            }
            if line.hasPrefix("@@") { inHunk = true }
            if line.hasPrefix("diff ") || line.hasPrefix("index ") || line.hasPrefix("--- ") || line.hasPrefix("+++ ")
                || line.hasPrefix("new file mode") || line.hasPrefix("deleted file mode") || line.hasPrefix("similarity index")
                || line.hasPrefix("rename ") {
                return DiffFenceLine(kind: .file, text: line)
            }
            if line.hasPrefix("@@") { return DiffFenceLine(kind: .hunk, text: line) }
            if line.hasPrefix("+") { return DiffFenceLine(kind: .added, text: line) }
            if line.hasPrefix("-") { return DiffFenceLine(kind: .removed, text: line) }
            return DiffFenceLine(kind: .context, text: line)
        }
    }
}

public enum SVGFence {
    /// Whether a fence's language hint names SVG.
    public static func handles(_ language: String?) -> Bool { language?.lowercased() == "svg" }

    /// `source` with everything that runs or fetches removed: `<script>`, `<foreignObject>`,
    /// `<iframe>`, `<embed>`, `<object>`, `<audio>`, `<video>`, `<animate>`/`<set>` (they can
    /// rewrite an `href`), the DOCTYPE and entities, `on…` event attributes, and any `href`,
    /// `src`, `xlink:href` or CSS `url(…)` that points anywhere but inside the document (`#id`) or
    /// at an inline `data:image/` payload. Nil when the text holds no `<svg` element.
    public static func sanitize(_ source: String) -> String? {
        guard source.range(of: "<svg", options: .caseInsensitive) != nil else { return nil }
        // One pass can rebuild what it removed (`<scr<script></script>ipt>`), so repeat until stable.
        var text = source
        for _ in 0..<8 {
            let next = pass(text)
            if next == text { break }
            text = next
        }
        return text
    }

    private static func pass(_ source: String) -> String {
        var text = source
        let blocks = ["script", "foreignObject", "iframe", "embed", "object", "audio", "video", "style"]
        for tag in blocks {
            var before = ""
            while before != text {
                before = text
                text = replace(text, "<(?:[\\w.-]+:)?\(tag)\\b[^>]*>.*?</(?:[\\w.-]+:)?\(tag)\\s*>", "")
            }
            text = replace(text, "</?(?:[\\w.-]+:)?\(tag)\\b[^>]*/?>", "")
        }
        for tag in ["animate", "animateTransform", "animateMotion", "set"] {
            var before = ""
            while before != text {
                before = text
                text = replace(text, "<(?:[\\w.-]+:)?\(tag)\\b[^>]*>.*?</(?:[\\w.-]+:)?\(tag)\\s*>", "")
            }
            text = replace(text, "</?(?:[\\w.-]+:)?\(tag)\\b[^>]*/?>", "")
        }
        text = replace(text, "<!DOCTYPE[^>]*(\\[.*?\\])?\\s*>", "")
        text = replace(text, "<!ENTITY[^>]*>", "")
        text = replace(text, "<\\?.*?\\?>", "")
        text = replace(text, "\\s+on[a-zA-Z]+\\s*=\\s*(\"[^\"]*\"|'[^']*'|[^\\s>]+)", "")
        for name in ["href", "xlink:href", "src"] {
            text = strip(text, attribute: name)
        }
        // An <image> whose reference was removed would draw as a broken-image mark.
        text = replace(text, "<image\\b(?![^>]*\\bhref\\s*=)[^>]*?/>", "")
        text = replace(text, "<image\\b(?![^>]*\\bhref\\s*=)[^>]*>.*?</image\\s*>", "")
        text = replace(text, "url\\(\\s*(?!['\"]?#)[^)]*\\)", "none")
        text = replace(text, "@import[^;]*;?", "")
        text = replace(text, "\\\\[0-9a-fA-F]{1,6}\\s?", "")
        return text
    }

    /// Removes `name="value"` unless the value is an in-document `#id` reference or a `data:image/`
    /// payload that is not itself SVG (an SVG payload could carry script of its own).
    private static func strip(_ text: String, attribute name: String) -> String {
        let pattern = "\\s+\(NSRegularExpression.escapedPattern(for: name))\\s*=\\s*(\"([^\"]*)\"|'([^']*)')"
        guard let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive, .dotMatchesLineSeparators]) else { return text }
        let result = NSMutableString(string: text)
        let source = text as NSString
        for match in regex.matches(in: text, range: NSRange(location: 0, length: source.length)).reversed() {
            let group = match.range(at: 2).location != NSNotFound ? match.range(at: 2) : match.range(at: 3)
            let value = source.substring(with: group).trimmingCharacters(in: .whitespaces).lowercased()
            let local = value.hasPrefix("#")
            let inlineRaster = value.hasPrefix("data:image/") && !value.hasPrefix("data:image/svg")
            if !local && !inlineRaster { result.deleteCharacters(in: match.range) }
        }
        return result as String
    }

    nonisolated(unsafe) private static var cache: [String: NSRegularExpression] = [:]
    private static let lock = NSLock()

    private static func compiled(_ pattern: String) -> NSRegularExpression? {
        lock.lock(); defer { lock.unlock() }
        if let hit = cache[pattern] { return hit }
        let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive, .dotMatchesLineSeparators])
        cache[pattern] = regex
        return regex
    }

    private static func replace(_ text: String, _ pattern: String, _ with: String) -> String {
        guard let regex = compiled(pattern) else { return text }
        return regex.stringByReplacingMatches(in: text, range: NSRange(text.startIndex..., in: text), withTemplate: with)
    }
}
