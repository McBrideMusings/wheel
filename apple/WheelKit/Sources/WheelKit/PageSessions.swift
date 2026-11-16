import Foundation

/// One cookie of a site's session, as it crosses wheeld between the Mac and a phone.
public struct PageCookie: Sendable, Equatable {
    public var name: String
    public var value: String
    public var domain: String
    public var path: String
    /// When it expires, in seconds since 1970; nil for a cookie that ends with the browser session.
    public var expires: Double?
    public var secure: Bool
    public var httpOnly: Bool
    /// `Lax`, `Strict` or `None`, as the site set it; nil when it set none.
    public var sameSite: String?

    public init(name: String, value: String, domain: String, path: String = "/", expires: Double? = nil,
                secure: Bool = false, httpOnly: Bool = false, sameSite: String? = nil) {
        self.name = name
        self.value = value
        self.domain = domain
        self.path = path
        self.expires = expires
        self.secure = secure
        self.httpOnly = httpOnly
        self.sameSite = sameSite
    }
}

/// A site's stored data, which follows a browser pane between the Mac and a phone: the cookies a
/// request to `origin` carries and the page's localStorage (`local`) and sessionStorage
/// (`session`). `origin` is the page's origin as the Mac opens it (`http://localhost:5173`).
public struct PageSession: Sendable, Equatable {
    public var origin: String
    public var cookies: [PageCookie]
    public var local: [String: String]
    public var session: [String: String]

    public init(origin: String, cookies: [PageCookie] = [], local: [String: String] = [:], session: [String: String] = [:]) {
        self.origin = origin
        self.cookies = cookies
        self.local = local
        self.session = session
    }
}

extension PageCookie: Codable {
    private enum K: String, CodingKey { case name, value, domain, path, expires, secure, http_only, same_site }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        self.init(name: try c.decode(String.self, forKey: .name), value: try c.decode(String.self, forKey: .value),
                  domain: try c.decode(String.self, forKey: .domain), path: try c.decodeIfPresent(String.self, forKey: .path) ?? "/",
                  expires: try c.decodeIfPresent(Double.self, forKey: .expires), secure: try c.decodeIfPresent(Bool.self, forKey: .secure) ?? false,
                  httpOnly: try c.decodeIfPresent(Bool.self, forKey: .http_only) ?? false, sameSite: try c.decodeIfPresent(String.self, forKey: .same_site))
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: K.self)
        try c.encode(name, forKey: .name)
        try c.encode(value, forKey: .value)
        try c.encode(domain, forKey: .domain)
        try c.encode(path, forKey: .path)
        try c.encodeIfPresent(expires, forKey: .expires)
        try c.encode(secure, forKey: .secure)
        try c.encode(httpOnly, forKey: .http_only)
        try c.encodeIfPresent(sameSite, forKey: .same_site)
    }
}

extension PageSession: Codable {
    private enum K: String, CodingKey { case origin, cookies, local, session }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        self.init(origin: try c.decode(String.self, forKey: .origin), cookies: try c.decodeIfPresent([PageCookie].self, forKey: .cookies) ?? [],
                  local: try c.decodeIfPresent([String: String].self, forKey: .local) ?? [:],
                  session: try c.decodeIfPresent([String: String].self, forKey: .session) ?? [:])
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: K.self)
        try c.encode(origin, forKey: .origin)
        try c.encode(cookies, forKey: .cookies)
        try c.encode(local, forKey: .local)
        try c.encode(session, forKey: .session)
    }

    /// The session as the JSON object a protocol frame carries.
    public var json: [String: Any] {
        guard let data = try? JSONEncoder().encode(self), let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return [:] }
        return object
    }
}

/// The rules a session follows between web views: which cookies a host gets, which page an
/// origin's storage belongs to, and the scripts that read and write a page's storage.
public enum PageSessions {
    /// The key the storage script marks a tab with once it has written a session, so a reload
    /// keeps what the page changed since; never part of a snapshot.
    static let markKey = "__wheel_session"

    /// The lowercased host of `origin` (`https://Example.com:8443` → `example.com`).
    public static func host(of origin: String) -> String? {
        URLComponents(string: origin)?.host?.lowercased()
    }

    /// The origin of `page`: scheme, host and a port other than the scheme's default
    /// (`http://localhost:5173/x?y` → `http://localhost:5173`); nil for a page with no host.
    public static func origin(of page: String) -> String? {
        guard let c = URLComponents(string: page), let scheme = c.scheme?.lowercased(), let host = c.host?.lowercased(), !host.isEmpty else { return nil }
        let port = c.port.flatMap { (scheme == "http" && $0 == 80) || (scheme == "https" && $0 == 443) ? nil : $0 }
        return "\(scheme)://\(host)\(port.map { ":\($0)" } ?? "")"
    }

    /// Whether `page` lies on `origin`.
    public static func sameOrigin(_ page: String, _ origin: String) -> Bool {
        self.origin(of: page).map { $0 == self.origin(of: origin) } ?? false
    }

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

    public static func cookie(_ cookie: HTTPCookie) -> PageCookie {
        PageCookie(name: cookie.name, value: cookie.value, domain: cookie.domain, path: cookie.path,
                   expires: cookie.expiresDate?.timeIntervalSince1970, secure: cookie.isSecure, httpOnly: cookie.isHTTPOnly,
                   sameSite: cookie.sameSitePolicy.map { $0 == .sameSiteStrict ? "Strict" : "Lax" })
    }

    /// The cookie for a cookie store; nil when its fields do not make one.
    public static func httpCookie(_ cookie: PageCookie) -> HTTPCookie? {
        var properties: [HTTPCookiePropertyKey: Any] = [
            .name: cookie.name, .value: cookie.value, .domain: cookie.domain, .path: cookie.path,
        ]
        if let expires = cookie.expires { properties[.expires] = Date(timeIntervalSince1970: expires) }
        if cookie.secure { properties[.secure] = "TRUE" }
        if cookie.httpOnly { properties[HTTPCookiePropertyKey("HttpOnly")] = "TRUE" }
        // Foundation names the policy in lower case and has no `None`, a cookie's default.
        if let policy = cookie.sameSite?.lowercased(), ["lax", "strict"].contains(policy) { properties[.sameSitePolicy] = policy }
        return HTTPCookie(properties: properties)
    }

    /// Evaluated in a page, the page's storage as JSON text: `{"local": {…}, "session": {…}}`.
    public static let snapshotScript = """
    (() => {
      const read = (s) => { const o = {}; for (let i = 0; i < s.length; i++) { const k = s.key(i); if (k !== "\(markKey)") o[k] = s.getItem(k); } return o; };
      return JSON.stringify({ local: read(localStorage), session: read(sessionStorage) });
    })()
    """

    /// The storage a `snapshotScript` result holds.
    public static func storage(fromSnapshot text: String) -> (local: [String: String], session: [String: String])? {
        guard let data = text.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return (object["local"] as? [String: String] ?? [:], object["session"] as? [String: String] ?? [:])
    }

    /// A script that writes `session`'s storage into a page on its origin's host and port (a phone
    /// opens a Mac loopback page under another scheme, so the scheme is not compared), once per
    /// tab for `mark`: a reload after that keeps what the page changed.
    public static func storageScript(_ session: PageSession, mark: String) -> String {
        let c = URLComponents(string: session.origin)
        let host = c?.host?.lowercased() ?? ""
        let port = c?.port.map(String.init) ?? (c?.scheme?.lowercased() == "https" ? "443" : "80")
        let payload = (try? JSONSerialization.data(withJSONObject: ["local": session.local, "session": session.session, "host": host, "port": port, "mark": mark]))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
        return """
        (() => {
          const s = \(payload);
          const port = location.port || (location.protocol === "https:" ? "443" : "80");
          if (location.hostname.toLowerCase() !== s.host || port !== s.port) return;
          try { if (sessionStorage.getItem("\(markKey)") === s.mark) return; } catch (e) { return; }
          for (const [k, v] of Object.entries(s.local)) localStorage.setItem(k, v);
          for (const [k, v] of Object.entries(s.session)) sessionStorage.setItem(k, v);
          sessionStorage.setItem("\(markKey)", s.mark);
        })();
        """
    }
}

/// Why no session came for a page: sharing is off for the pane, no Mac is attached, an older wheeld.
public struct PageSessionRefusal: Error, Sendable, Equatable {
    public let reason: String
    public init(_ reason: String) { self.reason = reason }
}
