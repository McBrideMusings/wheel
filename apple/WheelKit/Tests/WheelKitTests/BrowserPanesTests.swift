import Foundation
import Testing
@testable import WheelKit

struct BrowserPanesTests {
    /// Only a loopback address is this machine; a DNS name that starts with 127. is not.
    @Test func onlyALoopbackAddressIsLoopback() {
        let hosts = ["ws://127.0.0.1:7420/ws", "ws://localhost:7420/ws", "ws://[::1]:7420/ws", "ws://10.0.0.5:7420/ws", "wss://box.example/ws", "ws://127.attacker.example/ws", "ws://127.0.0.256/ws"]
        let local = hosts.map { DaemonEndpoint(url: URL(string: $0)!, token: { "" }).isLoopback }
        #expect(local == [true, true, true, false, false, false, false])
    }

    /// A page of the Mac's loopback loads on the phone under the loopback scheme with its host
    /// kept, comes back as the same page, and its requests go to the daemon's `/loopback/<port>`.
    @Test func loopbackPagesRoundTripThroughTheirScheme() {
        let phone = BrowserPanes.phonePage("http://localhost:5173/app?x=1#top")
        #expect(phone?.absoluteString == "wheel-loopback://localhost:5173/app?x=1#top")
        #expect(phone.map(BrowserPanes.macPage) == "http://localhost:5173/app?x=1#top")
        #expect(BrowserPanes.phonePage("http://127.0.0.1:8080")?.absoluteString == "wheel-loopback://127.0.0.1:8080/")
        #expect(BrowserPanes.phonePage("http://LOCALHOST/")?.absoluteString == "wheel-loopback://localhost:80/")
        #expect(BrowserPanes.phonePage("http://localhost/").map(BrowserPanes.macPage) == "http://localhost/")
        #expect(BrowserPanes.phonePage("https://localhost:5173/") == nil)
        #expect(BrowserPanes.phonePage("https://example.com/") == nil)
        #expect(BrowserPanes.phonePage("http://localhost.example.com:5173/") == nil)
        #expect(BrowserPanes.macPage(URL(string: "https://example.com/a")!) == "https://example.com/a")

        let daemon = URL(string: "wss://mac.local:7421/ws?token=t")!
        let request = URL(string: "wheel-loopback://localhost:5173/assets/app.js?v=2")!
        #expect(BrowserPanes.loopbackDaemonURL(for: request, daemon: daemon)?.absoluteString == "https://mac.local:7421/loopback/5173/assets/app.js?v=2")
        #expect(BrowserPanes.loopbackDaemonURL(for: URL(string: "wheel-page://pages/artifacts/p1/")!, daemon: daemon) == nil)
    }
}
