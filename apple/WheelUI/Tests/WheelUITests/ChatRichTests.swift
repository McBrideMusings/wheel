import Foundation
import Testing
@testable import WheelUI

@Suite struct ChatRichTests {
    private let newYork = TimeZone(identifier: "America/New_York")!
    private let now = ISO8601DateFormatter().date(from: "2026-10-28T20:00:00Z")!

    @Test func aTimestampShowsLocalTimeOnATwelveHourClock() {
        #expect(ChatRich.localTime("2026-10-28T16:04:58Z", now: now, zone: newYork) == "12:04pm EDT")
        #expect(ChatRich.localTime("2026-10-27T04:30:00+00:00", now: now, zone: newYork) == "Oct 27, 12:30am EDT")
        #expect(ChatRich.localTime("2026-10-28T16:04Z", now: now, zone: newYork) == "12:04pm EDT")
    }

    @Test func hexInlineCodeGetsASwatchAndAFenceDoesNot() {
        let reply = ChatRich.prepare("Use `#0A84ff` and `#12345` and `x`.\n\n```\n`#ffffff`\n```")
        #expect(reply.swatches == ["#0a84ff"])
        #expect(reply.markdown.contains("`#0A84ff` :swatch_0a84ff:"))
        #expect(!reply.markdown.contains("`#12345` :"))
    }

    @Test func timestampsBecomeLinksOutsideCode() {
        let reply = ChatRich.prepare("At 2026-10-28T16:04:58Z, not `2026-10-28T16:04:58Z`.", now: now, zone: newYork)
        #expect(reply.times == [.init(iso: "2026-10-28T16:04:58Z", local: "12:04pm EDT")])
        #expect(reply.markdown == "At [2026-10-28T16:04:58Z](wheel-time:2026-10-28T16:04:58Z), not `2026-10-28T16:04:58Z`.")
        #expect(ChatRich.timestamp(of: URL(string: "wheel-time:2026-10-28T16:04:58Z")!) == "2026-10-28T16:04:58Z")
    }

    @Test func mathNeedsPandocShapedDollars() {
        #expect(ChatRich.maths(in: "so $e^{i\\pi}+1=0$ holds").map(\.latex) == ["e^{i\\pi}+1=0"])
        #expect(ChatRich.maths(in: "$$\\int_0^1 x\\,dx$$").map(\.display) == [true])
        #expect(ChatRich.maths(in: "it costs $5 and $10 total").isEmpty)
        #expect(ChatRich.maths(in: "`$a$` and\n```\n$b$\n```").isEmpty)
    }
}

@Suite struct ChatRichGuardTests {
    @Test func aPriceBesideRealMathsIsNotPairedAsMaths() {
        let reply = ChatRich.prepare("$x^2$ costs $5 and $10")
        #expect(reply.markdown == "$x^2$ costs \u{FF04}5 and \u{FF04}10")
        #expect(ChatRich.prepare("costs $5 and $10").markdown == "costs $5 and $10")
    }

    @Test func urlsLinkLabelsAndMathBlocksAreLeftAlone() {
        let url = "see https://x/log?since=2026-10-28T16:04:58Z now"
        #expect(ChatRich.prepare(url).markdown == url)
        let label = "[at 2026-10-28T16:04:58Z](https://x)"
        #expect(ChatRich.prepare(label).markdown == label)
        let block = "$$\na = `#fff` 2026-10-28T16:04:58Z\n$$"
        #expect(ChatRich.prepare(block).markdown == block)
    }
}
