import Foundation
import Testing
@testable import WheelUI

@Suite struct RenderedProseTests {
    @Test func markersLeaveTheText() {
        let prose = RenderedProse.markdown("A **bold** and `code` word.")
        #expect(prose.text == "A bold and code word.")
        #expect(prose.runs.map(\.face) == [.strong, .code])
        #expect(prose.runs.map(\.range) == [2..<6, 11..<15])
    }

    @Test func blocksAreSeparatedByABlankLine() {
        let prose = RenderedProse.markdown("# Title\n\nFirst para.\n\nSecond para.")
        #expect(prose.text == "Title\n\nFirst para.\n\nSecond para.")
        #expect(prose.runs.first?.face == .heading(1))
    }

    @Test func listItemsKeepTheirMarkers() {
        #expect(RenderedProse.markdown("- one\n- two").text == "• one\n• two")
        #expect(RenderedProse.markdown("1. one\n2. two").text == "1. one\n2. two")
    }

    @Test func wordAndParagraphRanges() {
        let prose = RenderedProse.markdown("alpha **bravo** charlie\n\nsecond block")
        #expect(prose.text == "alpha bravo charlie\n\nsecond block")
        #expect(prose.wordRange(at: 8) == 6..<11)
        #expect(prose.wordRange(at: 11) == 6..<11)
        #expect(prose.paragraphRange(at: 3) == 0..<19)
        #expect(prose.paragraphRange(at: 30) == 21..<33)
    }
}
