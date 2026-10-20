import Foundation
import Testing
@testable import WheelKit

struct ManagedPanesTests {
    @Test func encodesTheUnlockAndHandBackOps() throws {
        let unlock = try ClientMsg.paneUnlock(req: 4, pane: "p7").json()
        #expect(unlock.contains(#""op":"pane_unlock""#) && unlock.contains(#""pane":"p7""#) && unlock.contains(#""req":4"#))
        #expect(try ClientMsg.paneHandBack(req: 5, pane: "p7").json().contains(#""op":"pane_hand_back""#))
    }

    @Test func switchedOffEveryPaneIsTheirsAndSwitchedOnTheManagedOnesLock() {
        #expect(ManagedPane.lock(createdBy: .lead, prior: nil, locking: false) == .yours)
        #expect(ManagedPane.lock(createdBy: .person, prior: .lead, locking: false) == .yours)
        #expect(ManagedPane.lock(createdBy: .lead, prior: nil, locking: true) == .locked)
        #expect(ManagedPane.lock(createdBy: .person, prior: .lead, locking: true) == .unlocked)
    }

    @Test func aStatusCarriesTheDaemonsPaneLockingSwitch() throws {
        let base = #"{"ev":"status","version":"0","uptime_secs":1,"log_path":"/l","terms":[],"perms":[],"spaces":[],"active":null"#
        for (tail, expected) in [("}", false), (#","pane_locking":true}"#, true)] {
            guard case let .status(_, _, _, _, _, _, _, _, _, locking, _) = try ServerMsg.decode(base + tail) else { Issue.record("wrong case"); return }
            #expect(locking == expected)
        }
    }

    @Test func aLayoutKeepsWhoManagedAnUnlockedPane() throws {
        let node = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p1","created_by":"person","prior":{"supervisor":{"id":"s1"}},"kind":"worker","session":"w1"}"#.utf8))
        let layout = GridLayout(node, in: CGRect(x: 0, y: 0, width: 100, height: 100))
        #expect(layout.panes.first?.createdBy == .person)
        #expect(layout.panes.first?.prior == .supervisor(id: "s1"))
    }
}
