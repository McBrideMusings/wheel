import Foundation

/// What a client needs to draw browser panes that is not drawing: which panes the layout holds,
/// what a person typed into the address field means, and which reported pages are its own.
public enum BrowserPanes {
    /// Every browser pane in the layout: pane id to the page the daemon says it is on.
    public static func urls(_ spaces: [SpaceInfo]) -> [String: String] {
        var out: [String: String] = [:]
        for space in spaces {
            for pane in space.root.panes {
                if case let .browser(url) = pane.kind { out[pane.id] = url }
            }
        }
        return out
    }

    /// The page a person means by `typed`: a path starting with `/` or `~` is that file; text
    /// with a scheme opens as typed; anything else is a web address and gets `https://`. Nil when
    /// even that is no URL (`rust async`) — there is no search to fall back to.
    public static func address(typed: String, home: URL = FileManager.default.homeDirectoryForCurrentUser) -> URL? {
        let text = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        if text == "~" || text.hasPrefix("~/") {
            return URL(fileURLWithPath: home.path + text.dropFirst())
        }
        if text.hasPrefix("/") { return URL(fileURLWithPath: text) }
        let lower = text.lowercased()
        if lower.hasPrefix("file://") {
            // The daemon keeps only a file on this machine, so a file on another host is no page.
            let path = lower.dropFirst(lower.hasPrefix("file://localhost") ? 16 : 7)
            return path.count > 1 && path.hasPrefix("/") ? URL(string: text) : nil
        }
        if ["http://", "https://"].contains(where: lower.hasPrefix) || lower == "about:blank" {
            return URL(string: text)
        }
        return URL(string: "https://" + text).flatMap { $0.host() == nil ? nil : $0 }
    }

    /// Whether a page the web view landed on is one the daemon keeps as a pane's page; a `data:`
    /// or `blob:` document is shown but never reported.
    public static func reportable(_ url: URL) -> Bool {
        switch url.scheme?.lowercased() {
        case "http", "https", "file": true
        case "about": url.absoluteString.lowercased() == "about:blank"
        default: false
        }
    }

    /// How the address field shows a page it is not editing.
    public enum Shown: Equatable {
        /// A blank pane: the field shows its placeholder.
        case blank
        /// A web page: `secure` for https, the host in full and the rest dimmed.
        case web(secure: Bool, host: String, rest: String)
        /// A file: its folder dimmed and its name in full.
        case file(folder: String, name: String)
        /// Anything else, as it is.
        case other(String)
    }

    public static func shown(_ url: String) -> Shown {
        // A failed file is shown by the path the person typed, which has no scheme.
        if url.hasPrefix("/") {
            let name = (url as NSString).lastPathComponent
            return .file(folder: String(url.dropLast(name.count)), name: name)
        }
        guard let parsed = URL(string: url), let scheme = parsed.scheme?.lowercased() else { return .other(url) }
        switch scheme {
        case "about": return url.lowercased() == "about:blank" ? .blank : .other(url)
        case "file":
            let path = parsed.path(percentEncoded: false)
            let name = (path as NSString).lastPathComponent
            return .file(folder: String(path.dropLast(name.count)), name: name)
        case "http", "https":
            guard let host = parsed.host(percentEncoded: false) else { return .other(url) }
            let port = parsed.port.map { ":\($0)" } ?? ""
            var rest = parsed.path(percentEncoded: true)
            if rest == "/" { rest = "" }
            if let query = parsed.query(percentEncoded: true) { rest += "?" + query }
            if let fragment = parsed.fragment(percentEncoded: true) { rest += "#" + fragment }
            return .web(secure: scheme == "https", host: host + port, rest: rest)
        default: return .other(url)
        }
    }

    /// The pages a pane reported to the daemon whose echo has not come back. The daemon applies a
    /// pane's reports in order and tells every client each one, so the echo of a page this pane
    /// already left must not drag it back: a page the daemon reports that is still waiting here is
    /// this pane's own, and drops it and every report before it. The last of those stays this
    /// pane's own, because a whole layout sent later still carries it until the daemon applies the
    /// next report. Any other page was asked for by someone else (the CLI, another client) and the
    /// pane goes there.
    public struct Echoes: Equatable {
        private var waiting: [String] = []
        private var confirmed: String?

        public init() {}

        public mutating func sent(_ url: String) { waiting.append(url) }

        /// The pane went to `url` because the daemon said so. That is the page the daemon holds
        /// now, so a layout built before any later report still carries it and is the pane's own;
        /// reports sent before the move were overtaken by it.
        public mutating func adopt(_ url: String) {
            waiting.removeAll()
            confirmed = url
        }

        /// Whether `url`, which the daemon now says the pane is on, is one this pane sent.
        public mutating func isOwn(_ url: String) -> Bool {
            if let at = waiting.firstIndex(of: url) {
                waiting.removeFirst(at + 1)
                confirmed = url
                return true
            }
            if url == confirmed { return true }
            confirmed = nil
            return false
        }
    }
}
