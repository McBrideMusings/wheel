import CryptoKit
import Foundation

/// Web panes: the daemon resolves each one's artifact to the folder its page sits in, serves it at
/// `/artifacts/<pane>/` and sends `pane_reload` when a file in it changes.
public enum WebPanes {
    /// Every web pane in the layout: pane id to the artifact it shows.
    public static func artifacts(_ spaces: [SpaceInfo]) -> [String: ArtifactRef] {
        var out: [String: ArtifactRef] = [:]
        for space in spaces {
            for pane in space.root.panes {
                if case let .web(artifact) = pane.kind { out[pane.id] = artifact }
            }
        }
        return out
    }

    /// Every web pane in the layout: pane id to the browser storage its page keeps.
    public static func stores(_ spaces: [SpaceInfo]) -> [String: UUID] {
        var out: [String: UUID] = [:]
        for space in spaces {
            for pane in space.root.panes {
                if case let .web(artifact) = pane.kind { out[pane.id] = store(cwd: space.cwd, artifact: artifact) }
            }
        }
        return out
    }

    /// The browser storage an artifact's page keeps: one per artifact, the same on every launch,
    /// so a page's `localStorage` outlives its pane and the app. A project artifact's name belongs
    /// to its space's directory, so two projects with a `site` each get their own; a machine
    /// artifact is the same artifact in every space, so it gets one. The id is the first 16 bytes
    /// of a SHA-256 of those parts, marked as an RFC 9562 version 8 UUID.
    public static func store(cwd: String, artifact: ArtifactRef) -> UUID {
        let parts = artifact.scope == .machine
            ? ["machine", artifact.name]
            : ["project", cwd, artifact.name]
        var b = Array(SHA256.hash(data: Data(parts.joined(separator: "\n").utf8)).prefix(16))
        b[6] = (b[6] & 0x0F) | 0x80
        b[8] = (b[8] & 0x3F) | 0x80
        return UUID(uuid: (b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                           b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]))
    }

    /// The scheme a web pane's page is opened under. A page's browser storage is keyed by its
    /// origin, so the origin cannot hold the daemon's port: the client answers this scheme itself
    /// and fetches each request from the daemon, wherever it listens.
    public static let scheme = "wheel-page"

    /// Where a client opens web pane `pane`: the artifact's folder under a fixed origin, whatever
    /// address the daemon has.
    public static func pageURL(pane: String) -> URL? {
        URL(string: "\(scheme)://pages/artifacts/\(pane)/")
    }

    /// The daemon's address for a request a page in `pane` made: the same path and query on the
    /// daemon's HTTP side (`ws` becomes `http`, `wss` becomes `https`). Nil for anything outside
    /// `/artifacts/<pane>/`.
    public static func daemonURL(for page: URL, daemon: URL, pane: String) -> URL? {
        guard page.scheme == scheme, var c = URLComponents(url: daemon, resolvingAgainstBaseURL: false),
              let from = URLComponents(url: page, resolvingAgainstBaseURL: false),
              from.percentEncodedPath.hasPrefix("/artifacts/\(pane)/") else { return nil }
        c.scheme = c.scheme == "wss" ? "https" : "http"
        c.percentEncodedPath = from.percentEncodedPath
        c.percentEncodedQuery = from.percentEncodedQuery
        return c.url
    }

    /// The address with its query removed, so no log line holds one.
    public static func withoutQuery(_ url: URL) -> String {
        var parts = URLComponents(url: url, resolvingAgainstBaseURL: false)
        parts?.query = nil
        return parts?.string ?? url.path
    }

    /// The error's domain, code and failing address with its query removed. WebKit's
    /// `localizedDescription` can repeat the failing URL, query and token included.
    public static func describe(_ error: Error) -> String {
        let nsError = error as NSError
        var text = "\(nsError.domain) \(nsError.code)"
        if let failing = nsError.userInfo[NSURLErrorFailingURLErrorKey] as? URL {
            text += " \(withoutQuery(failing))"
        } else if let string = nsError.userInfo[NSURLErrorFailingURLStringErrorKey] as? String, let failing = URL(string: string) {
            text += " \(withoutQuery(failing))"
        }
        return text
    }
}
