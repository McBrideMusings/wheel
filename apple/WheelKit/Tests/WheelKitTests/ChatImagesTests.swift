import Testing
@testable import WheelKit

@Suite struct ChatImagesTests {
    @Test func aLineOfOnlyAnImagePathBecomesAMarkdownImage() {
        let source = "Here:\n/tmp/shot.PNG\n~/a/b.jpeg\nhttps://x.test/c.webp?v=2\nsee /tmp/inline.png in text\n/tmp/notes.txt"
        let out = ChatImages.rewrite(source, inline: true)
        #expect(out == "Here:\n![](/tmp/shot.PNG)\n![](~/a/b.jpeg)\n![](https://x.test/c.webp?v=2)\nsee /tmp/inline.png in text\n/tmp/notes.txt")
    }

    @Test func fencedCodeIsLeftAlone() {
        let source = "```\n/tmp/shot.png\n```\n/tmp/shot.png"
        #expect(ChatImages.rewrite(source, inline: true) == "```\n/tmp/shot.png\n```\n![](/tmp/shot.png)")
    }

    @Test func withImagesOffAMarkdownImageIsALinkAndABarePathIsUntouched() {
        let source = "![shot](/tmp/a.png) and ![](https://x.test/b.gif) and `![c](d.png)`\n/tmp/shot.png"
        let out = ChatImages.rewrite(source, inline: false)
        #expect(out == "[shot](\(ChatLinkAction.url(file: "/tmp/a.png", line: nil))) and [https://x.test/b.gif](https://x.test/b.gif) and `![c](d.png)`\n/tmp/shot.png")
    }
}
