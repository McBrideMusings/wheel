import Foundation
import Testing
@testable import WheelKit

struct SpokenTextTests {
    @Test func codeBlocksAreSkippedAndInlineMarksRemoved() {
        let reply = """
        ## Done

        I changed `main.rs` and **fixed** the [bug](https://x.test).

        ```rust
        fn main() {}
        ```

        - first
        - second
        """
        #expect(SpokenText.from(reply) == "Done. I changed main.rs and fixed the bug. first. second.")
    }

    @Test func namesKeepTheirUnderscoresAndLinksAreNotReadOut() {
        #expect(SpokenText.from("Call read_aloud on _this_ and see https://x.test/a_b now") == "Call read_aloud on this and see a link now.")
    }

    @Test func emptyReplySaysNothing() {
        #expect(SpokenText.from("```\ncode\n```").isEmpty)
    }

    @Test func lastReplyIsTheAgentTextAfterTheLastSentMessage() {
        var transcript = Transcript()
        transcript.apply(.user(turn: 1, text: "one"))
        transcript.apply(.turnStart(turn: 1))
        transcript.apply(.text("first answer"))
        transcript.apply(.turnEnd(turn: 1, stopReason: "end_turn"))
        transcript.apply(.user(turn: 2, text: "two"))
        transcript.apply(.turnStart(turn: 2))
        transcript.apply(.text("second "))
        transcript.apply(.toolCall(id: "t", title: "ls", kind: "execute", status: "completed", content: []))
        transcript.apply(.text("answer"))
        transcript.apply(.turnEnd(turn: 2, stopReason: "end_turn"))
        #expect(transcript.lastReply == "second \n\nanswer")
    }
}
