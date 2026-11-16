import Foundation

/// What a client needs to draw browser panes that is not drawing: which panes the layout holds
/// and how the address field shows a page. What typed text means, and which pages a pane may
/// open, the daemon decides.
public enum BrowserPanes {
    /// Every browser pane in the layout: pane id to the page it opens at.
    public static func urls(_ spaces: [SpaceInfo]) -> [String: String] {
        var out: [String: String] = [:]
        for space in spaces {
            for pane in space.allPanes {
                if case let .browser(url) = pane.kind { out[pane.id] = url }
            }
        }
        return out
    }

    /// The scheme a client that draws a browser pane's page itself (a phone) loads a page of the
    /// Mac's loopback under, so the page keeps one origin and its root-relative links resolve on it;
    /// its handler fetches each request from the daemon's `/loopback/<port>/` (ADR-0005).
    public static let loopbackScheme = "wheel-loopback"

    private static let loopbackHosts: Set<String> = ["localhost", "127.0.0.1", "[::1]", "::1", "0.0.0.0"]

    /// Where a phone loads `page`: a page of the Mac's loopback (`http://localhost:5173/x`) under
    /// `loopbackScheme` with the same host, port, path and query; nil for any other page, which
    /// the phone loads as it is.
    public static func phonePage(_ page: String) -> URL? {
        guard var c = URLComponents(string: page), c.scheme?.lowercased() == "http",
              let host = c.host?.lowercased(), loopbackHosts.contains(host) else { return nil }
        c.scheme = loopbackScheme
        c.host = host
        if c.port == nil { c.port = 80 }
        if c.percentEncodedPath.isEmpty { c.percentEncodedPath = "/" }
        return c.url
    }

    /// The page a phone's `loopbackScheme` address stands for, as the Mac would open it; any other
    /// address as it is.
    public static func macPage(_ url: URL) -> String {
        guard url.scheme == loopbackScheme, var c = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            return url.absoluteString
        }
        c.scheme = "http"
        if c.port == 80 { c.port = nil }
        return c.url?.absoluteString ?? url.absoluteString
    }

    /// Whether a request for `url` comes from a page the loopback handler may serve: one under
    /// `loopbackScheme` on the same host and port (`document` is the request's main document). A
    /// page from anywhere else (a public site the phone browsed to) never reaches the Mac's
    /// loopback, so it cannot send requests there that its own origin would not allow.
    public static func loopbackRequestAllowed(_ url: URL, document: URL?) -> Bool {
        guard let document, document.scheme == loopbackScheme, url.scheme == loopbackScheme else { return false }
        return document.host?.lowercased() == url.host?.lowercased() && document.port == url.port
    }

    /// The daemon's address for a request under `loopbackScheme`: `/loopback/<port><path>` with
    /// the same query on the daemon's HTTP side (`ws` becomes `http`, `wss` becomes `https`). Nil
    /// for any other address, a host that is not the Mac's loopback included.
    public static func loopbackDaemonURL(for page: URL, daemon: URL) -> URL? {
        guard page.scheme == loopbackScheme, let port = page.port,
              let host = page.host?.lowercased(), loopbackHosts.contains(host),
              var c = URLComponents(url: daemon, resolvingAgainstBaseURL: false),
              let from = URLComponents(url: page, resolvingAgainstBaseURL: false) else { return nil }
        c.scheme = c.scheme == "wss" ? "https" : "http"
        let path = from.percentEncodedPath.isEmpty ? "/" : from.percentEncodedPath
        c.percentEncodedPath = "/loopback/\(port)\(path)"
        c.percentEncodedQuery = from.percentEncodedQuery
        return c.url
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
}
