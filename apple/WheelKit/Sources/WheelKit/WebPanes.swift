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

    /// Where a client opens web pane `pane`: the folder's own address on the daemon's HTTP side
    /// (`ws` becomes `http`, `wss` becomes `https`), which serves the artifact's page — its
    /// `index.html`, or the file its entry names — with the token in the query. The daemon hands
    /// the page's own requests the token as a cookie.
    public static func url(daemon: URL, pane: String, token: String) -> URL? {
        guard var c = URLComponents(url: daemon, resolvingAgainstBaseURL: false) else { return nil }
        c.scheme = c.scheme == "wss" ? "https" : "http"
        c.path = "/artifacts/\(pane)/"
        // Every character but letters and digits escaped: a `+` left bare would reach the daemon
        // as a space.
        c.percentEncodedQuery = "token=" + (token.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? "")
        return c.url
    }
}
