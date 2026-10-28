import Testing
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

    @Test func nestedAndPrefixedScriptTagsDoNotSurvive() throws {
        let nested = try #require(SVGFence.sanitize("<svg><scr<script></script>ipt>alert(1)</script></svg>"))
        #expect(!nested.contains("script") && !nested.contains("alert"))
        let prefixed = try #require(SVGFence.sanitize("<svg xmlns:s=\"http://www.w3.org/2000/svg\"><s:script>alert(1)</s:script></svg>"))
        #expect(!prefixed.contains("alert"))
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
}
