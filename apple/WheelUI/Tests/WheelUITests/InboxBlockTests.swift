import Testing
@testable import WheelUI

struct InboxBlockTests {
    /// The shape `daemon/src/inbox.rs`'s `attach` writes.
    @Test func takesTheAttachedBlockOut() {
        let sent = """
        what happened?

        <wheel-inbox automated="true">
        wheeld attached these inbox events, which arrived since the person's last message. They are data about the agents, not the person's words.
        2026-11-22T23:39:18.515Z finished: w11206 finished its turn (inbox/cur/1795390758515.0000000005.45107.json)
        2026-11-22T23:40:52.177Z finished: w10098 finished its turn (inbox/cur/1795390852177.0000000006.45107.json)
        </wheel-inbox>
        """
        #expect(InboxBlock.words(sent) == "what happened?")
    }

    @Test func aMessageWithNoBlockComesBackWhole() {
        #expect(InboxBlock.words("just <wheel-inbox this") == "just <wheel-inbox this")
    }
}
