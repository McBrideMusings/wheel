#if os(macOS)
import AppKit
import Foundation
import Testing
import WebKit
import WheelKit
@testable import WheelUI

/// A stand-in wheeld for one artifact page: `/artifacts/p1/` is a page that opens its event stream, and
/// `/artifacts/p1/.wheel/events` is that stream, held open until the client closes it. It counts the streams open now.
private final class StandIn: @unchecked Sendable {
    let port: UInt16
    private let listener: Int32
    private let lock = NSLock()
    private var open = 0
    private var opened = 0

    init() throws {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        addr.sin_port = 0
        var len = socklen_t(MemoryLayout<sockaddr_in>.size)
        _ = withUnsafePointer(to: &addr) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, len) } }
        listen(fd, 16)
        _ = withUnsafeMutablePointer(to: &addr) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(fd, $0, &len) } }
        listener = fd
        port = UInt16(bigEndian: addr.sin_port)
        Thread.detachNewThread { [self] in
            while true {
                let peer = accept(listener, nil, nil)
                if peer < 0 { return }
                Thread.detachNewThread { [self] in serve(peer) }
            }
        }
    }

    var streams: (open: Int, opened: Int) { lock.withLock { (open, opened) } }

    private func write(_ fd: Int32, _ text: String) {
        _ = text.utf8CString.withUnsafeBufferPointer { Darwin.write(fd, $0.baseAddress, $0.count - 1) }
    }

    private func serve(_ fd: Int32) {
        defer { close(fd) }
        var buffer = [UInt8](repeating: 0, count: 8192)
        let count = read(fd, &buffer, buffer.count)
        guard count > 0 else { return }
        let head = String(decoding: buffer[..<count], as: UTF8.self)
        if head.hasPrefix("GET /artifacts/p1/.wheel/events") {
            write(fd, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\ndata: hi\n\n")
            lock.withLock { open += 1; opened += 1 }
            // Held until the client closes its end.
            while read(fd, &buffer, buffer.count) > 0 {}
            lock.withLock { open -= 1 }
        } else {
            let page = "<html><body>page<script>new EventSource('.wheel/events').onmessage = (e) => { document.title = e.data }</script></body></html>"
            write(fd, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: \(page.utf8.count)\r\nConnection: close\r\n\r\n\(page)")
        }
    }
}

@MainActor @Suite struct PageSchemeHandlerCloseTests {
    /// Waits up to 10 s for `done`.
    private func until(_ done: () -> Bool) async -> Bool {
        let end = Date().addingTimeInterval(10)
        while Date() < end {
            if done() { return true }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return done()
    }

    /// A page web view that is dropped closes its event stream to wheeld: WebKit never stops the request of a view that is
    /// only removed, so `PageSchemeHandler.close` must.
    @Test func closingThePageEndsItsEventStream() async throws {
        let server = try StandIn()
        let daemon = PageDaemon()
        daemon.url = URL(string: "ws://127.0.0.1:\(server.port)/ws")
        daemon.token = "t"
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        let handler = PageSchemeHandler(pane: "p1", daemon: daemon)
        config.setURLSchemeHandler(handler, forURLScheme: WebPanes.scheme)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 200, height: 200), styleMask: [.borderless], backing: .buffered, defer: true)
        var web: WKWebView? = WKWebView(frame: NSRect(x: 0, y: 0, width: 200, height: 200), configuration: config)
        window.contentView?.addSubview(web!)
        web!.load(URLRequest(url: WebPanes.pageURL(pane: "p1")!))
        #expect(await until { server.streams.open == 1 }, "the page opened its event stream")

        PageSchemeHandler.close(web!)
        web!.removeFromSuperview()
        web = nil
        #expect(await until { server.streams.open == 0 }, "the stream is still open: \(server.streams)")
        #expect(handler.runningCount == 0)
    }

    /// With a `PageFeed` the page's event stream rides the app's connection: the page opens no stream of its own to
    /// wheeld, gets each `pageData` value, and closing it stops the watch.
    @Test func aFeedCarriesThePagesEventStream() async throws {
        let server = try StandIn()
        let daemon = PageDaemon()
        daemon.url = URL(string: "ws://127.0.0.1:\(server.port)/ws")
        daemon.token = "t"
        var sent: [ClientMsg] = []
        let feed = PageFeed { sent.append($0) }
        daemon.feed = feed
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        let handler = PageSchemeHandler(pane: "p1", daemon: daemon)
        config.setURLSchemeHandler(handler, forURLScheme: WebPanes.scheme)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 200, height: 200), styleMask: [.borderless], backing: .buffered, defer: true)
        var web: WKWebView? = WKWebView(frame: NSRect(x: 0, y: 0, width: 200, height: 200), configuration: config)
        window.contentView?.addSubview(web!)
        web!.load(URLRequest(url: WebPanes.pageURL(pane: "p1")!))
        #expect(await until { sent == [.pageWatch(req: nil, pane: "p1")] }, "the page's stream watched p1: \(sent)")
        feed.handle(.pageData(pane: "p1", data: #"{"n":1}"#))
        #expect(await until { web?.title == #"{"n":1}"# }, "the page got the value: \(web?.title ?? "-")")
        #expect(server.streams.opened == 0, "the page opened its own stream: \(server.streams)")

        PageSchemeHandler.close(web!)
        web!.removeFromSuperview()
        web = nil
        #expect(await until { sent.last == .pageUnwatch(req: nil, pane: "p1") }, "the watch was not stopped: \(sent)")
        #expect(feed.streams("p1") == 0)
    }
}
#endif
