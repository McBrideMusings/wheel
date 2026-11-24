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
        // No URL is left to fetch: every href, src and url() that remains is a fragment.
        #expect(clean.range(of: #"(?i)(href|src)\s*=\s*["'](?!#)"#, options: .regularExpression) == nil)
        #expect(clean.range(of: #"(?i)url\(\s*["']?(?!#)"#, options: .regularExpression) == nil)
        #expect(!clean.contains("@import"))
    }

}
