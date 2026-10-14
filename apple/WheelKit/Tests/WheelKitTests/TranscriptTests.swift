import Testing
@testable import WheelKit

struct TranscriptTests {
    @Test func mergesTextChunksAndUpdatesToolsInPlace() {
        var t = Transcript()
        t.apply(.user(turn: 1, text: "clean up"))
        t.apply(.turnStart(turn: 1))
        t.apply(.text("Look"))
        t.apply(.text("ing now."))
        t.apply(.toolCall(id: "c1", title: "rm -rf build", kind: "execute", status: "pending", content: []))
        t.apply(.text("Done."))
        t.apply(.toolUpdate(id: "c1", title: nil, status: "completed", content: nil))
        t.apply(.turnEnd(turn: 1, stopReason: "end_turn"))
        #expect(t.rows.map(\.kind) == [
            .user("clean up", queued: false),
            .agent("Looking now."),
            .tool(title: "rm -rf build", kind: "execute", status: "completed", content: []),
            .agent("Done."),
        ])
    }

    @Test func mergesThoughtChunksAndShowsABlockAsAPlaceholderRow() {
        var t = Transcript()
        t.apply(.thought("pondering "))
        t.apply(.thought("it"))
        t.apply(.block(kind: "resource_link", name: "notes.md"))
        t.apply(.text("Done."))
        t.apply(.thought("again"))
        #expect(t.rows.map(\.kind) == [
            .thought("pondering it"),
            .block(kind: "resource_link", name: "notes.md"),
            .agent("Done."),
            .thought("again"),
        ])
    }

    @Test func aToolRowKeepsItsCallIdSoARequestCanFindIt() {
        var t = Transcript()
        t.apply(.text("working"))
        t.apply(.toolCall(id: "c1", title: "rm -rf build", kind: "execute", status: "pending", content: []))
        t.apply(.toolUpdate(id: "c1", title: nil, status: "in_progress", content: nil))
        #expect(t.rows.map(\.toolCall) == [nil, "c1"])
    }

    @Test func anUpdateWithContentReplacesTheCallsContent() {
        var t = Transcript()
        let diff = { (new: String) in ToolContent.diff(path: "src/a.rs", oldText: "one\n", newText: new) }
        t.apply(.toolCall(id: "c2", title: "Edit src/a.rs", kind: "edit", status: "pending", content: [.text("editing a.rs"), diff("two\n")]))
        #expect(t.rows.map(\.kind) == [.tool(title: "Edit src/a.rs", kind: "edit", status: "pending", content: [.text("editing a.rs"), diff("two\n")])])
        // No content in the update keeps what the row has.
        t.apply(.toolUpdate(id: "c2", title: nil, status: "in_progress", content: nil))
        #expect(t.rows.map(\.kind) == [.tool(title: "Edit src/a.rs", kind: "edit", status: "in_progress", content: [.text("editing a.rs"), diff("two\n")])])
        t.apply(.toolUpdate(id: "c2", title: nil, status: "completed", content: [diff("three\n"), .terminal(id: "term-1")]))
        #expect(t.rows.map(\.kind) == [.tool(title: "Edit src/a.rs", kind: "edit", status: "completed", content: [diff("three\n"), .terminal(id: "term-1")])])
        // An empty list clears it.
        t.apply(.toolUpdate(id: "c2", title: nil, status: nil, content: []))
        #expect(t.rows.map(\.kind) == [.tool(title: "Edit src/a.rs", kind: "edit", status: "completed", content: [])])
    }

    @Test func aQueuedMessageStaysBelowTheRunningTurnUntilItStarts() {
        var t = Transcript()
        t.apply(.user(turn: 1, text: "first"))
        t.apply(.turnStart(turn: 1))
        t.apply(.text("Work"))
        t.apply(.user(turn: 2, text: "second"))
        t.apply(.text("ing."))
        t.apply(.toolCall(id: "c1", title: "ls", kind: "execute", status: "pending", content: []))
        #expect(t.rows.map(\.kind) == [
            .user("first", queued: false),
            .agent("Working."),
            .tool(title: "ls", kind: "execute", status: "pending", content: []),
            .user("second", queued: true),
        ])
        t.apply(.toolUpdate(id: "c1", title: nil, status: "completed", content: nil))
        t.apply(.turnEnd(turn: 1, stopReason: "end_turn"))
        t.apply(.turnStart(turn: 2))
        t.apply(.text("Next."))
        #expect(t.rows.map(\.kind) == [
            .user("first", queued: false),
            .agent("Working."),
            .tool(title: "ls", kind: "execute", status: "completed", content: []),
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
        t.apply(.toolCall(id: "c1", title: "Read a.rs", kind: "read", status: "completed", content: []))
        t.apply(.text("Read it."))
        t.apply(.plan([PlanEntry(content: "read", status: "completed"), PlanEntry(content: "edit", status: "in_progress")]))
        #expect(t.rows.count == 3)
        #expect(t.rows[0].kind == .plan([PlanEntry(content: "read", status: "completed"), PlanEntry(content: "edit", status: "in_progress")]))
    }

    @Test func anUpdateForAnUnknownToolIsIgnored() {
        var t = Transcript()
        t.apply(.toolUpdate(id: "nope", title: "x", status: "completed", content: nil))
        #expect(t.rows.isEmpty)
    }

    @Test func resetReplacesEverything() {
        var t = Transcript()
        t.apply(.user(turn: 1, text: "old"))
        t.reset([.user(turn: 5, text: "new"), .turnStart(turn: 5), .text("hi")])
        #expect(t.rows.map(\.kind) == [.user("new", queued: false), .agent("hi")])
    }

    @Test func aRowsRevisionCountsEachChangeToItsKindAndOnlyThatRow() {
        var t = Transcript()
        t.apply(.user(turn: 1, text: "go"))
        t.apply(.turnStart(turn: 1))
        t.apply(.text("a"))
        let user = t.rows[0].revision
        let before = t.rows[1].revision
        t.apply(.text("b"))
        #expect(t.rows[1].revision > before)
        #expect(t.rows[0].revision == user)
        #expect(t.rows[1].kind == .agent("ab"))
    }
}
