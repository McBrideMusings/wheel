import Foundation
import Testing
@testable import WheelKit

struct BrowserPanesTests {
    /// A pane's own reports coming back must not move it, even when it has moved on since; a page
    /// someone else asked for must.
    @Test func ownEchoesAreIgnoredAndOthersFollowed() {
        var echoes = BrowserPanes.Echoes()
        echoes.sent("https://a.example/")
        echoes.sent("https://b.example/")
        echoes.sent("https://c.example/")
        // A full layout can skip straight to a later report, which spends the reports before it,
        // and can repeat the last one the daemon applied while the next is still on its way.
        let reported = ["https://a.example/", "https://c.example/", "https://c.example/", "https://b.example/", "https://cli.example/"]
        let own = reported.map { echoes.isOwn($0) }
        #expect(own == [true, true, true, false, false])
    }

    /// The page the daemon opened the pane on is its own too: a layout built before the daemon
    /// applied the pane's first report still carries it, and must not pull the pane back.
    @Test func thePageTheDaemonGaveIsOwnUntilReplaced() {
        var echoes = BrowserPanes.Echoes()
        echoes.adopt("https://home.example/")
        echoes.sent("https://a.example/")
        let own = ["https://home.example/", "https://a.example/", "https://cli.example/"].map { echoes.isOwn($0) }
        #expect(own == [true, true, false])
    }
}
