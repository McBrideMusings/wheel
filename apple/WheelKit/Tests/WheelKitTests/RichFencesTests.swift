import Testing
@testable import WheelKit

struct RichFencesTests {
    @Test func aDiffFenceTagsHeadersHunksAndChangedLines() {
        let lines = DiffFence.parse("--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n")
        #expect(lines.map(\.kind) == [.file, .file, .hunk, .context, .removed, .added])
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
