import Foundation
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

    @Test func aWebImageIsAPlaceholderLinkUntilLoaded() {
        let source = "![a](https://x.test/a.png)\nhttps://y.test/b.jpg\n![l](/tmp/c.png)"
        let waiting = ChatImages.rewritten(source, inline: true, loaded: { _ in false })
        #expect(waiting.pending == ["https://x.test/a.png", "https://y.test/b.jpg"])
        #expect(waiting.markdown.contains("[Load image from x.test](\(ChatLinkAction.url(loadImage: "https://x.test/a.png")))"))
        #expect(waiting.markdown.hasSuffix("![l](/tmp/c.png)"))
        let released = ChatImages.rewritten(source, inline: true, loaded: { $0 == "https://x.test/a.png" })
        #expect(released.pending == ["https://y.test/b.jpg"])
        #expect(released.markdown.hasPrefix("![a](https://x.test/a.png)\n[Load image from y.test]"))
    }

    @Test func theLoadLinkReadsBackAsALoadAction() {
        let url = URL(string: ChatLinkAction.url(loadImage: "https://x.test/a.png?v=1&w=2"))!
        #expect(ChatLinkAction(url: url) == .loadImage("https://x.test/a.png?v=1&w=2"))
        #expect(ChatLinkAction(url: URL(string: "wheel-link://image?url=/etc/passwd")!) == nil)
    }

    @Test func withImagesOffAMarkdownImageIsALinkAndABarePathIsUntouched() {
        let source = "![shot](/tmp/a.png) and ![](https://x.test/b.gif) and `![c](d.png)`\n/tmp/shot.png"
        let out = ChatImages.rewrite(source, inline: false)
        #expect(out == "[shot](\(ChatLinkAction.url(file: "/tmp/a.png", line: nil))) and [https://x.test/b.gif](https://x.test/b.gif) and `![c](d.png)`\n/tmp/shot.png")
    }
}
