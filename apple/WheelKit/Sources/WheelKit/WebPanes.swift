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
