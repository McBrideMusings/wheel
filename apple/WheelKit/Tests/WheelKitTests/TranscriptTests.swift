import Foundation
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

    @Test func mergesThoughtChunks() {
        var t = Transcript()
        t.apply(.thought("pondering "))
        t.apply(.thought("it"))
        t.apply(.text("Done."))
        t.apply(.thought("again"))
        #expect(t.rows.map(\.kind) == [
            .thought("pondering it"),
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
        t.apply(.toolUpdate(id: "c2", title: nil, status: "completed", content: [diff("three\n")]))
        #expect(t.rows.map(\.kind) == [.tool(title: "Edit src/a.rs", kind: "edit", status: "completed", content: [diff("three\n")])])
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
        t.apply(.error(turn: nil, message: "the agent exited; restarting it"))
        #expect(t.rows.map(\.kind) == [.notice("Turn ended: Cancelled"),.error("the agent exited; restarting it")])
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
        t.reset([SessionEvent.user(turn: 5, text: "new"), .turnStart(turn: 5), .text("hi")].map { StampedEvent($0) }, daemonNow: Date())
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

    /// The daemon's clock runs 30 s behind this client's: a tool that has run 5 s still reads 5 s.
    @Test func aRunningTimerFollowsTheDaemonsClock() {
        let daemonNow = Date(timeIntervalSince1970: 1_000)
        let received = daemonNow + 30
        var t = Transcript()
        t.reset([StampedEvent(.toolCall(id: "a", title: "ls", kind: "execute", status: "in_progress", content: []), at: daemonNow - 5)],
                daemonNow: daemonNow, received: received)
        #expect(t.rows[0].elapsed(now: received) == 5)
        t.apply(StampedEvent(.toolUpdate(id: "a", title: nil, status: "completed", content: nil), at: daemonNow + 2))
        #expect(t.rows[0].elapsed(now: received + 99) == 7)
    }

    /// A tool runs from its call to its final status; a thought from its first chunk to the next
    /// event of its turn; a turn that ends stops whatever was still running.
    @Test func toolsAndThoughtsCarryHowLongTheyRan() {
        let t0 = Date(timeIntervalSince1970: 1_000)
        var t = Transcript()
        t.apply(.user(turn: 1, text: "go"), at: t0)
        t.apply(.turnStart(turn: 1), at: t0)
        t.apply(.thought("hmm"), at: t0 + 1)
        t.apply(.thought(" more"), at: t0 + 2)
        t.apply(.toolCall(id: "a", title: "ls", kind: "execute", status: "pending", content: []), at: t0 + 4)
        let thought = t.rows[1], tool = t.rows[2]
        #expect(thought.kind == .thought("hmm more") && !thought.open && thought.elapsed(now: t0 + 99) == 3)
        #expect(tool.open && tool.elapsed(now: t0 + 6.5) == 2.5)
        t.apply(.toolUpdate(id: "a", title: nil, status: "in_progress", content: nil), at: t0 + 5)
        #expect(t.rows[2].open)
        t.apply(.toolUpdate(id: "a", title: nil, status: "completed", content: nil), at: t0 + 7)
        #expect(!t.rows[2].open && t.rows[2].elapsed(now: t0 + 99) == 3)
        t.apply(.toolCall(id: "b", title: "sleep", kind: "execute", status: "pending", content: []), at: t0 + 8)
        t.apply(.turnEnd(turn: 1, stopReason: "cancelled"), at: t0 + 9)
        #expect(!t.rows[3].open && t.rows[3].elapsed(now: t0 + 99) == 1)
        // Replayed history has no times: it still closes, and says nothing about duration.
        var replayed = Transcript()
        replayed.reset([SessionEvent.toolCall(id: "c", title: "ls", kind: "execute", status: "pending", content: []),
                        .turnEnd(turn: 1, stopReason: "end_turn")].map { StampedEvent($0) }, daemonNow: Date())
        #expect(!replayed.rows[0].open && replayed.rows[0].elapsed(now: t0) == nil)
    }

    /// A call that reuses an id (replayed history) must not leave the earlier row running.
    @Test func aReusedToolCallIdClosesTheEarlierRow() {
        var t = Transcript()
        t.apply(.toolCall(id: "c1", title: "ls", kind: "execute", status: "pending", content: []))
        t.apply(.toolCall(id: "c1", title: "ls", kind: "execute", status: "pending", content: []))
        #expect(t.rows.count == 2)
        #expect(t.rows.filter(\.open).count == 1 && !t.rows[0].open && t.rows[1].open)
        t.apply(.toolUpdate(id: "c1", title: nil, status: "completed", content: nil))
        #expect(t.rows.filter(\.open).isEmpty)
    }

    private func replay(_ events: [SessionEvent]) -> Transcript {
        var t = Transcript()
        for event in events { t.apply(event) }
        return t
    }

    private static let sevenCalls: [SessionEvent] = [
        .user(turn: 1, text: "go"), .turnStart(turn: 1), .thought("hm"),
        .toolCall(id: "1", title: "ls", kind: "execute", status: "completed", content: []),
        .toolCall(id: "2", title: "a", kind: "read", status: "completed", content: []),
        .toolCall(id: "3", title: "b", kind: "read", status: "completed", content: []),
        .toolCall(id: "4", title: "e", kind: "edit", status: "completed", content: [
            .diff(path: "x.txt", oldText: "one\ntwo\n", newText: "one\nTWO\nthree\n")]),
        .toolCall(id: "5", title: "f", kind: "edit", status: "completed", content: [
            .diff(path: "y.txt", oldText: nil, newText: "new\n")]),
        .toolCall(id: "6", title: "g", kind: "search", status: "completed", content: []),
        .toolCall(id: "7", title: "h", kind: "fetch", status: "completed", content: []),
        .text("done"), .turnEnd(turn: 1, stopReason: "end_turn"),
    ]

    /// With the defaults a turn is one work line holding every call and the thought, then the reply
    /// and the card of the files it edited.
    @Test func aTurnIsOneWorkLineThenTheReplyThenTheEditsCard() {
        let t = replay(Self.sevenCalls)
        #expect(Transcript.items(t.rows) == [.row(0), .work(Array(1...8)), .row(9), .edits(1..<10)])
        let files = Edits.files(in: Array(t.rows))
        #expect(files.map(\.path) == ["x.txt", "y.txt"])
        #expect(files.map(\.added) == [2, 1] && files.map(\.removed) == [1, 0])
        #expect(Edits.title(count: files.count) == "Edited 2 files")
    }

    @Test func runsAndEachAndNoCardAndHiddenThoughts() {
        let t = replay(Self.sevenCalls)
        var layout = TranscriptLayout()
        layout.tools = .runs
        #expect(Transcript.items(t.rows, layout: layout) == [.row(0), .row(1), .tools(Array(2...8)), .row(9), .edits(1..<10)])
        layout.tools = .each
        layout.editsCard = false
        #expect(Transcript.items(t.rows, layout: layout) == (0...9).map { .row($0) })
        layout.thinking = .hidden
        #expect(Transcript.items(t.rows, layout: layout) == [0, 2, 3, 4, 5, 6, 7, 8, 9].map { .row($0) })
        layout.tools = .turn
        #expect(Transcript.items(t.rows, layout: layout) == [.row(0), .work(Array(2...8)), .row(9)])
        // A running turn has no card yet.
        #expect(!Transcript.items(Array(t.rows[0..<8]), busy: true).contains { if case .edits = $0 { true } else { false } })
    }

    /// Every message the agent writes is drawn in the main chat; only the work between messages
    /// folds, one section per stretch, including the stretch before the first and after the last.
    @Test func eachMessageStandsOutsideTheWorkSectionsAroundIt() {
        let t = replay([
            .user(turn: 1, text: "go"), .turnStart(turn: 1),
            .toolCall(id: "1", title: "ls", kind: "execute", status: "completed", content: []),
            .text("Found a 33 GB rip. Now checking."),
            .thought("hm"),
            .toolCall(id: "2", title: "a", kind: "read", status: "completed", content: []),
            .text("The full answer."),
            .toolCall(id: "3", title: "hook fix", kind: "execute", status: "completed", content: []),
            .text("Still stands."), .turnEnd(turn: 1, stopReason: "end_turn"),
        ])
        #expect(Transcript.items(t.rows) == [.row(0), .work([1]), .row(2), .work([3, 4]), .row(5), .work([6]), .row(7)])
        var layout = TranscriptLayout()
        layout.thinking = .hidden
        #expect(Transcript.items(t.rows, layout: layout) == [.row(0), .work([1]), .row(2), .work([4]), .row(5), .work([6]), .row(7)])
    }

    /// A plan the agent shows the person is drawn in the main chat between two sections, and
    /// thoughts that are hidden make no empty section.
    @Test func aPlanSplitsSectionsAndHiddenThoughtsMakeNone() {
        let t = replay([
            .user(turn: 1, text: "go"), .turnStart(turn: 1),
            .toolCall(id: "1", title: "ls", kind: "execute", status: "completed", content: []),
            .plan([PlanEntry(content: "step", status: "pending")]),
            .toolCall(id: "2", title: "a", kind: "read", status: "completed", content: []),
            .text("hi"), .thought("hm"), .turnEnd(turn: 1, stopReason: "end_turn"),
        ])
        #expect(Transcript.items(t.rows) == [.row(0), .work([1]), .row(2), .work([3]), .row(4), .work([5])])
        var layout = TranscriptLayout()
        layout.thinking = .hidden
        #expect(Transcript.items(t.rows, layout: layout) == [.row(0), .work([1]), .row(2), .work([3]), .row(4)])
    }

    @Test func aRunReadsAsWordsFromItsKinds() {
        let t = replay(Self.sevenCalls)
        #expect(ToolWords.summary(of: Array(t.rows[2...3])) == "Ran 1 command, read 1 file")
        #expect(ToolWords.summary(of: Array(t.rows[2...6])) == "Ran 1 command, read 2 files, edited 2 files")
    }
}
