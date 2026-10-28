import Foundation
import Testing
@testable import WheelKit

@Suite struct ChatLinksTests {
    /// Every file exists at its own path with a `/r/` prefix; pane p1958 exists.
    private let knowledge = ChatLinkKnowledge(
        file: { path in
            path.hasSuffix("nope.swift") ? .missing : path.hasSuffix("pending.swift") ? .unknown
                : .found(path.hasPrefix("/") || path.hasPrefix("~") ? path : "/r/\(path)")
        },
        pane: { $0 == "p1958" ? "p1958 · Claude" : nil },
        artifact: { $0 == "dash" })

    private func rewrite(_ s: String, _ settings: ChatLinkSettings = ChatLinkSettings()) -> String {
        ChatLinks.rewrite(s, settings: settings, knowledge: knowledge)
    }

    @Test func linksAnExistingPathWithItsLineAndLeavesAMissingOneAlone() {
        let out = rewrite("See RowView.swift:160 and nope.swift:3, also pending.swift.")
        #expect(out == "See [RowView.swift:160](wheel-link://file?path=/r/RowView.swift&line=160) and nope.swift:3, also pending.swift.")
    }

    @Test func aHomePathShowsItsNameOrItsFullPath() {
        let name = rewrite("Read ~/Projects/wheel/DESIGN.md.")
        #expect(name == "Read [DESIGN.md](wheel-link://file?path=~/Projects/wheel/DESIGN.md).")
        var full = ChatLinkSettings()
        full.fileLabel = .full
        #expect(rewrite("Read ~/Projects/wheel/DESIGN.md.", full) == "Read [\\~/Projects/wheel/DESIGN.md](wheel-link://file?path=~/Projects/wheel/DESIGN.md).")
    }

    @Test func filesOffLeavesPathsAsText() {
        var off = ChatLinkSettings()
        off.files = .off
        #expect(rewrite("See RowView.swift:160", off) == "See RowView.swift:160")
    }

    @Test func aPathThatIsAWholeCodeSpanStaysCode() {
        #expect(rewrite("Edit `a/b.swift:7` now") == "Edit [`b.swift:7`](wheel-link://file?path=/r/a/b.swift&line=7) now")
        #expect(rewrite("Run `swift build a.b` now") == "Run `swift build a.b` now")
    }

    @Test func codeFencesAndExistingLinksAreLeftAlone() {
        let source = "```\nRowView.swift:1\n```\n[RowView.swift](https://x.test/a.swift) and <https://x.test/b>"
        #expect(rewrite(source) == source)
    }

    @Test func paneIdsLinkOnlyWhenThePaneExists() {
        #expect(rewrite("Look at p1958 and p99999.") == "Look at [p1958 · Claude](wheel-link://pane?id=p1958) and p99999.")
        var off = ChatLinkSettings()
        off.panes = false
        #expect(rewrite("Look at p1958", off) == "Look at p1958")
    }

    @Test func webLinksAndArtifactNamesLink() {
        #expect(rewrite("Open https://example.com/a.txt.") == "Open [https://example.com/a.txt](https://example.com/a.txt).")
        #expect(rewrite("Open artifact dash, not artifact other.") == "Open [artifact dash](wheel-link://artifact?name=dash), not artifact other.")
    }

    @Test func wordsThatAreNotPathsAreNotAsked() {
        let targets = ChatLinks.targets(in: "1.5 times, e.g. this, and/or v2.0 or a.md plus src/x.rs twice src/x.rs", settings: ChatLinkSettings())
        #expect(targets == [.file(path: "and/or", line: nil), .file(path: "src/x.rs", line: nil)])
    }

    @Test func aClickReadsBackWhatTheLinkNamed() throws {
        let link = try #require(URL(string: "wheel-link://file?path=/r/a%20b.swift&line=9"))
        #expect(ChatLinkAction(url: link) == .file(path: "/r/a b.swift", line: 9))
        #expect(ChatLinkAction(url: URL(string: "wheel-link://pane?id=p7")!) == .pane("p7"))
        #expect(ChatLinkAction(url: URL(string: "https://example.com/x")!) == .web("https://example.com/x"))
        #expect(ChatLinkAction(url: URL(string: "mailto:a@b.c")!) == nil)
    }

    @Test func aLongerFenceIsNotClosedByAShorterOne() {
        let source = "````\n```\nRowView.swift:1\n````\nRowView.swift:2"
        #expect(rewrite(source) == "````\n```\nRowView.swift:1\n````\n[RowView.swift:2](wheel-link://file?path=/r/RowView.swift&line=2)")
    }

    @Test func theLabelIsTheNameTheReplyWrote() {
        let symlinked = ChatLinkKnowledge(file: { _ in .found("/r/v3-2026.log") })
        let out = ChatLinks.rewrite("See a/current.log", settings: ChatLinkSettings(), knowledge: symlinked)
        #expect(out == "See [current.log](wheel-link://file?path=/r/v3-2026.log)")
    }

    @Test func oneReplyAsksAboutAtMostTheCapOfDistinctFiles() {
        let paths = (1...40).map { "dir/f\($0).txt" }
        let targets = ChatLinks.targets(in: paths.joined(separator: "\n\n"), settings: ChatLinkSettings())
        #expect(targets.count == ChatLinks.maxFileProbes)
    }

    @Test func readsTheLinkSettings() {
        let list = [
            SettingInfo(key: "chat.links.files", value: "pane", origin: .default, default: "preview", help: ""),
            SettingInfo(key: "chat.links.file_label", value: "full", origin: .default, default: "name", help: ""),
            SettingInfo(key: "chat.links.panes", value: "false", origin: .default, default: "true", help: ""),
            SettingInfo(key: "chat.links.web", value: "system", origin: .default, default: "pane", help: ""),
        ]
        let links = ChatAppearance(settings: list).links
        #expect(links.files == .pane && links.fileLabel == .full && !links.panes && links.web == .system)
    }
}
