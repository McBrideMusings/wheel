import Foundation
import Testing
@testable import WheelKit

struct LoopbackCORSTests {
    let a = URL(string: "wheel-loopback://localhost:5173/app")!
    let b = URL(string: "wheel-loopback://localhost:3000/api")!

    /// A loopback page reaches any loopback port; a page from elsewhere, or a request sent with
    /// another site's origin, reaches none; a request with no Origin to another port only reads.
    @Test func aLoopbackPageReachesAnyLoopbackPortAndNothingElseDoes() {
        func allowed(_ url: URL, _ document: URL?, _ origin: String?, _ method: String = "GET") -> Bool {
            BrowserPanes.loopbackRequestAllowed(url, document: document, origin: origin, method: method)
        }
        #expect(allowed(b, a, nil))
        #expect(allowed(b, a, nil, "HEAD"))
        #expect(allowed(b, a, "wheel-loopback://localhost:5173", "POST"))
        #expect(allowed(b, a, "wheel-loopback://127.0.0.1:8080", "DELETE"))
        #expect(!allowed(b, a, nil, "POST"))
        #expect(!allowed(b, a, nil, "PUT"))
        #expect(allowed(URL(string: "wheel-loopback://localhost:5173/form")!, a, nil, "POST"))
        #expect(!allowed(b, URL(string: "https://example.com/")!, nil))
        #expect(!allowed(b, nil, nil))
        #expect(!allowed(b, a, "https://evil.test"))
        #expect(!allowed(b, a, "null"))
        #expect(!allowed(URL(string: "https://example.com/")!, a, nil))
    }

    /// One origin whether port 80 is written or not; every port of a host is one site.
    @Test func originsAndSitesCompareAsABrowserCompares() {
        #expect(LoopbackCORS.sameOrigin(URL(string: "wheel-loopback://localhost:80/a")!, URL(string: "wheel-loopback://LOCALHOST/b")!))
        #expect(!LoopbackCORS.sameOrigin(a, b))
        #expect(LoopbackCORS.site(of: a) == "localhost")
        #expect(LoopbackCORS.site(of: a) == LoopbackCORS.site(of: b))
        #expect(LoopbackCORS.site(of: URL(string: "wheel-loopback://127.0.0.1:3000/")!) == "127.0.0.1")
        #expect(LoopbackCORS.site(of: URL(string: "https://localhost/")!) == nil)
    }

    /// SameSite: Strict only same-site, Lax also on another site's top-level GET, None (or none
    /// named) everywhere.
    @Test func sameSiteDecidesWhichCookiesGo() {
        let contexts: [CookieMatch.Context] = [.sameSite, .crossSiteNavigation, .crossSite]
        #expect(contexts.map { CookieMatch.sameSiteAllows("strict", in: $0) } == [true, false, false])
        #expect(contexts.map { CookieMatch.sameSiteAllows("lax", in: $0) } == [true, true, false])
        #expect(contexts.map { CookieMatch.sameSiteAllows(nil, in: $0) } == [true, true, true])
        #expect(contexts.map { CookieMatch.sameSiteAllows("none", in: $0) } == [true, true, true])
    }

    /// The headers WebKit adds to revalidate a page it was given before do not count as the
    /// page's; the same headers with other values do.
    @Test func aRevalidationNeedsNoPreflight() {
        let kept = LoopbackCORS.Validators(answer: ["ETag": "\"v1\"", "Last-Modified": "Wed, 21 Oct 2015 07:28:00 GMT"])
        #expect(LoopbackCORS.Validators(answer: ["Content-Type": "text/plain"]) == nil)
        let reload = ["Cache-Control": "max-age=0", "If-None-Match": "\"v1\"", "If-Modified-Since": "Wed, 21 Oct 2015 07:28:00 GMT",
                      "Origin": "wheel-loopback://localhost:5173"]
        #expect(!LoopbackCORS.needsPreflight(method: "GET", headers: LoopbackCORS.pageHeaders(reload, revalidating: kept)))
        #expect(LoopbackCORS.needsPreflight(method: "GET", headers: LoopbackCORS.pageHeaders(reload, revalidating: nil)))
        let own = ["Cache-Control": "max-age=0", "If-None-Match": "\"v2\""]
        #expect(LoopbackCORS.unsafeHeaderNames(LoopbackCORS.pageHeaders(own, revalidating: kept)) == ["cache-control", "if-none-match"])
        #expect(LoopbackCORS.withoutBody(["Content-Type": "application/json", "X-A": "1"]) == ["X-A": "1"])
    }

    /// A page's origin on the phone and the Mac's name for it.
    @Test func originsMapToTheMacs() {
        #expect(LoopbackCORS.pageOrigin(of: b) == "wheel-loopback://localhost:3000")
        #expect(LoopbackCORS.pageOrigin(of: URL(string: "wheel-loopback://LOCALHOST:80/")!) == "wheel-loopback://localhost:80")
        #expect(LoopbackCORS.pageOrigin(of: URL(string: "https://localhost:3000/")!) == nil)
        #expect(LoopbackCORS.macOrigin("wheel-loopback://localhost:5173") == "http://localhost:5173")
        #expect(LoopbackCORS.macOrigin("wheel-loopback://127.0.0.1:80") == "http://127.0.0.1")
        #expect(LoopbackCORS.macOrigin("wheel-loopback://[::1]:3000") == "http://[::1]:3000")
        #expect(LoopbackCORS.macOrigin("wheel-loopback://localhost:5173/x") == nil)
        #expect(LoopbackCORS.macOrigin("wheel-loopback://evil.test:5173") == nil)
        #expect(LoopbackCORS.macOrigin("https://localhost:5173") == nil)
        #expect(LoopbackCORS.macOrigin("null") == nil)
    }

    /// The request WebKit hands over carries its own Accept, Referer and User-Agent; only what the
    /// page adds past the safelist needs a preflight.
    @Test func onlyAnUnsafeMethodOrHeaderNeedsAPreflight() {
        let engine = ["Accept": "*/*", "Origin": "wheel-loopback://localhost:5173", "Referer": "wheel-loopback://localhost:5173/",
                      "User-Agent": "Mozilla/5.0 (iPhone)"]
        #expect(!LoopbackCORS.needsPreflight(method: "GET", headers: engine))
        #expect(!LoopbackCORS.needsPreflight(method: "POST", headers: engine.merging(["Content-Type": "text/plain;charset=UTF-8"]) { $1 }))
        #expect(!LoopbackCORS.needsPreflight(method: "GET", headers: ["Range": "bytes=0-99"]))
        #expect(LoopbackCORS.needsPreflight(method: "PUT", headers: engine))
        #expect(LoopbackCORS.needsPreflight(method: "POST", headers: ["Content-Type": "application/json"]))
        #expect(LoopbackCORS.needsPreflight(method: "GET", headers: ["Range": "bytes=9-1"]))
        #expect(LoopbackCORS.needsPreflight(method: "GET", headers: ["Accept": "a(b)"]))
        #expect(LoopbackCORS.unsafeHeaderNames(engine.merging(["X-Token": "1", "Content-Type": "application/json"]) { $1 }) == ["content-type", "x-token"])
        #expect(LoopbackCORS.preflightHeaders(method: "PUT", headers: ["X-B": "1", "x-a": "2"])
            == ["Access-Control-Request-Method": "PUT", "Access-Control-Request-Headers": "x-a,x-b"])
        #expect(LoopbackCORS.preflightHeaders(method: "DELETE", headers: engine) == ["Access-Control-Request-Method": "DELETE"])
    }

    @Test func aPreflightAllowsOnlyWhatItsAnswerNames() {
        let origin = "http://localhost:5173"
        let custom = ["X-Token": "1"]
        func refusal(_ answer: [String: String], status: Int = 204, method: String = "GET",
                     headers: [String: String] = ["X-Token": "1"], credentials: Bool = false) -> String? {
            LoopbackCORS.preflightRefusal(status: status, answer: answer, origin: origin, method: method, headers: headers, credentials: credentials)
        }
        let allows = ["Access-Control-Allow-Origin": origin, "Access-Control-Allow-Headers": "X-Token, Content-Type"]
        #expect(refusal(allows) == nil)
        #expect(refusal(allows, status: 404) == "the preflight answered 404")
        #expect(refusal([:]) == "Access-Control-Allow-Origin is missing, not \(origin)")
        #expect(refusal(["Access-Control-Allow-Origin": "http://localhost:9999", "Access-Control-Allow-Headers": "x-token"]) != nil)
        #expect(refusal(["Access-Control-Allow-Origin": origin]) == "Access-Control-Allow-Headers does not allow x-token")
        #expect(refusal(["access-control-allow-origin": "*", "access-control-allow-headers": "*"]) == nil)
        #expect(refusal(["Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "*"], headers: ["Authorization": "x"]) != nil)
        // Methods: a safelisted one needs no listing; any other does, and `*` covers it only without credentials.
        #expect(refusal(["Access-Control-Allow-Origin": origin], method: "POST", headers: [:]) == nil)
        #expect(refusal(["Access-Control-Allow-Origin": origin], method: "PUT", headers: [:]) == "Access-Control-Allow-Methods does not allow PUT")
        #expect(refusal(["Access-Control-Allow-Origin": origin, "Access-Control-Allow-Methods": "GET, PUT"], method: "PUT", headers: [:]) == nil)
        #expect(refusal(["Access-Control-Allow-Origin": origin, "Access-Control-Allow-Methods": "*"], method: "PUT", headers: [:]) == nil)
        // Credentials: an exact origin and Allow-Credentials, and no wildcard.
        let credentialed = ["Access-Control-Allow-Origin": origin, "Access-Control-Allow-Credentials": "true", "Access-Control-Allow-Headers": "x-token"]
        #expect(refusal(credentialed, headers: custom, credentials: true) == nil)
        #expect(refusal(allows, credentials: true) == "Access-Control-Allow-Credentials is not true")
        #expect(refusal(["Access-Control-Allow-Origin": "*", "Access-Control-Allow-Credentials": "true", "Access-Control-Allow-Headers": "x-token"], credentials: true) != nil)
        #expect(refusal(["Access-Control-Allow-Origin": origin, "Access-Control-Allow-Credentials": "true", "Access-Control-Allow-Headers": "*"], credentials: true) != nil)
    }

    /// The answer's Allow-Origin naming the page's Mac origin names the page's own for WebKit;
    /// any other value stays as it was, so WebKit refuses it as the Mac's browser would.
    @Test func theAnswerNamesThePagesOwnOrigin() {
        let page = "wheel-loopback://localhost:5173"
        let mac = "http://localhost:5173"
        #expect(LoopbackCORS.pageAnswer(["Access-Control-Allow-Origin": mac, "X": "1"], origin: mac, pageOrigin: page)
            == ["Access-Control-Allow-Origin": page, "X": "1"])
        #expect(LoopbackCORS.pageAnswer(["access-control-allow-origin": "*"], origin: mac, pageOrigin: page) == ["access-control-allow-origin": "*"])
        #expect(LoopbackCORS.pageAnswer(["Access-Control-Allow-Origin": "http://127.0.0.1:5173"], origin: mac, pageOrigin: page)
            == ["Access-Control-Allow-Origin": "http://127.0.0.1:5173"])
    }
}
