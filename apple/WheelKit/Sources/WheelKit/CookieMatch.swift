import Foundation

/// How a browser matches a stored cookie to a request, for a client that sends the `Cookie`
/// header itself because WebKit adds none to a custom scheme's requests.
public enum CookieMatch {
    /// Whether a cookie set for `domain` goes with a request to `host`, as a browser matches it:
    /// the same host, or `host` inside the cookie's domain (`.example.com` reaches `www.example.com`).
    public static func carries(_ domain: String, to host: String) -> Bool {
        let domain = domain.lowercased(), host = host.lowercased()
        // A cookie set with a Domain attribute is stored with a leading dot and goes to the
        // domain's subdomains too; one without it goes to its own host only.
        guard domain.hasPrefix(".") else { return !domain.isEmpty && host == domain }
        let bare = domain.drop(while: { $0 == "." })
        return !bare.isEmpty && (host == bare || host.hasSuffix("." + bare))
    }

    /// The request a cookie is weighed for under its `SameSite` attribute.
    public enum Context: Equatable {
        /// The request's site is the site of the page that sends it.
        case sameSite
        /// Another site's top-level `GET` navigation to it.
        case crossSiteNavigation
        /// Any other request from another site.
        case crossSite
    }

    /// Whether a cookie whose `SameSite` is `policy` (`strict`, `lax`, or nil or `none` for none)
    /// goes with a request in `context`. A cookie that names no `SameSite` is sent as `None`, as
    /// WebKit (the Mac's browser) sends it.
    public static func sameSiteAllows(_ policy: String?, in context: Context) -> Bool {
        switch policy?.lowercased() {
        case "strict": context == .sameSite
        case "lax": context != .crossSite
        default: true
        }
    }

    /// Whether a cookie of `path` goes with a request for `requestPath`: the same path, or one
    /// under it on a `/` boundary (`/app` goes to `/app/x`, never to `/application`).
    public static func pathMatches(_ path: String, _ requestPath: String) -> Bool {
        let request = requestPath.isEmpty ? "/" : requestPath
        guard request.hasPrefix(path) else { return false }
        return request.count == path.count || path.hasSuffix("/") || request.dropFirst(path.count).first == "/"
    }
}
