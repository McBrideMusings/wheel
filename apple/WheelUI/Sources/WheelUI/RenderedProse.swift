import Foundation

/// The text a reply shows, with the Markdown syntax taken out: the characters a person sees and
/// selects, in the order they see them. Selection offsets, the highlight and a copy all count in
/// these characters, so a `**` or a `#` in the source moves nothing and never reaches the
/// pasteboard.
public struct RenderedProse: Equatable, Sendable {
    public enum Face: Equatable, Sendable {
        case plain, strong, emphasis, strongEmphasis, code
        /// A heading of this level (1 to 6).
        case heading(Int)
    }

    /// A stretch of the text drawn in one face, in UTF-16 units.
    public struct Run: Equatable, Sendable {
        public var range: Range<Int>
        public var face: Face
    }

    public let text: String
    public let runs: [Run]
    public var length: Int { text.utf16.count }

    public init(text: String, runs: [Run] = []) {
        self.text = text
        self.runs = runs
    }

    /// Text shown exactly as written, such as a person's message.
    public static func plain(_ text: String) -> RenderedProse {
        RenderedProse(text: text)
    }

    /// The Markdown source as the reply view renders it: blocks apart by a blank line, list items
    /// by a line break with their marker, code blocks as they are written.
    public static func markdown(_ source: String) -> RenderedProse {
        if let cached = cache.object(forKey: source as NSString) { return cached.value }
        let value = parse(source)
        cache.setObject(Box(value), forKey: source as NSString)
        return value
    }

    private final class Box {
        let value: RenderedProse
        init(_ value: RenderedProse) { self.value = value }
    }

    private nonisolated(unsafe) static let cache: NSCache<NSString, Box> = {
        let cache = NSCache<NSString, Box>()
        cache.countLimit = 200
        return cache
    }()

    private static func parse(_ source: String) -> RenderedProse {
        guard let parsed = try? AttributedString(markdown: source) else { return .plain(source) }
        var text = ""
        var length = 0
        var runs: [Run] = []
        var previous: [PresentationIntent.IntentType] = []
        var started = false
        for run in parsed.runs {
            let chunk = String(parsed[run.range].characters)
            let components = run.presentationIntent?.components ?? []
            // Outermost first, so the block this run sits in is last.
            let path = Array(components.reversed())
            let block = path.last
            var prefix = ""
            if started, block?.identity != previous.last?.identity {
                prefix = inSameList(path, previous) ? "\n" : "\n\n"
            }
            if block?.identity != previous.last?.identity, let marker = listMarker(path, after: previous) {
                prefix += marker
            }
            if !prefix.isEmpty {
                text += prefix
                length += prefix.utf16.count
            }
            let units = chunk.utf16.count
            if units > 0 {
                let face = face(of: run.inlinePresentationIntent, path: path)
                if face != .plain { runs.append(Run(range: length..<(length + units), face: face)) }
                text += chunk
                length += units
            }
            previous = path
            started = true
        }
        return RenderedProse(text: text, runs: runs)
    }

    /// Both blocks sit in one list, so their items follow on consecutive lines.
    private static func inSameList(_ a: [PresentationIntent.IntentType], _ b: [PresentationIntent.IntentType]) -> Bool {
        guard let listA = a.first(where: { if case .unorderedList = $0.kind { true } else if case .orderedList = $0.kind { true } else { false } }),
              let listB = b.first(where: { if case .unorderedList = $0.kind { true } else if case .orderedList = $0.kind { true } else { false } })
        else { return false }
        return listA.identity == listB.identity
    }

    /// The bullet or number a list item starts with, when the path ends in a list item's paragraph.
    private static func listMarker(_ path: [PresentationIntent.IntentType], after previous: [PresentationIntent.IntentType]) -> String? {
        guard let item = path.last(where: { if case .listItem = $0.kind { true } else { false } }),
              case let .listItem(ordinal) = item.kind,
              !previous.contains(where: { $0.identity == item.identity }) else { return nil }
        let ordered = path.contains { if case .orderedList = $0.kind { true } else { false } }
        return ordered ? "\(ordinal). " : "• "
    }

    private static func face(of inline: InlinePresentationIntent?, path: [PresentationIntent.IntentType]) -> Face {
        for intent in path.reversed() {
            if case let .header(level) = intent.kind { return .heading(level) }
            if case .codeBlock = intent.kind { return .code }
        }
        guard let inline else { return .plain }
        if inline.contains(.code) { return .code }
        switch (inline.contains(.stronglyEmphasized), inline.contains(.emphasized)) {
        case (true, true): return .strongEmphasis
        case (true, false): return .strong
        case (false, true): return .emphasis
        default: return .plain
        }
    }
}

extension RenderedProse {
    /// The word under `offset` (or ending there), for a double-click; nil on a space or a symbol.
    public func wordRange(at offset: Int) -> Range<Int>? {
        let string = text as NSString
        guard string.length > 0 else { return nil }
        let at = min(max(offset, 0), string.length)
        var words: [Range<Int>] = []
        string.enumerateSubstrings(in: NSRange(location: 0, length: string.length), options: [.byWords, .substringNotRequired]) { _, range, _, _ in
            words.append(range.location..<(range.location + range.length))
        }
        return words.first { $0.contains(at) } ?? words.first { $0.upperBound == at }
    }

    /// The block under `offset` up to its line breaks, for a triple-click: a paragraph, a list
    /// item or a line of code.
    public func paragraphRange(at offset: Int) -> Range<Int>? {
        let string = text as NSString
        guard string.length > 0 else { return nil }
        let at = min(max(offset, 0), string.length)
        let before = string.range(of: "\n", options: .backwards, range: NSRange(location: 0, length: at))
        let start = before.location == NSNotFound ? 0 : before.location + 1
        let after = string.range(of: "\n", range: NSRange(location: at, length: string.length - at))
        let end = after.location == NSNotFound ? string.length : after.location
        return start < end ? start..<end : nil
    }
}
