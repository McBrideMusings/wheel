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

    private static let patternJSON = ##"[{"match":"wheel-[a-z0-9.]+","action":"run","command":"bd show $0"},{"match":"#(\\d+)","action":"open","url":"https://github.com/McBrideMusings/wheel/issues/$1"},{"match":"sha:(\\w+)","action":"copy","text":"$1"},{"match":"(","action":"copy"}]"##

    private func withPatterns() -> ChatLinkSettings {
        var settings = ChatLinkSettings()
        settings.patterns = ChatLinkPattern.parse(Self.patternJSON)
        return settings
    }

    @Test func aPatternEntryWhoseExpressionDoesNotCompileIsLeftOut() {
        #expect(withPatterns().patterns.count == 3)
    }

    @Test func aRunPatternLinksTheMatchToACommandWithTheMatchFilledIn() {
        let out = rewrite("See wheel-62v3 now", withPatterns())
        #expect(out == "See [wheel-62v3](wheel-link://run?command=bd%20show%20wheel-62v3) now")
        let url = URL(string: "wheel-link://run?command=bd%20show%20wheel-62v3")!
        #expect(ChatLinkAction(url: url) == .run("bd show wheel-62v3"))
    }

    @Test func anOpenPatternLinksToTheUrlWithTheGroupFilledIn() {
        let out = rewrite("Fixed in #123.", withPatterns())
        #expect(out == "Fixed in [#123](https://github.com/McBrideMusings/wheel/issues/123).")
    }

    @Test func aCopyPatternCopiesItsTemplate() {
        let out = rewrite("at sha:abc123", withPatterns())
        let link = out.firstMatch(of: /\((wheel-link:[^)]*)\)/)!.1
        #expect(ChatLinkAction(url: URL(string: String(link))!) == .copy("abc123"))
    }

    @Test func aGroupWrittenIntoACommandIsQuotedAndIntoAUrlIsEncoded() {
        let run = ChatLinkPattern(match: "x=(.+)", action: .run(command: "echo $1"))!
        let open = ChatLinkPattern(match: "q=(.+)", action: .open(url: "https://example.com/?q=$1"))!
        var settings = ChatLinkSettings()
        settings.patterns = [run, open]
        let ns = "x=a b;rm it's" as NSString
        let found = run.regex.firstMatch(in: ns as String, range: NSRange(location: 0, length: ns.length))!
        #expect(run.resolved(found, in: ns) == .run(#"echo 'a b;rm it'\''s'"#))
        let q = "q=a&b=c" as NSString
        let foundQ = open.regex.firstMatch(in: q as String, range: NSRange(location: 0, length: q.length))!
        #expect(open.resolved(foundQ, in: q) == .open("https://example.com/?q=a%26b%3Dc"))
    }

    @Test func aPatternWinsOverTheBuiltInLinkForTheSameText() {
        var settings = ChatLinkSettings()
        settings.patterns = [ChatLinkPattern(match: "p1958", action: .copy(text: "$0"))!]
        #expect(rewrite("Look at p1958", settings).contains("wheel-link://copy"))
    }

    @Test func readsThePatternsSetting() {
        let list = [SettingInfo(key: "chat.links.patterns", value: Self.patternJSON, origin: .project, default: "[]", help: "")]
        #expect(ChatAppearance(settings: list).links.patterns.count == 3)
    }
}
