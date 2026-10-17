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
}
