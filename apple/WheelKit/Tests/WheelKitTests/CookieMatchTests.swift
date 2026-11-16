import Foundation
import Testing
@testable import WheelKit

struct CookieMatchTests {
    /// Which cookies a request to a host carries, as a browser decides it.
    @Test func aCookieGoesWhereABrowserSendsIt() {
        #expect(CookieMatch.carries("example.com", to: "example.com"))
        #expect(CookieMatch.carries(".example.com", to: "www.example.com"))
        #expect(CookieMatch.carries("Example.COM", to: "example.com"))
        #expect(!CookieMatch.carries("example.com", to: "badexample.com"))
        #expect(!CookieMatch.carries("www.example.com", to: "example.com"))
        #expect(!CookieMatch.carries("example.com", to: "www.example.com"))
        #expect(!CookieMatch.carries(".example.com", to: "badexample.com"))
        #expect(CookieMatch.pathMatches("/", "/anything"))
        #expect(CookieMatch.pathMatches("/app", "/app"))
        #expect(CookieMatch.pathMatches("/app", "/app/x"))
        #expect(CookieMatch.pathMatches("/app/", "/app/x"))
        #expect(!CookieMatch.pathMatches("/app", "/application"))
        #expect(!CookieMatch.pathMatches("/app", "/"))
        #expect(CookieMatch.carries("localhost", to: "localhost"))
        #expect(!CookieMatch.carries("", to: "localhost"))
    }
}
