import Foundation
import Testing
@testable import WheelKit

struct StopReasonTests {
    @Test func aNormalEndNeedsNoWords() {
        #expect(StopReason.words(for: "end_turn") == nil)
    }

    @Test func eachKnownReasonHasItsWords() {
        #expect(StopReason.words(for: "max_tokens") == "Stopped at its length limit")
        #expect(StopReason.words(for: "max_turn_requests") == "Stopped at its request limit")
        #expect(StopReason.words(for: "refusal") == "Declined to continue")
        #expect(StopReason.words(for: "cancelled") == "Cancelled")
    }

    @Test func anUnknownReasonIsSpelledOutNotShownAsAToken() {
        #expect(StopReason.words(for: "some_new_reason") == "Some New Reason")
    }

    @Test func theLeadCardWordsOnlyAFinishedTurnsReason() throws {
        func item(_ json: String) throws -> InboxItem {
            try JSONDecoder().decode(InboxItem.self, from: Data(json.utf8))
        }
        let source = #""source":{"space":"s1","pane":"p2","session":"a1"}"#
        let lengthLimit = try item(#"{"id":"i1",\#(source),"kind":"turn_ended","time":3,"state":"closed","detail":"max_tokens"}"#)
        let normalEnd = try item(#"{"id":"i2",\#(source),"kind":"turn_ended","time":3,"state":"closed","detail":"end_turn"}"#)
        let noReason = try item(#"{"id":"i3",\#(source),"kind":"turn_ended","time":3,"state":"closed"}"#)
        let permission = try item(#"{"id":"i4",\#(source),"kind":"permission","time":5,"state":"surfaced","detail":"rm -rf build"}"#)
        #expect(lengthLimit.shownDetail == "Stopped at its length limit")
        #expect(normalEnd.shownDetail == nil)
        #expect(noReason.shownDetail == nil)
        #expect(permission.shownDetail == "rm -rf build")
    }
}
