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

    /// Plays daemon messages at a pane that starts on `start`, and records every page it loads.
    private struct Pane {
        var navigation = BrowserPanes.Navigation()
        var page: String
        var loads: [String] = []

        init(on start: String) {
            page = start
            navigation.went(start)
            load(start)
        }

        mutating func load(_ url: String) {
            loads.append(url)
            page = url
        }

        mutating func type() { navigation.asked() }
        mutating func opened(_ url: String) { if let t = navigation.opened(url) { load(t) } }
        mutating func broadcast(_ url: String, failed: Bool = false) {
            if let t = navigation.daemonSays(url, page: page, failed: failed) { load(t) }
        }
    }

    @Test func replyBeforeBroadcastLoadsOnce() {
        var pane = Pane(on: "https://home.example/")
        pane.type()
        pane.opened("https://typed.example/")
        pane.broadcast("https://typed.example/")
        #expect(pane.loads == ["https://home.example/", "https://typed.example/"])
        #expect(pane.page == "https://typed.example/")
    }

    @Test func broadcastBeforeReplyLoadsOnce() {
        var pane = Pane(on: "https://home.example/")
        pane.type()
        pane.broadcast("https://typed.example/")
        pane.opened("https://typed.example/")
        #expect(pane.loads == ["https://home.example/", "https://typed.example/"])
        #expect(pane.page == "https://typed.example/")
    }

    /// The daemon already holds the typed page, so it sends no broadcast; the reply alone loads it.
    @Test func unchangedPageLoadsFromTheReplyAlone() {
        var pane = Pane(on: "https://home.example/")
        pane.type()
        pane.opened("https://home.example/")
        #expect(pane.loads == ["https://home.example/", "https://home.example/"])
        #expect(pane.page == "https://home.example/")
    }

    /// Another client moves the pane before the daemon reads the typed text, so the daemon ends on
    /// the typed page: the typed page loads once, wherever the reply falls among the broadcasts.
    @Test func anotherClientMovingFirstStillEndsOnTheTypedPage() {
        for replyFirst in [true, false] {
            var pane = Pane(on: "https://home.example/")
            pane.type()
            pane.broadcast("https://other.example/")
            if replyFirst { pane.opened("https://typed.example/") }
            pane.broadcast("https://typed.example/")
            if !replyFirst { pane.opened("https://typed.example/") }
            #expect(pane.page == "https://typed.example/")
            #expect(pane.loads.filter { $0 == "https://typed.example/" }.count == 1)
        }
    }

    /// Another client sending the page the view is already on changes nothing.
    @Test func anotherClientSendingTheCurrentPageLoadsNothing() {
        var pane = Pane(on: "https://home.example/")
        pane.broadcast("https://other.example/")
        pane.page = "https://elsewhere.example/"
        pane.broadcast("https://elsewhere.example/")
        #expect(pane.loads == ["https://home.example/", "https://other.example/"])
    }

    /// With a failure showing over the page, the daemon sending that page again tries it again.
    @Test func theCurrentPageIsLoadedAgainWhileAFailureShows() {
        var pane = Pane(on: "https://home.example/")
        pane.broadcast("https://other.example/")
        pane.page = "https://elsewhere.example/"
        pane.broadcast("https://elsewhere.example/", failed: true)
        #expect(pane.loads.last == "https://elsewhere.example/")
    }

    /// Another client moves the pane after the daemon read the typed text: the pane ends on the
    /// other client's page, which the daemon holds.
    @Test func anotherClientMovingLastEndsOnItsPage() {
        var pane = Pane(on: "https://home.example/")
        pane.type()
        pane.opened("https://typed.example/")
        pane.broadcast("https://typed.example/")
        pane.broadcast("https://other.example/")
        #expect(pane.page == "https://other.example/")
        #expect(pane.loads == ["https://home.example/", "https://typed.example/", "https://other.example/"])
    }
}
