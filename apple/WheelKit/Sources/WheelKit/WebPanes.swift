import Foundation

/// Web panes: the daemon resolves each one's artifact to a folder, serves it at
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

    /// Where a client opens web pane `pane`: its folder's `index.html` on the daemon's HTTP side
    /// (`ws` becomes `http`, `wss` becomes `https`), with the token in the query. The daemon hands
    /// the page's own requests the token as a cookie.
    public static func url(daemon: URL, pane: String, token: String) -> URL? {
        guard var c = URLComponents(url: daemon, resolvingAgainstBaseURL: false) else { return nil }
        c.scheme = c.scheme == "wss" ? "https" : "http"
        c.path = "/artifacts/\(pane)/index.html"
        // Every character but letters and digits escaped: a `+` left bare would reach the daemon
        // as a space.
        c.percentEncodedQuery = "token=" + (token.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? "")
        return c.url
    }
}
