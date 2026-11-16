import Foundation
import Testing
@testable import WheelKit

struct PageSessionsTests {
    /// A cookie goes to its own host and the hosts inside its domain, never to a host that only
    /// ends in the same letters.
    @Test func aCookieGoesWhereABrowserSendsIt() {
        #expect(PageSessions.carries("example.com", to: "example.com"))
        #expect(PageSessions.carries(".example.com", to: "www.example.com"))
        #expect(PageSessions.carries("Example.COM", to: "example.com"))
        #expect(!PageSessions.carries("example.com", to: "badexample.com"))
        #expect(!PageSessions.carries("www.example.com", to: "example.com"))
        // A cookie set without a Domain attribute is the host's own, never its subdomains'.
        #expect(!PageSessions.carries("example.com", to: "www.example.com"))
        #expect(!PageSessions.carries(".example.com", to: "badexample.com"))
        #expect(PageSessions.pathMatches("/", "/anything"))
        #expect(PageSessions.pathMatches("/app", "/app"))
        #expect(PageSessions.pathMatches("/app", "/app/x"))
        #expect(PageSessions.pathMatches("/app/", "/app/x"))
        #expect(!PageSessions.pathMatches("/app", "/application"))
        #expect(!PageSessions.pathMatches("/app", "/"))
        #expect(PageSessions.carries("localhost", to: "localhost"))
        #expect(!PageSessions.carries("", to: "localhost"))
    }

    /// An origin drops the path and the scheme's default port; a phone's loopback address stands
    /// for the Mac's page, so its storage is the same origin's.
    @Test func originsCompareAsABrowserDoes() {
        #expect(PageSessions.origin(of: "http://localhost:5173/app?x=1") == "http://localhost:5173")
        #expect(PageSessions.origin(of: "https://Example.com:443/a") == "https://example.com")
        #expect(PageSessions.origin(of: "about:blank") == nil)
        #expect(PageSessions.sameOrigin("http://localhost:5173/two", "http://localhost:5173"))
        #expect(!PageSessions.sameOrigin("http://localhost:5174/", "http://localhost:5173"))
        let phone = BrowserPanes.phonePage("http://localhost:5173/x")!
        #expect(PageSessions.sameOrigin(BrowserPanes.macPage(phone), "http://localhost:5173"))
    }

    /// A cookie keeps every field through the store's type and back, HttpOnly and SameSite included.
    /// (Foundation caps an expiry at 400 days out; SameSite reads as the wire writes it, `Lax`.)
    @Test func aCookieRoundTripsThroughTheStoresType() throws {
        let tomorrow = (Date().timeIntervalSince1970 + 86_400).rounded()
        let cookie = PageCookie(name: "sid", value: "abc", domain: "example.com", path: "/app", expires: tomorrow,
                                secure: true, httpOnly: true, sameSite: "Lax")
        let stored = try #require(PageSessions.httpCookie(cookie))
        #expect(stored.isHTTPOnly)
        #expect(PageSessions.cookie(stored) == cookie)
        let strict = try #require(PageSessions.httpCookie(PageCookie(name: "a", value: "b", domain: "example.com", sameSite: "Strict")))
        #expect(PageSessions.cookie(strict).sameSite == "Strict")
    }

    /// The session frames carry the session whole, with the wire's field names.
    @Test func sessionFramesEncodeAndDecode() throws {
        let session = PageSession(origin: "https://example.com", cookies: [PageCookie(name: "sid", value: "abc", domain: "example.com", httpOnly: true)],
                                  local: ["theme": "dark"], session: ["tab": "2"])
        let put = try ClientMsg.paneSessionPut(req: 4, pane: "p2", session: session).json()
        #expect(put.contains(#""http_only":true"#))
        #expect(put.contains(#""op":"pane_session_put""#))
        #expect(try ClientMsg.paneShareSession(req: 5, pane: "p2", share: nil).json() == #"{"op":"pane_share_session","pane":"p2","req":5}"#)
        #expect(try ClientMsg.paneShareSession(req: 6, pane: "p2", share: false).json() == #"{"op":"pane_share_session","pane":"p2","req":6,"share":false}"#)
        #expect(try ClientMsg.sessionAnswer(id: 7, session: nil).json() == #"{"id":7,"op":"session_answer"}"#)
        let wire = #"{"ev":"pane_session","req":4,"pane":"p2","session":{"origin":"https://example.com","cookies":[{"name":"sid","value":"abc","domain":"example.com","path":"/","secure":false,"http_only":true}],"local":{"theme":"dark"},"session":{"tab":"2"}}}"#
        guard case let .paneSession(req, pane, got) = try JSONDecoder().decode(ServerMsg.self, from: Data(wire.utf8)) else {
            Issue.record("not a pane_session"); return
        }
        #expect(req == 4 && pane == "p2" && got == session)
        let request = #"{"ev":"session_request","id":9,"pane":"p2","origin":"http://localhost:5173"}"#
        guard case let .sessionRequest(id, _, origin) = try JSONDecoder().decode(ServerMsg.self, from: Data(request.utf8)) else {
            Issue.record("not a session_request"); return
        }
        #expect(id == 9 && origin == "http://localhost:5173")
    }

    /// A browser pane's own sharing choice arrives in the layout, and a pane with none follows the setting.
    @Test func aBrowserPaneCarriesItsSharingChoice() throws {
        let off = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p3","kind":"browser","url":"https://example.com/","share_session":false}"#.utf8))
        #expect(off == .pane(id: "p3", kind: .browser(url: "https://example.com/", share: false)))
        let follows = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p3","kind":"browser","url":"https://example.com/"}"#.utf8))
        guard case let .pane(_, kind) = follows else { Issue.record("wrong case"); return }
        #expect(kind.browserShare == nil)
    }

    /// The storage script names the page's host and port and the snapshot drops its mark.
    @Test func theStorageScriptsAgree() {
        let script = PageSessions.storageScript(PageSession(origin: "http://localhost:5173", local: ["k": "v"]), mark: "m1")
        #expect(script.contains(#""host":"localhost""#))
        #expect(script.contains(#""port":"5173""#))
        #expect(PageSessions.storageScript(PageSession(origin: "https://example.com"), mark: "m").contains(#""port":"443""#))
        let read = PageSessions.storage(fromSnapshot: #"{"local":{"a":"1"},"session":{}}"#)
        #expect(read?.local == ["a": "1"])
        #expect(PageSessions.snapshotScript.contains(PageSessions.markKey))
    }
}
