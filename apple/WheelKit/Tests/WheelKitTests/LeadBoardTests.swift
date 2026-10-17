import Foundation
import Testing
@testable import WheelKit

struct LeadBoardTests {
    @Test func decodesTheLeadsInboxAndItsBadge() throws {
        let list = #"{"ev":"inbox","req":4,"items":[{"id":"i1","source":{"space":"s1","pane":"p2","session":"a1"},"kind":"permission","time":5,"state":"surfaced","detail":"rm -rf build","perm":"perm-1"},{"id":"i2","source":{"space":"s1","pane":"p3","session":"a2"},"kind":"turn_ended","time":3,"state":"closed"}]}"#
        guard case let .inbox(req, items) = try ServerMsg.decode(list) else { Issue.record("wrong case"); return }
        #expect(req == 4)
        #expect(items.map(\.kind) == [.permission, .turnEnded])
        #expect(items.map(\.state) == [.surfaced, .closed])
        #expect(items.first?.detail == "rm -rf build")
        #expect(items.last?.detail == nil)
        guard case let .inboxCount(new, open) = try ServerMsg.decode(#"{"ev":"inbox_count","new":1,"open":3}"#) else { Issue.record("wrong case"); return }
        #expect((new, open) == (1, 3))
        #expect(try ClientMsg.inboxLs(req: 9).json() == #"{"op":"inbox_ls","req":9}"#)
    }
}
