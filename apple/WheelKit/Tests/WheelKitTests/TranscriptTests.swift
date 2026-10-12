import Testing
@testable import WheelKit

struct TranscriptTests {
    @Test func mergesTextChunksAndUpdatesToolsInPlace() {
        var t = Transcript()
        t.apply(.user(turn: 1, text: "clean up"))
        t.apply(.turnStart(turn: 1))
        t.apply(.text("Look"))
        t.apply(.text("ing now."))
        t.apply(.toolCall(id: "c1", title: "rm -rf build", kind: "execute", status: "pending"))
        t.apply(.text("Done."))
        t.apply(.toolUpdate(id: "c1", title: nil, status: "completed"))
        t.apply(.turnEnd(turn: 1, stopReason: "end_turn"))
        #expect(t.rows.map(\.kind) == [
            .user("clean up", queued: false),
            .agent("Looking now."),
            .tool(title: "rm -rf build", kind: "execute", status: "completed"),
            .agent("Done."),
        ])
    }

    @Test func aQueuedMessageStaysBelowTheRunningTurnUntilItStarts() {
        var t = Transcript()
        t.apply(.user(turn: 1, text: "first"))
        t.apply(.turnStart(turn: 1))
        t.apply(.text("Work"))
        t.apply(.user(turn: 2, text: "second"))
        t.apply(.text("ing."))
        t.apply(.toolCall(id: "c1", title: "ls", kind: "execute", status: "pending"))
        #expect(t.rows.map(\.kind) == [
            .user("first", queued: false),
            .agent("Working."),
            .tool(title: "ls", kind: "execute", status: "pending"),
            .user("second", queued: true),
        ])
        t.apply(.toolUpdate(id: "c1", title: nil, status: "completed"))
        t.apply(.turnEnd(turn: 1, stopReason: "end_turn"))
        t.apply(.turnStart(turn: 2))
        t.apply(.text("Next."))
        #expect(t.rows.map(\.kind) == [
            .user("first", queued: false),
            .agent("Working."),
            .tool(title: "ls", kind: "execute", status: "completed"),
            .user("second", queued: false),
            .agent("Next."),
        ])
    }

    @Test func aTurnStartForAnUnknownTurnChangesNothing() {
        var t = Transcript()
        t.apply(.user(turn: 2, text: "waiting"))
        t.apply(.turnStart(turn: 9))
        t.apply(.text("hi"))
        #expect(t.rows.map(\.kind) == [.agent("hi"), .user("waiting", queued: true)])
    }

    @Test func aTurnThatDidNotFinishLeavesANotice() {
        var t = Transcript()
        t.apply(.turnEnd(turn: 2, stopReason: "cancelled"))
        t.apply(.error(turn: nil, message: "the lead agent exited; restarting it"))
        #expect(t.rows.map(\.kind) == [.notice("Turn ended: cancelled"), .error("the lead agent exited; restarting it")])
    }

    @Test func aPlanUpdateReplacesThePlanWhereverItSits() {
        var t = Transcript()
        t.apply(.plan([PlanEntry(content: "read", status: "pending"), PlanEntry(content: "edit", status: "pending")]))
        t.apply(.toolCall(id: "c1", title: "Read a.rs", kind: "read", status: "completed"))
        t.apply(.text("Read it."))
        t.apply(.plan([PlanEntry(content: "read", status: "completed"), PlanEntry(content: "edit", status: "in_progress")]))
        #expect(t.rows.count == 3)
        #expect(t.rows[0].kind == .plan([PlanEntry(content: "read", status: "completed"), PlanEntry(content: "edit", status: "in_progress")]))
    }

    @Test func anUpdateForAnUnknownToolIsIgnored() {
        var t = Transcript()
        t.apply(.toolUpdate(id: "nope", title: "x", status: "completed"))
        #expect(t.rows.isEmpty)
    }

    @Test func resetReplacesEverything() {
        var t = Transcript()
        t.apply(.user(turn: 1, text: "old"))
        t.reset([.user(turn: 5, text: "new"), .turnStart(turn: 5), .text("hi")])
        #expect(t.rows.map(\.kind) == [.user("new", queued: false), .agent("hi")])
    }
}
