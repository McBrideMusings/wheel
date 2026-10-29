import Foundation

/// How a reply's links behave, from the `chat.links.*` settings.
public struct ChatLinkSettings: Sendable, Equatable {
    public enum Files: String, Sendable, CaseIterable { case preview, pane, off }
    public enum FileLabel: String, Sendable, CaseIterable { case name, full }
    public enum Web: String, Sendable, CaseIterable { case pane, system }

    public var files: Files = .preview
    public var fileLabel: FileLabel = .name
    public var panes = true
    public var web: Web = .pane
    /// The links the person defined, in the order they are tried.
    public var patterns: [ChatLinkPattern] = []

    public init() {}
}

/// What a link of a person's own pattern does, with its template filled in from the match.
public enum ChatLinkPatternAction: Hashable, Sendable {
    case open(String)
    case run(String)
    case copy(String)
}

/// One entry of `chat.links.patterns`: a regular expression and what a match of it does. The
/// templates keep `$0`, `$1`… until a match fills them in.
public struct ChatLinkPattern: @unchecked Sendable, Equatable {
    public enum Action: Sendable, Equatable {
        case open(url: String)
        case run(command: String)
        case copy(text: String)
    }

    public var match: String
    public var action: Action
    let regex: NSRegularExpression

    public init?(match: String, action: Action) {
        guard let regex = try? NSRegularExpression(pattern: match) else { return nil }
        self.match = match
        self.action = action
        self.regex = regex
    }

    public static func == (a: ChatLinkPattern, b: ChatLinkPattern) -> Bool { a.match == b.match && a.action == b.action }

    /// The entries of the setting's value (a JSON list the daemon has checked). An entry this
    /// build cannot read, or whose expression it cannot compile, is left out.
    public static func parse(_ json: String) -> [ChatLinkPattern] {
        guard let data = json.data(using: .utf8),
              let list = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else { return [] }
        return list.compactMap { item in
            guard let match = item["match"] as? String, let name = item["action"] as? String else { return nil }
            switch name {
            case "open": return (item["url"] as? String).flatMap { ChatLinkPattern(match: match, action: .open(url: $0)) }
            case "run": return (item["command"] as? String).flatMap { ChatLinkPattern(match: match, action: .run(command: $0)) }
            case "copy": return ChatLinkPattern(match: match, action: .copy(text: item["text"] as? String ?? "$0"))
            default: return nil
            }
        }
    }

    /// What a match of this pattern does: the template with `$N` replaced by group N of the match.
    /// A group written into a command is quoted for the shell and one written into a URL is
    /// percent-encoded, so the text an agent wrote cannot add to either.
    func resolved(_ found: NSTextCheckingResult, in text: NSString) -> ChatLinkPatternAction {
        func fill(_ template: String, _ quote: (String) -> String) -> String {
            var out = ""
            let chars = Array(template)
            var i = 0
            while i < chars.count {
                guard chars[i] == "$", i + 1 < chars.count, chars[i + 1].isASCII, chars[i + 1].isNumber else {
                    out.append(chars[i])
                    i += 1
                    continue
                }
                var j = i + 1
                var digits = ""
                while j < chars.count, chars[j].isASCII, chars[j].isNumber { digits.append(chars[j]); j += 1 }
                if let n = Int(digits), n < found.numberOfRanges, found.range(at: n).location != NSNotFound {
                    out += quote(text.substring(with: found.range(at: n)))
                }
                i = j
            }
            return out
        }
        switch action {
        case let .open(url): return .open(fill(url, Self.percentEncoded))
        case let .run(command): return .run(fill(command, Self.shellQuoted))
        case let .copy(template): return .copy(fill(template) { $0 })
        }
    }

    private static let urlSafe = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~")
    private static func percentEncoded(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: urlSafe) ?? s
    }

    private static let shellSafe = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._/:@%+=,")
    private static func shellQuoted(_ s: String) -> String {
        // A control character typed into a terminal is a keystroke (Return, Ctrl-C), never text.
        let s = String(String.UnicodeScalarView(s.unicodeScalars.filter { $0.value >= 0x20 && $0.value != 0x7f }))
        if !s.isEmpty, s.unicodeScalars.allSatisfy({ shellSafe.contains($0) }) { return s }
        return "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}

/// What a reply's text can link to.
public enum ChatLinkTarget: Hashable, Sendable {
    /// A file as the reply wrote it: absolute, `~/`, or relative to the pane's directory.
    case file(path: String, line: Int?)
    case pane(String)
    case artifact(String)
    case web(String)
    /// A match of one of the person's own patterns.
    case pattern(ChatLinkPatternAction)
}

/// What a click on a link does, read back from the link's address.
public enum ChatLinkAction: Equatable, Sendable {
    case file(path: String, line: Int?)
    case pane(String)
    case artifact(String)
    case web(String)
    /// Run a command in a new terminal pane beside the chat's.
    case run(String)
    case copy(String)
    /// Fetch and show the web image at this address, which nothing fetched until now.
    case loadImage(String)

    static let scheme = "wheel-link"

    /// The address the rewritten Markdown carries.
    static func url(file path: String, line: Int?) -> String {
        var s = "\(scheme)://file?path=\(escape(path))"
        if let line { s += "&line=\(line)" }
        return s
    }
    static func url(pane id: String) -> String { "\(scheme)://pane?id=\(escape(id))" }
    static func url(artifact name: String) -> String { "\(scheme)://artifact?name=\(escape(name))" }
    static func url(loadImage target: String) -> String { "\(scheme)://image?url=\(escape(target))&key=\(key)" }
    static func url(run command: String) -> String { "\(scheme)://run?command=\(escape(command))&key=\(key)" }
    static func url(copy text: String) -> String { "\(scheme)://copy?text=\(escape(text))&key=\(key)" }

    /// Known only to this process. A run or copy link carries it, so text a reply wrote itself
    /// (`[x](wheel-link://run?command=…)`) can never type into a shell or fill the clipboard.
    private static let key = UUID().uuidString

    private static let unreserved = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~/")
    private static func escape(_ s: String) -> String {
        s.addingPercentEncoding(withAllowedCharacters: unreserved) ?? s
    }

    /// The action a link's address names; nil for an address that is none of ours or a web link.
    public init?(url: URL) {
        let scheme = url.scheme?.lowercased()
        if scheme == "http" || scheme == "https" {
            self = .web(url.absoluteString)
            return
        }
        guard scheme == Self.scheme, let host = url.host,
              let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems
        else { return nil }
        func value(_ key: String) -> String? { items.first { $0.name == key }?.value }
        switch host {
        case "file":
            guard let path = value("path") else { return nil }
            self = .file(path: path, line: value("line").flatMap(Int.init))
        case "pane":
            guard let id = value("id") else { return nil }
            self = .pane(id)
        case "artifact":
            guard let name = value("name") else { return nil }
            self = .artifact(name)
        case "run":
            guard value("key") == Self.key, let command = value("command") else { return nil }
            self = .run(command)
        case "image":
            guard value("key") == Self.key, let target = value("url"), ChatImages.isRemote(target) else { return nil }
            self = .loadImage(target)
        case "copy":
            guard value("key") == Self.key, let text = value("text") else { return nil }
            self = .copy(text)
        default:
            return nil
        }
    }
}

/// The daemon's answer to whether a file exists, as far as a client has asked.
public enum FileProbe: Sendable, Equatable {
    case unknown
    case missing
    /// The file is there; the absolute path the daemon resolved it to.
    case found(String)
}

/// A few lines of a file, as `file_peek` answered.
public struct FilePeek: Sendable, Equatable {
    public var path: String
    public var found: Bool
    /// The number of the first of `lines`.
    public var start: Int
    public var lines: [String]
    public var error: String?

    public init(path: String, found: Bool, start: Int, lines: [String], error: String? = nil) {
        self.path = path
        self.found = found
        self.start = start
        self.lines = lines
        self.error = error
    }
}

/// An image file's bytes, as `file_image` answered.
public struct FileImage: Sendable, Equatable {
    public var path: String
    public var found: Bool
    public var mime: String?
    public var data: Data?
    public var error: String?

    public init(path: String, found: Bool, mime: String? = nil, data: Data? = nil, error: String? = nil) {
        self.path = path
        self.found = found
        self.mime = mime
        self.data = data
        self.error = error
    }
}

/// What a client knows about the things a reply names.
public struct ChatLinkKnowledge {
    public var file: (_ path: String) -> FileProbe
    /// The chip text for a pane id that exists ("p1958 · Claude"); nil when no such pane.
    public var pane: (_ id: String) -> String?
    public var artifact: (_ name: String) -> Bool

    public init(
        file: @escaping (String) -> FileProbe = { _ in .unknown },
        pane: @escaping (String) -> String? = { _ in nil },
        artifact: @escaping (String) -> Bool = { _ in false }
    ) {
        self.file = file
        self.pane = pane
        self.artifact = artifact
    }
}

/// Finds the file paths, pane ids, web links and artifact names in a reply's Markdown and turns
/// the ones that exist into Markdown links. A path inside a fenced code block is left alone; a
/// path that is the whole of an inline code span stays a code span, now a link.
public enum ChatLinks {
    /// The most distinct files one reply asks the daemon about (and links).
    public static let maxFileProbes = 24

    /// `source` with each link that exists written as `[label](address)`.
    public static func rewrite(_ source: String, settings: ChatLinkSettings, knowledge: ChatLinkKnowledge) -> String {
        process(source, settings: settings) { target, original in
            switch target {
            case let .file(path, line):
                guard case let .found(resolved) = knowledge.file(path) else { return nil }
                let label: String
                switch settings.fileLabel {
                case .name:
                    let name = (path as NSString).lastPathComponent
                    label = line.map { "\(name):\($0)" } ?? name
                case .full:
                    label = original
                }
                return (label, ChatLinkAction.url(file: resolved, line: line))
            case let .pane(id):
                guard let label = knowledge.pane(id) else { return nil }
                return (label, ChatLinkAction.url(pane: id))
            case let .artifact(name):
                guard knowledge.artifact(name) else { return nil }
                return (original, ChatLinkAction.url(artifact: name))
            case let .web(url):
                return (original, url)
            case let .pattern(action):
                switch action {
                case let .open(url): return (original, url)
                case let .run(command): return (original, ChatLinkAction.url(run: command))
                case let .copy(text): return (original, ChatLinkAction.url(copy: text))
                }
            }
        }
    }

    /// The files, and the artifact names, `source` names that a client has to ask the daemon about,
    /// each once, in order.
    public static func targets(in source: String, settings: ChatLinkSettings) -> [ChatLinkTarget] {
        var seen = Set<ChatLinkTarget>()
        var out: [ChatLinkTarget] = []
        _ = process(source, settings: settings) { target, _ in
            switch target {
            case .file, .artifact:
                if seen.insert(target).inserted { out.append(target) }
            case .pane, .web, .pattern:
                break
            }
            return nil
        }
        return out
    }

    // MARK: Scanning

    private typealias Link = (label: String, address: String)

    /// Walk the source line by line, skipping fenced code, and ask `link` about each candidate.
    private static func process(
        _ source: String,
        settings: ChatLinkSettings,
        link: (ChatLinkTarget, _ original: String) -> Link?
    ) -> String {
        // A fenced block closes on a fence of the same character at least as long as its opener.
        var fence: (char: Character, length: Int)?
        var files = Set<ChatLinkTarget>()
        let limited: (ChatLinkTarget, String) -> Link? = { target, original in
            if case .file = target, !files.contains(target) {
                guard files.count < maxFileProbes else { return nil }
                files.insert(target)
            }
            return link(target, original)
        }
        var out: [String] = []
        for line in source.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.drop { $0 == " " }
            let run = trimmed.prefix { $0 == "`" || $0 == "~" }
            let isFence = line.count - trimmed.count < 4 && run.count >= 3 && Set(run).count == 1
            if let open = fence {
                out.append(String(line))
                if isFence, run.first == open.char, run.count >= open.length,
                   trimmed.dropFirst(run.count).allSatisfy({ $0 == " " }) { fence = nil }
                continue
            }
            if isFence, let char = run.first {
                fence = (char, run.count)
                out.append(String(line))
                continue
            }
            out.append(rewriteLine(String(line), settings: settings, link: limited))
        }
        return out.joined(separator: "\n")
    }

    private static func rewriteLine(_ line: String, settings: ChatLinkSettings, link: (ChatLinkTarget, String) -> Link?) -> String {
        let chars = Array(line)
        var out = ""
        var text = ""
        func flushText() {
            if !text.isEmpty { out += rewriteText(text, settings: settings, link: link) }
            text = ""
        }
        var i = 0
        while i < chars.count {
            let c = chars[i]
            if c == "\\", i + 1 < chars.count {
                text.append(c)
                text.append(chars[i + 1])
                i += 2
            } else if c == "`" {
                var run = 0
                while i + run < chars.count, chars[i + run] == "`" { run += 1 }
                if let close = closeOfCode(chars, from: i + run, run: run) {
                    flushText()
                    let inner = String(chars[(i + run)..<close])
                    let whole = String(chars[i..<(close + run)])
                    if run == 1, let replaced = codeSpanLink(inner, settings: settings, link: link) {
                        out += replaced
                    } else {
                        out += whole
                    }
                    i = close + run
                } else {
                    text += String(chars[i..<(i + run)])
                    i += run
                }
            } else if c == "[", let end = markdownLinkEnd(chars, from: i) {
                flushText()
                out += String(chars[i..<end])
                i = end
            } else if c == "<", let end = chars[i...].firstIndex(of: ">"), end > i + 1,
                      String(chars[(i + 1)..<end]).hasPrefix("http") {
                flushText()
                out += String(chars[i...end])
                i = end + 1
            } else {
                text.append(c)
                i += 1
            }
        }
        flushText()
        return out
    }

    /// The index of the backtick run of exactly `run` that closes a span whose text starts at `from`.
    private static func closeOfCode(_ chars: [Character], from: Int, run: Int) -> Int? {
        var i = from
        while i < chars.count {
            if chars[i] == "`" {
                var n = 0
                while i + n < chars.count, chars[i + n] == "`" { n += 1 }
                if n == run { return i }
                i += n
            } else {
                i += 1
            }
        }
        return nil
    }

    /// The index just past `[text](address)` starting at `from`, or nil when it is not one.
    private static func markdownLinkEnd(_ chars: [Character], from: Int) -> Int? {
        var depth = 0
        var i = from
        while i < chars.count {
            if chars[i] == "\\" { i += 2; continue }
            if chars[i] == "[" { depth += 1 }
            if chars[i] == "]" {
                depth -= 1
                if depth == 0 { break }
            }
            i += 1
        }
        guard i + 1 < chars.count, chars[i] == "]", chars[i + 1] == "(",
              let close = chars[(i + 2)...].firstIndex(of: ")") else { return nil }
        return close + 1
    }

    /// An inline code span that is entirely one target becomes a link whose text is still code.
    private static func codeSpanLink(_ inner: String, settings: ChatLinkSettings, link: (ChatLinkTarget, String) -> Link?) -> String? {
        let text = inner.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty, let match = matches(in: text, settings: settings).first,
              match.range == NSRange(location: 0, length: (text as NSString).length),
              let linked = link(match.target, text) else { return nil }
        return "[`\(linked.label)`](\(linked.address))"
    }

    private static func rewriteText(_ text: String, settings: ChatLinkSettings, link: (ChatLinkTarget, String) -> Link?) -> String {
        let ns = text as NSString
        var out = ""
        var cursor = 0
        for match in matches(in: text, settings: settings) {
            let original = ns.substring(with: match.range)
            guard let linked = link(match.target, original) else { continue }
            out += ns.substring(with: NSRange(location: cursor, length: match.range.location - cursor))
            out += "[\(escapeLabel(linked.label))](\(linked.address))"
            cursor = match.range.location + match.range.length
        }
        out += ns.substring(from: cursor)
        return out
    }

    private static func escapeLabel(_ label: String) -> String {
        var out = ""
        for c in label {
            if "\\`*_[]<>~|".contains(c) { out.append("\\") }
            out.append(c)
        }
        return out
    }

    // MARK: Matching

    struct Match {
        var range: NSRange
        var target: ChatLinkTarget
    }

    private static let urlPattern = try! NSRegularExpression(pattern: #"https?://[^\s<>()\[\]`"']+"#)
    private static let artifactPattern = try! NSRegularExpression(pattern: #"(?<![\w-])artifact ([A-Za-z0-9][A-Za-z0-9._-]*)"#)
    private static let panePattern = try! NSRegularExpression(pattern: #"(?<![\w/.-])p(\d+)(?![\w/@-]|\.\w)"#)
    private static let filePattern = try! NSRegularExpression(
        pattern: #"(?<![\w/.~@+:-])(?:(?:~|\.{1,2})?/)?(?:[\w@+.-]+/)*[\w@+.-]+(?::(\d+)(?::\d+)?)?(?![\w@+-])"#)
    private static let extensionPattern = try! NSRegularExpression(pattern: #"[\w@+-]{2,}\.[A-Za-z][A-Za-z0-9]{0,9}$"#)

    /// Every target in `text`, in order, none overlapping another: a web link wins over a path
    /// inside it, then the person's own patterns in the order they wrote them, an artifact name,
    /// a pane id, a file.
    static func matches(in text: String, settings: ChatLinkSettings) -> [Match] {
        let ns = text as NSString
        let all = NSRange(location: 0, length: ns.length)
        var taken: [Match] = []
        func free(_ r: NSRange) -> Bool { !taken.contains { NSIntersectionRange($0.range, r).length > 0 } }

        for m in urlPattern.matches(in: text, range: all) {
            var r = m.range
            while r.length > 1, ".,;:!?".contains(ns.substring(with: NSRange(location: r.location + r.length - 1, length: 1))) {
                r.length -= 1
            }
            if free(r) { taken.append(Match(range: r, target: .web(ns.substring(with: r)))) }
        }
        for m in artifactPattern.matches(in: text, range: all) {
            var name = ns.substring(with: m.range(at: 1))
            while name.hasSuffix(".") || name.hasSuffix("-") { name.removeLast() }
            guard !name.isEmpty else { continue }
            let r = NSRange(location: m.range.location, length: "artifact ".utf16.count + name.utf16.count)
            if free(r) { taken.append(Match(range: r, target: .artifact(name))) }
        }
        for pattern in settings.patterns {
            for m in pattern.regex.matches(in: text, range: all) where m.range.length > 0 && free(m.range) {
                taken.append(Match(range: m.range, target: .pattern(pattern.resolved(m, in: ns))))
            }
        }
        if settings.panes {
            for m in panePattern.matches(in: text, range: all) where free(m.range) {
                taken.append(Match(range: m.range, target: .pane(ns.substring(with: m.range))))
            }
        }
        if settings.files != .off {
            for m in filePattern.matches(in: text, range: all) {
                var r = m.range
                var line: Int?
                if m.range(at: 1).location != NSNotFound {
                    line = Int(ns.substring(with: m.range(at: 1)))
                    // The column, when written, ends the match but belongs to no field of ours.
                }
                var body = ns.substring(with: r)
                if let colon = body.firstIndex(of: ":") { body = String(body[..<colon]) }
                var trimmed = body
                while trimmed.hasSuffix(".") || trimmed.hasSuffix("-") { trimmed.removeLast() }
                guard !trimmed.isEmpty, looksLikeFile(trimmed) else { continue }
                if trimmed.count != body.count {
                    // A sentence's full stop ended the match: it is not part of the path, and a
                    // line number cannot follow it.
                    line = nil
                    r.length = (trimmed as NSString).length
                } else if line == nil {
                    r.length = (body as NSString).length
                }
                if free(r) { taken.append(Match(range: r, target: .file(path: trimmed, line: line))) }
            }
        }
        return taken.sorted { $0.range.location < $1.range.location }
    }

    /// A path has a folder in it, or a name with an extension; a lone word or number is not one.
    private static func looksLikeFile(_ s: String) -> Bool {
        if s.contains("/") { return s.count > 1 && !s.hasSuffix("/") }
        return extensionPattern.firstMatch(in: s, range: NSRange(location: 0, length: (s as NSString).length)) != nil
    }
}

extension ChatLinks {
    /// Where agent session `session`'s pane is: its space, the space's directory and the pane's id.
    public static func place(ofSession session: String, in spaces: [SpaceInfo]) -> (space: String, cwd: String, pane: String)? {
        for space in spaces {
            for pane in space.root.panes {
                if case let .agent(name, _) = pane.kind, name == session { return (space.id, space.cwd, pane.id) }
            }
        }
        return nil
    }

    /// Everything about `spaces` that `place(ofSession:in:)` and `chip(for:in:)` read: each pane's
    /// space, directory, id, chip and agent session. Equal keys give equal links.
    public static func layoutKey(_ spaces: [SpaceInfo]) -> String {
        var parts: [String] = []
        for space in spaces {
            for pane in space.root.panes {
                var part = "\(space.id) \(space.cwd) \(chip(for: pane.id, in: [space]) ?? pane.id)"
                if case let .agent(name, _) = pane.kind { part += " \(name)" }
                parts.append(part)
            }
        }
        return parts.joined(separator: "\n")
    }

    /// The chip text for pane `id` in any of `spaces`: "p1958 · Claude", the id and what the pane
    /// shows; nil when no pane has that id.
    public static func chip(for id: String, in spaces: [SpaceInfo]) -> String? {
        for space in spaces {
            for pane in space.root.panes where pane.id == id {
                let what: String
                switch pane.kind {
                case .empty: what = "Empty"
                case let .terminal(_, agent): what = agent.map { AgentInfo.title(of: $0.name) } ?? "Terminal"
                case let .web(artifact): what = artifact.name
                case .browser: what = "Browser"
                case let .agent(_, agent): what = AgentInfo.title(of: agent)
                case let .unknown(kind, _): what = kind
                }
                return "\(id) · \(what)"
            }
        }
        return nil
    }
}
