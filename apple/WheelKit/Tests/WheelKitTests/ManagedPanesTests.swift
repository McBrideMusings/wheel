import Foundation
import Testing
@testable import WheelKit

struct ManagedPanesTests {
    @Test func encodesTheUnlockAndHandBackOps() throws {
        let unlock = try ClientMsg.paneUnlock(req: 4, pane: "p7").json()
        #expect(unlock.contains(#""op":"pane_unlock""#) && unlock.contains(#""pane":"p7""#) && unlock.contains(#""req":4"#))
        #expect(try ClientMsg.paneHandBack(req: 5, pane: "p7").json().contains(#""op":"pane_hand_back""#))
    }

    @Test func aLayoutKeepsWhoManagedAnUnlockedPane() throws {
        let node = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p1","created_by":"person","prior":{"supervisor":{"id":"s1"}},"kind":"worker","session":"w1"}"#.utf8))
        let layout = GridLayout(node, in: CGRect(x: 0, y: 0, width: 100, height: 100))
        #expect(layout.panes.first?.createdBy == .person)
        #expect(layout.panes.first?.prior == .supervisor(id: "s1"))
    }
}
