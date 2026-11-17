import Foundation

/// The cross-origin rules (CORS, in the Fetch standard) a phone's loopback page needs beyond what
/// WebKit applies to a custom scheme. WebKit hands a cross-origin request under
/// `BrowserPanes.loopbackScheme` to the scheme handler with an `Origin` naming the page
/// (`wheel-loopback://localhost:5173`), with cookies wanted only when the page asked for
/// credentials (`URLRequest.httpShouldHandleCookies`), and checks the answer's
/// `Access-Control-Allow-Origin` and, for a request with credentials, its
/// `Access-Control-Allow-Credentials`. It sends no preflight and checks no
/// `Access-Control-Allow-Methods` or `-Headers`: the handler does those, with the page's origin as
/// the Mac names it (`http://localhost:5173`), and turns that origin in an answer back into the
/// page's own for WebKit's check.
public enum LoopbackCORS {
    /// The header wheeld's `/loopback/<port>/` takes the page's Mac origin in; the program at the
    /// port sees it as the request's `Origin`.
    public static let originHeader = "Wheel-Page-Origin"
    /// The header wheeld's `/loopback/<port>/` takes the loopback host the page named in
    /// (`localhost`, `127.0.0.1`); the program sees it with the port as the request's `Host`.
    public static let hostHeader = "Wheel-Page-Host"

    /// The origin of a `loopbackScheme` address as WebKit writes it in `Origin`
    /// (`wheel-loopback://localhost:5173`); nil for any other address.
    public static func pageOrigin(of url: URL) -> String? {
        guard url.scheme == BrowserPanes.loopbackScheme, let c = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let host = c.percentEncodedHost?.lowercased(), BrowserPanes.loopbackHosts.contains(host) else { return nil }
        return "\(BrowserPanes.loopbackScheme)://\(host)" + (c.port.map { ":\($0)" } ?? "")
    }

    /// The origin the Mac's browser has for a page whose origin on the phone is `origin`:
    /// `wheel-loopback://localhost:5173` is `http://localhost:5173`, and port 80 goes unwritten.
    /// Nil for an origin that is not on the loopback scheme (a public site's, or `null`).
    public static func macOrigin(_ origin: String) -> String? {
        guard let url = URL(string: origin), pageOrigin(of: url) == origin.lowercased(),
              var c = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return nil }
        c.scheme = "http"
        if c.port == 80 { c.port = nil }
        return c.url?.absoluteString
    }

    /// Whether `a` and `b`, two `loopbackScheme` addresses, have one origin, with port 80 the
    /// same written or not.
    public static func sameOrigin(_ a: URL, _ b: URL) -> Bool {
        guard let a = pageOrigin(of: a).flatMap(macOrigin) else { return false }
        return a == pageOrigin(of: b).flatMap(macOrigin)
    }

    /// The site of a `loopbackScheme` address for `SameSite`: the Mac's scheme and host, so every
    /// port of `localhost` is one site and `127.0.0.1` is another.
    public static func site(of url: URL) -> String? {
        pageOrigin(of: url).flatMap(macOrigin).flatMap { URLComponents(string: $0)?.percentEncodedHost }
    }

    /// What an earlier answer for a page said about its version, which WebKit sends back by itself
    /// when it revalidates the page (a reload).
    public struct Validators: Equatable, Sendable {
        public let etag: String?
        public let lastModified: String?

        /// The validators of `answer`; nil when it has none.
        public init?(answer: [String: String]) {
            etag = LoopbackCORS.value(answer, "ETag")
            lastModified = LoopbackCORS.value(answer, "Last-Modified")
            if etag == nil, lastModified == nil { return nil }
        }
    }

    /// `headers` without those WebKit added itself to revalidate the page `validators` came from:
    /// an `If-None-Match` or `If-Modified-Since` that repeats them, and the `Cache-Control:
    /// max-age=0` beside it. The page's request is weighed for a preflight by what is left.
    public static func pageHeaders(_ headers: [String: String], revalidating validators: Validators?) -> [String: String] {
        guard let validators else { return headers }
        var out = headers
        var revalidates = false
        for (name, v) in headers {
            let n = name.lowercased(), v = v.trimmingCharacters(in: .whitespaces)
            if (n == "if-none-match" && v == validators.etag) || (n == "if-modified-since" && v == validators.lastModified) {
                out.removeValue(forKey: name)
                revalidates = true
            }
        }
        if revalidates {
            for (name, v) in headers where name.lowercased() == "cache-control" && v.trimmingCharacters(in: .whitespaces) == "max-age=0" {
                out.removeValue(forKey: name)
            }
        }
        return out
    }

    /// The headers a request keeps when a redirect turns it into a `GET` with no body.
    public static func withoutBody(_ headers: [String: String]) -> [String: String] {
        headers.filter { !["content-type", "content-encoding", "content-language", "content-location"].contains($0.key.lowercased()) }
    }

    /// The methods a cross-origin request may use with no preflight.
    static let safelistedMethods: Set<String> = ["GET", "HEAD", "POST"]

    /// Headers WebKit writes on every request itself; a page never sets them.
    static let engineHeaders: Set<String> = ["origin", "referer", "user-agent", "accept-encoding"]

    /// The names a preflight must ask for: each header of `headers` that is not CORS-safelisted,
    /// lowercased and sorted, as `Access-Control-Request-Headers` lists them.
    public static func unsafeHeaderNames(_ headers: [String: String]) -> [String] {
        let page = headers.filter { !engineHeaders.contains($0.key.lowercased()) }
        let safe = page.filter { safelisted(name: $0.key.lowercased(), value: $0.value) }
        // Safelisted values add up to at most 1024 bytes; past that every one counts as unsafe.
        let total = safe.values.reduce(0) { $0 + $1.utf8.count }
        let unsafe = total > 1024 ? Array(page.keys) : page.keys.filter { safe[$0] == nil }
        return unsafe.map { $0.lowercased() }.sorted()
    }

    /// Whether a cross-origin request with `method` and `headers` is sent only after a preflight.
    public static func needsPreflight(method: String, headers: [String: String]) -> Bool {
        !safelistedMethods.contains(method) || !unsafeHeaderNames(headers).isEmpty
    }

    /// The headers of the preflight `OPTIONS` for `method` and `headers`.
    public static func preflightHeaders(method: String, headers: [String: String]) -> [String: String] {
        var out = ["Access-Control-Request-Method": method]
        let names = unsafeHeaderNames(headers)
        if !names.isEmpty { out["Access-Control-Request-Headers"] = names.joined(separator: ",") }
        return out
    }

    /// Why the preflight's answer (`status`, `answer`) refuses a request from the Mac origin
    /// `origin` with `method` and `headers`, `credentials` when the page asked for them; nil when
    /// it allows it.
    public static func preflightRefusal(status: Int, answer: [String: String], origin: String,
                                        method: String, headers: [String: String], credentials: Bool) -> String? {
        guard (200..<300).contains(status) else { return "the preflight answered \(status)" }
        if let why = originRefusal(answer: answer, origin: origin, credentials: credentials) { return why }
        let methods = list(answer, "Access-Control-Allow-Methods")
        if !safelistedMethods.contains(method), !methods.contains(method), credentials || !methods.contains("*") {
            return "Access-Control-Allow-Methods does not allow \(method)"
        }
        let allowed = Set(list(answer, "Access-Control-Allow-Headers").map { $0.lowercased() })
        for name in unsafeHeaderNames(headers) where !allowed.contains(name) {
            // A wildcard never covers Authorization, nor any header of a request with credentials.
            if credentials || name == "authorization" || !allowed.contains("*") {
                return "Access-Control-Allow-Headers does not allow \(name)"
            }
        }
        return nil
    }

    /// Why an answer's `Access-Control-Allow-Origin` and `-Credentials` refuse the Mac origin
    /// `origin`: the check every answer to a cross-origin request passes, a redirect's included.
    public static func originRefusal(answer: [String: String], origin: String, credentials: Bool) -> String? {
        let allow = value(answer, "Access-Control-Allow-Origin")
        if credentials {
            guard allow == origin else { return "Access-Control-Allow-Origin is \(allow ?? "missing"), not \(origin)" }
            guard value(answer, "Access-Control-Allow-Credentials") == "true" else {
                return "Access-Control-Allow-Credentials is not true"
            }
        } else if allow != "*", allow != origin {
            return "Access-Control-Allow-Origin is \(allow ?? "missing"), not \(origin)"
        }
        return nil
    }

    /// `answer` as WebKit checks it for the page at `pageOrigin`: an
    /// `Access-Control-Allow-Origin` naming the page's Mac origin `origin` names the page's own.
    public static func pageAnswer(_ answer: [String: String], origin: String, pageOrigin: String) -> [String: String] {
        var out = answer
        for (name, v) in answer where name.lowercased() == "access-control-allow-origin" && v.trimmingCharacters(in: .whitespaces) == origin {
            out[name] = pageOrigin
        }
        return out
    }

    static func value(_ headers: [String: String], _ name: String) -> String? {
        headers.first { $0.key.lowercased() == name.lowercased() }?.value.trimmingCharacters(in: .whitespaces)
    }

    private static func list(_ headers: [String: String], _ name: String) -> [String] {
        (value(headers, name) ?? "").split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }

    /// Whether the header `name` (lowercased) with `value` is CORS-safelisted.
    static func safelisted(name: String, value: String) -> Bool {
        guard value.utf8.count <= 128 else { return false }
        switch name {
        case "accept":
            return !value.utf8.contains(where: unsafeByte)
        case "accept-language", "content-language":
            return value.utf8.allSatisfy { b in
                (0x30...0x39).contains(b) || (0x41...0x5A).contains(b) || (0x61...0x7A).contains(b) || " *,-.;=".utf8.contains(b)
            }
        case "content-type":
            guard !value.utf8.contains(where: unsafeByte) else { return false }
            let essence = value.split(separator: ";", maxSplits: 1).first.map { $0.trimmingCharacters(in: .whitespaces).lowercased() } ?? ""
            return ["application/x-www-form-urlencoded", "multipart/form-data", "text/plain"].contains(essence)
        case "range":
            // Only `bytes=<start>-` or `bytes=<start>-<end>`.
            guard value.hasPrefix("bytes=") else { return false }
            let parts = value.dropFirst(6).split(separator: "-", maxSplits: 1, omittingEmptySubsequences: false)
            guard parts.count == 2, let start = UInt64(parts[0]) else { return false }
            return parts[1].isEmpty || UInt64(parts[1]).map { $0 >= start } == true
        default:
            return false
        }
    }

    private static func unsafeByte(_ b: UInt8) -> Bool {
        (b < 0x20 && b != 0x09) || b == 0x7F || "\"():<>?@[\\]{}".utf8.contains(b)
    }
}
