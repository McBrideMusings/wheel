import Foundation
import Testing
import WheelKit
@testable import WheelUI

/// `PageFeed` across more than one page of a pane and a reconnect: what each page receives and what goes to wheeld.
@MainActor @Suite struct PageFeedTests {
    /// One page reading its stream on a task, as PageSchemeHandler does: the frames it got and whether the stream failed.
    @MainActor private final class Page {
        var frames: [String] = []
        var failed = false
        init(_ stream: AsyncThrowingStream<Data, Error>) {
            Task { @MainActor in
                do { for try await data in stream { frames.append(String(decoding: data, as: UTF8.self)) } } catch { failed = true }
            }
        }
    }

    private func until(_ done: () -> Bool) async -> Bool {
        let end = Date().addingTimeInterval(5)
        while !done(), Date() < end { try? await Task.sleep(for: .milliseconds(20)) }
        return done()
    }

    @Test func pagesOfOnePaneShareOneWatchThroughAReconnect() async {
        var sent: [ClientMsg] = []
        let feed = PageFeed { sent.append($0) }
        let first = Page(feed.events("p1"))
        feed.handle(.pageData(pane: "p1", data: "1"))
        #expect(await until { first.frames == ["data: 1\n\n"] }, "\(first.frames)")

        // A second page of the pane gets the latest value at once and sends no second watch.
        let second = Page(feed.events("p1"))
        #expect(await until { second.frames == ["data: 1\n\n"] }, "\(second.frames)")
        #expect(sent == [.pageWatch(req: nil, pane: "p1")])

        // A new connection watches again; wheeld's replay of the value the pages hold is dropped, a new one is not.
        feed.connected()
        #expect(sent.last == .pageWatch(req: nil, pane: "p1"))
        feed.handle(.pageData(pane: "p1", data: "1"))
        feed.handle(.pageData(pane: "p1", data: "2"))
        #expect(await until { first.frames.count == 2 && second.frames.count == 2 })
        #expect(first.frames == ["data: 1\n\n", "data: 2\n\n"])
        #expect(second.frames == ["data: 1\n\n", "data: 2\n\n"])

        // A wheeld without `page_watch`: every stream ends with an error, so each page's EventSource asks again.
        feed.end()
        #expect(await until { first.failed && second.failed })
        #expect(feed.streams("p1") == 0)
    }

    @Test func theLastPageToCloseStopsTheWatch() async {
        var sent: [ClientMsg] = []
        let feed = PageFeed { sent.append($0) }
        // Each page reads its stream on a task WebKit cancels when the page stops it, as PageSchemeHandler does.
        let read = { (stream: AsyncThrowingStream<Data, Error>) in Task { for try await _ in stream {} } }
        let one = read(feed.events("p1"))
        let two = read(feed.events("p1"))
        one.cancel()
        try? await Task.sleep(for: .milliseconds(100))
        #expect(!sent.contains(.pageUnwatch(req: nil, pane: "p1")), "one page still streams")
        two.cancel()
        let end = Date().addingTimeInterval(5)
        while !sent.contains(.pageUnwatch(req: nil, pane: "p1")), Date() < end { try? await Task.sleep(for: .milliseconds(20)) }
        #expect(sent.last == .pageUnwatch(req: nil, pane: "p1"), "\(sent)")
    }
}
