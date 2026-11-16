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

    /// Whether a cookie of `path` goes with a request for `requestPath`: the same path, or one
    /// under it on a `/` boundary (`/app` goes to `/app/x`, never to `/application`).
    public static func pathMatches(_ path: String, _ requestPath: String) -> Bool {
        let request = requestPath.isEmpty ? "/" : requestPath
        guard request.hasPrefix(path) else { return false }
        return request.count == path.count || path.hasSuffix("/") || request.dropFirst(path.count).first == "/"
    }
}
