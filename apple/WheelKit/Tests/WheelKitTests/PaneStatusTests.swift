import Foundation
import Testing
@testable import WheelKit

struct PaneStatusTests {
    private let listing = #"{"ev":"status","req":1,"version":"x","log_path":"/l","uptime_secs":3,"terms":[],"perms":[],"active":"s1","agents":[],"spaces":[{"id":"s1","name":"tmp","cwd":"/tmp","focus":"p1","status":{"kind":"needs_you","waiting":2},"root":{"node":"split","id":"x1","axis":"horizontal","ratio":0.5,"a":{"node":"pane","id":"p1","kind":"agent","session":"w1","agent":"claude","status":{"kind":"needs_you","waiting":2}},"b":{"node":"pane","id":"p2","kind":"agent","session":"w2","agent":"claude","status":{"kind":"working"}}}}]}"#

    @Test func readsEveryPaneAndSpaceStatusFromTheListing() throws {
        guard case let .status(_, _, _, _, _, _, spaces, _, _, _, _, _, _, _) = try ServerMsg.decode(listing) else { Issue.record("wrong case"); return }
        #expect(spaces[0].status == .needsYou(waiting: 2))
        #expect(spaces[0].paneStatuses == ["p1": .needsYou(waiting: 2), "p2": .working])
        // Status is not part of what a pane is.
        #expect(spaces[0].root.panes.map(\.id) == ["p1", "p2"])
    }

    @Test func aLayoutWithoutStatusesHasNone() throws {
        let text = #"{"ev":"spaces","active":"s1","spaces":[{"id":"s1","name":"tmp","cwd":"/tmp","focus":"p1","root":{"node":"pane","id":"p1","kind":"empty"}}]}"#
        guard case let .spaces(spaces, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(spaces[0].status == nil)
        #expect(spaces[0].paneStatuses.isEmpty)
    }

    @Test func aStatusThisBuildCannotReadIsNoStatus() throws {
        let text = #"{"ev":"spaces","active":"s1","spaces":[{"id":"s1","name":"tmp","cwd":"/tmp","focus":"p1","status":{"kind":"sleeping"},"root":{"node":"pane","id":"p1","kind":"empty","status":{"kind":"sleeping"}}}]}"#
        guard case let .spaces(spaces, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(spaces[0].status == nil)
        #expect(spaces[0].paneStatuses.isEmpty)
    }

    @Test func statusChangedAppliesToThePaneAndItsSpace() throws {
        guard case let .statusChanged(space, pane, status, spaceStatus) = try ServerMsg.decode(
            #"{"ev":"status_changed","space":"s1","pane":"p2","status":{"kind":"stopped"},"space_status":{"kind":"stopped"}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(space == "s1")
        #expect(pane == "p2")
        #expect(status == .stopped)
        #expect(spaceStatus == .stopped)
        let before = SpaceInfo(id: "s1", name: "s1", cwd: "/tmp", root: .pane(id: "p2", kind: .empty), focus: "p2",
                               status: .working, paneStatuses: ["p2": .working])
        let after = before.applying(pane: pane, status: status, spaceStatus: spaceStatus)
        #expect(after.status == .stopped)
        #expect(after.paneStatuses == ["p2": .stopped])
        // A closed pane changes only the space.
        let closed = after.applying(pane: nil, status: nil, spaceStatus: nil)
        #expect(closed.status == nil)
        #expect(closed.paneStatuses == ["p2": .stopped])
    }

    @Test func aLayoutBroadcastKeepsTheStatusesOfPanesStillThere() {
        let old = SpaceInfo(id: "s1", name: "s1", cwd: "/tmp",
                            root: .split(id: "x1", axis: .horizontal, ratio: 0.5, a: .pane(id: "p1", kind: .empty), b: .pane(id: "p2", kind: .empty)),
                            focus: "p1", status: .idle, paneStatuses: ["p1": .idle, "p2": .idle])
        let new = SpaceInfo(id: "s1", name: "s1", cwd: "/tmp", root: .pane(id: "p1", kind: .empty), focus: "p1")
        let kept = new.keepingStatuses(of: old)
        #expect(kept.status == .idle)
        #expect(kept.paneStatuses == ["p1": .idle])
    }

    @Test func dotsSettingRoundTrips() {
        #expect(StatusRank.listed("needs_you, stopped") == [.needsYou, .stopped])
        #expect(StatusRank.listed("") == [])
        #expect(StatusRank.listed("nope,idle") == [.idle])
        #expect(StatusRank.setting([.idle, .needsYou]) == "needs_you,idle")
    }

    @Test func aSpaceDrawsOneMarkPerAgentMostUrgentFirstAndCountsTheRest() {
        func pane(_ n: Int) -> Node { .pane(id: "p\(n)", kind: .empty) }
        let root = Node.split(id: "x", axis: .horizontal, ratio: 0.5, a: pane(1), b: .split(id: "y", axis: .horizontal, ratio: 0.5, a: pane(2), b: .split(id: "z", axis: .horizontal, ratio: 0.5, a: pane(3), b: .split(id: "w", axis: .horizontal, ratio: 0.5, a: pane(4), b: .split(id: "v", axis: .horizontal, ratio: 0.5, a: pane(5), b: pane(6))))))
        let space = SpaceInfo(id: "s1", name: "s1", cwd: "/tmp", root: root, focus: "p1", status: .needsYou(waiting: 1),
                              paneStatuses: ["p1": .idle, "p2": .working, "p3": .needsYou(waiting: 1), "p4": .working, "p5": .stopped, "p6": .idle])
        let all = space.agentStatuses(shown: Set(StatusRank.allCases))
        #expect(all.marks == [.needsYou(waiting: 1), .stopped, .working, .working])
        #expect(all.more == 2)
        let noIdle = space.agentStatuses(shown: [.needsYou, .stopped, .working])
        #expect(noIdle.marks.count == 4 && noIdle.more == 0)
        #expect(space.agentStatuses(shown: []).marks.isEmpty)
    }
}
