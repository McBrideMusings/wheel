import Foundation

/// Web panes: the daemon serves each one's folder at `/artifacts/<pane>/` and sends `pane_reload`
/// when a file in it changes.
public enum WebPanes {
    /// Every web pane in the layout: pane id to the folder it shows.
    public static func folders(_ spaces: [SpaceInfo]) -> [String: String] {
        var out: [String: String] = [:]
        for space in spaces {
            for pane in space.root.panes {
                if case let .web(dir) = pane.kind { out[pane.id] = dir }
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
