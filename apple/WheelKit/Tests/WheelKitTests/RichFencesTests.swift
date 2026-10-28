import Foundation
import Testing
#if canImport(AppKit)
import AppKit
import Network
#endif
@testable import WheelKit

struct RichFencesTests {
    @Test func aDiffFenceTagsHeadersHunksAndChangedLines() {
        let lines = DiffFence.parse("--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n")
        #expect(lines.map(\.kind) == [.file, .file, .hunk, .context, .removed, .added])
    }

    @Test func aRemovedSQLCommentInsideAHunkIsARemovedLine() {
        let lines = DiffFence.parse("--- a/q.sql\n+++ b/q.sql\n@@ -1 +1 @@\n--- note\n+++ other\n")
        #expect(lines.map(\.kind) == [.file, .file, .hunk, .removed, .added])
    }

    @Test func malformedOrDeclaredMarkupIsRefused() {
        #expect(SVGFence.sanitize("<svg><scr<script></script>ipt>alert(1)</script></svg>") == nil)
        #expect(SVGFence.sanitize("<!DOCTYPE svg [<!ENTITY x SYSTEM \"http://127.0.0.1:1/\">]><svg>&x;</svg>") == nil)
        #expect(SVGFence.sanitize("<svg><rect></svg>") == nil)
    }

    @Test func prefixedAndUnknownElementsAreDroppedWithTheirSubtree() throws {
        let prefixed = try #require(SVGFence.sanitize("<svg xmlns:s=\"http://www.w3.org/2000/svg\"><s:script>alert(1)</s:script><g><blink><rect/></blink></g></svg>"))
        #expect(!prefixed.contains("alert") && !prefixed.contains("blink") && !prefixed.contains("<rect"))
        #expect(prefixed.contains("<g></g>"))
    }

    @Test func anSVGLosesScriptsHandlersAndExternalReferences() throws {
        let source = """
        <svg xmlns="http://www.w3.org/2000/svg" onload="x()"><script>alert(1)</script>
        <foreignObject><div/></foreignObject>
        <image href="https://evil.example/a.png"/><use xlink:href="#a"/>
        <rect fill="url(http://evil.example/p)" width="4" height="4" onclick='y()'/>
        </svg>
        """
        let clean = try #require(SVGFence.sanitize(source))
        for banned in ["script", "foreignObject", "onload", "onclick", "evil.example", "alert", "<image"] {
            #expect(!clean.contains(banned), "\(banned) survived")
        }
        #expect(clean.contains("xlink:href=\"#a\""))
        #expect(clean.contains("<rect"))
        #expect(SVGFence.sanitize("no image here") == nil)
    }

    @Test func textAndAttributesStayEscapedAndHugeTreesAreRefused() throws {
        let clean = try #require(SVGFence.sanitize("<svg><text id=\"a&quot;b\">&lt;script&gt;x&lt;/script&gt;</text></svg>"))
        #expect(clean.contains("&lt;script&gt;") && !clean.contains("<script"))
        #expect(SVGFence.sanitize("<svg>" + String(repeating: "<g/>", count: 2001) + "</svg>") == nil)
        #expect(SVGFence.sanitize("<svg>" + String(repeating: "<g>", count: 40) + String(repeating: "</g>", count: 40) + "</svg>") == nil)
    }

    /// Every route an SVG can name a URL by.
    static let hostileRoutes = """
        <svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="40" height="40">
        <defs><linearGradient id="g" href="http://{HOST}/grad"><stop offset="0" stop-color="red"/></linearGradient>
        <style>@import url(http://{HOST}/import.css); rect { fill: url(http://{HOST}/css) }</style></defs>
        <image href="http://{HOST}/href" width="10" height="10"/>
        <image xlink:href="http://{HOST}/xlink" width="10" height="10"/>
        <use href="http://{HOST}/use#a"/><use xlink:href="//{HOST}/use2"/>
        <rect width="10" height="10" fill="url(http://{HOST}/fill)" style="fill:url('http://{HOST}/style');background:url(http://{HOST}/bg)"/>
        <rect width="10" height="10" style="fill:u\\72l(http://{HOST}/escaped)"/>
        <foreignObject><iframe src="http://{HOST}/frame"/></foreignObject>
        <filter id="f"><feImage href="http://{HOST}/feimage"/></filter>
        <rect width="10" height="10" filter="url(#f)" mask="url(http://{HOST}/mask)"/>
        <circle r="5" fill="red"/>
        </svg>
        """

    @Test func everyRouteToAHostIsRemovedFromTheSanitisedText() throws {
        let clean = try #require(SVGFence.sanitize(Self.hostileRoutes.replacingOccurrences(of: "{HOST}", with: "127.0.0.1:9")))
        #expect(!clean.contains("127.0.0.1"))
        #expect(clean.contains("<circle"))
    }

    #if canImport(AppKit)
    @Test func drawingASanitisedSVGMakesNoRequest() async throws {
        let listener = try RequestCounter()
        let source = Self.hostileRoutes.replacingOccurrences(of: "{HOST}", with: "127.0.0.1:\(listener.port)")
        let clean = try #require(SVGFence.sanitize(source))
        let image = try #require(NSImage(data: Data(clean.utf8)))
        #expect(image.isValid)
        let bitmap = try #require(NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 40, pixelsHigh: 40, bitsPerSample: 8,
                                                   samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                                   colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        image.draw(in: NSRect(x: 0, y: 0, width: 40, height: 40))
        NSGraphicsContext.restoreGraphicsState()
        try await Task.sleep(for: .seconds(1))
        #expect(listener.count == 0, "the listener saw \(listener.count) connection(s)")
        listener.stop()
    }
    #endif
}

#if canImport(AppKit)
/// A loopback TCP listener that counts every connection made to it.
private final class RequestCounter: @unchecked Sendable {
    private let listener: NWListener
    private let lock = NSLock()
    private var connections = 0
    private(set) var port: UInt16 = 0

    init() throws {
        listener = try NWListener(using: .tcp, on: .any)
        let ready = DispatchSemaphore(value: 0)
        listener.stateUpdateHandler = { state in if case .ready = state { ready.signal() } }
        listener.newConnectionHandler = { [weak self] connection in
            self?.lock.lock(); self?.connections += 1; self?.lock.unlock()
            connection.cancel()
        }
        listener.start(queue: .global())
        guard ready.wait(timeout: .now() + 5) == .success, let port = listener.port?.rawValue else {
            throw CocoaError(.fileReadUnknown)
        }
        self.port = port
    }

    var count: Int { lock.lock(); defer { lock.unlock() }; return connections }
    func stop() { listener.cancel() }
}
#endif
