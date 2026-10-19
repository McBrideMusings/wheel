import Foundation
import Testing
@testable import WheelKit

struct SupervisorsTests {
    private static let pane = #"{"node":"pane","id":"p2","kind":"supervisor","supervisor":{"id":"E-u1","goal":"E","workers":3,"cap":60,"state":"held","session":"sup-1","queue":[{"bead":"t-3","model":"sonnet"},{"bead":"t-1","model":"haiku"}]}}"#

    private func supervisor(_ state: SupervisorRun = .running, cap: Int? = 90, queue: [QueueEntry] = []) -> Supervisor {
        Supervisor(id: "E-u1", goal: "E", workers: 2, cap: cap, state: state, session: "sup-1", queue: queue)
    }

    private func worker(_ bead: String, restarts: Int = 0) -> WorkerInfo {
        WorkerInfo(session: "worker-\(bead)", bead: bead, title: "Title \(bead)", model: "sonnet", restarts: restarts)
    }

    @Test func decodesASupervisorPaneAndItsSession() throws {
        let node = try JSONDecoder().decode(Node.self, from: Data(Self.pane.utf8))
        guard case let .pane(_, .supervisor(sup), _, _) = node else { Issue.record("not a supervisor: \(node)"); return }
        #expect((sup.id, sup.goal, sup.workers, sup.cap, sup.state) == ("E-u1", "E", 3, 60, .held))
        #expect(sup.queue?.map(\.bead) == ["t-3", "t-1"])
        let space = SpaceInfo(id: "s1", name: "tmp", cwd: "/tmp", root: node, focus: "p2")
        #expect(AgentPanes.sessions([space]) == ["p2": "sup-1"])
        // A supervisor missing its fields is an unsupported pane, not a lost layout.
        let broken = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p3","kind":"supervisor"}"#.utf8))
        guard case .pane(_, .unknown("supervisor", _), _, _) = broken else { Issue.record("not unknown: \(broken)"); return }
    }

    @Test func decodesTheSupervisorRepliesThatFlattenTheSupervisor() throws {
        let info = #"{"ev":"supervisor","req":5,"supervisor":{"space":"s1","pane":"p2","id":"E-u1","goal":"E","workers":2,"state":"running","session":"sup-1","agent":{"phase":"ready"},"running":[{"session":"worker-t-1","bead":"t-1","title":"Close it","model":"sonnet","worktree":"/w","branch":"t-1","continues":1,"restarts":2}],"inbox":2}}"#
        guard case let .supervisor(req, got) = try ServerMsg.decode(info) else { Issue.record("wrong case"); return }
        #expect(req == 5)
        #expect((got.pane, got.supervisor.id, got.inbox) == ("p2", "E-u1", 2))
        #expect(got.running == [WorkerInfo(session: "worker-t-1", bead: "t-1", title: "Close it", model: "sonnet", worktree: "/w", branch: "t-1", continues: 1, restarts: 2)])
        let events = #"{"ev":"supervisor_events","req":6,"events":[{"id":1,"at":1000,"supervisor":"E-u1","worker":"worker-t-1","bead":"t-1","kind":"sent_back","detail":"x"}]}"#
        guard case let .supervisorEvents(_, list) = try ServerMsg.decode(events) else { Issue.record("wrong case"); return }
        #expect(list.first?.kind == .sentBack)
        let inbox = #"{"ev":"supervisor_inbox","req":7,"items":[{"id":"s1","at":2,"supervisor":"E-u1","worker":"worker-t-2","bead":"t-2","kind":"blocked","detail":"d"}]}"#
        guard case let .supervisorInbox(_, items) = try ServerMsg.decode(inbox) else { Issue.record("wrong case"); return }
        #expect(items.first?.bead == "t-2")
    }

    @Test func encodesTheSupervisorRequests() throws {
        #expect(try ClientMsg.supervisorShow(req: 1, id: "E-u1").json() == #"{"id":"E-u1","op":"supervisor_show","req":1}"#)
        #expect(try ClientMsg.supervisorWorkers(req: 2, id: "E-u1", workers: 4).json() == #"{"id":"E-u1","op":"supervisor_workers","req":2,"workers":4}"#)
        #expect(try ClientMsg.supervisorCap(req: 3, id: "E-u1", cap: nil).json() == #"{"id":"E-u1","op":"supervisor_cap","req":3}"#)
        #expect(try ClientMsg.supervisorCap(req: 3, id: "E-u1", cap: 70).json() == #"{"cap":70,"id":"E-u1","op":"supervisor_cap","req":3}"#)
    }

    @Test func countsWhatLandedAgainstEverythingInScope() {
        let info = SupervisorInfo(space: "s1", pane: "p2", supervisor: supervisor(queue: [QueueEntry(bead: "t-3", model: "sonnet"), QueueEntry(bead: "t-2", model: "sonnet")]),
                                  running: [worker("t-2")])
        let landed = LoopEvent(id: 1, at: 1, supervisor: "E-u1", bead: "t-1", kind: .landed)
        let board = SupervisorBoard(info: info, events: [landed, landed])
        #expect(board.landed.map(\.bead) == ["t-1"])
        // t-2 is running, so it is out of the queue; t-3 is the only one still queued.
        #expect(board.queued.map(\.bead) == ["t-3"])
        #expect(board.progressText == "1 of 3 landed")
    }

    @Test func aLandingStaysCountedAfterTheDaemonDropsItsEvent() {
        let info = SupervisorInfo(space: "s1", pane: "p2", supervisor: supervisor(queue: [QueueEntry(bead: "t-2", model: "sonnet")]))
        let board = SupervisorBoard(info: info, events: [], landedBefore: [LandedBead(bead: "t-1", detail: "closed")])
        #expect(board.landed.map(\.bead) == ["t-1"])
        #expect(board.progressText == "1 of 2 landed")
        // A bead waiting in the inbox is still in scope.
        let waiting = SupervisorInboxItem(id: "s1", at: 0, supervisor: "E-u1", worker: "worker-t-3", bead: "t-3", kind: .blocked, detail: "")
        #expect(SupervisorBoard(info: info, events: [], landedBefore: [LandedBead(bead: "t-1", detail: "closed")], inbox: [waiting]).progressText == "1 of 3 landed")
    }

    @Test func wordsEachWorkersStateWithAGlyph() {
        let events = [
            LoopEvent(id: 1, at: 0, supervisor: "E-u1", bead: "t-2", kind: .sentBack),
            LoopEvent(id: 2, at: 0, supervisor: "E-u1", bead: "t-3", kind: .stalled),
            LoopEvent(id: 3, at: 0, supervisor: "E-u1", bead: "t-3", kind: .restarted),
        ]
        let running = [worker("t-1"), worker("t-2"), worker("t-3", restarts: 2)]
        let now = Date(timeIntervalSince1970: 6 * 60)
        func words(_ state: SupervisorRun) -> [String] {
            SupervisorBoard(info: SupervisorInfo(space: "s1", pane: "p2", supervisor: supervisor(state), running: running), events: events)
                .lines(now: now).map(\.word)
        }
        #expect(words(.running) == ["Working", "Landing · sent back 1 of 2", "Stalled 6m · restarted 2 of 3"])
        #expect(Set(words(.paused)) == ["Paused after its turn"])
        #expect(Set(words(.held)) == ["Finishing, then held"])
    }

    @Test func holdsAtTheCapAndSaysSo() {
        let held = SupervisorBoard(info: SupervisorInfo(space: "s1", pane: "p2", supervisor: supervisor(.held)), events: [])
        #expect(held.holdText == "Holding at 90% of the week: running workers finish their bead, nothing new starts.")
        #expect(held.notRefilling)
        let running = SupervisorBoard(info: SupervisorInfo(space: "s1", pane: "p2", supervisor: supervisor()), events: [])
        #expect(running.holdText == nil)
    }

    @Test func marksASpaceTabWithRunningPausedHeldAndWhatWaits() throws {
        func space(_ state: String) throws -> SpaceInfo {
            let text = #"{"node":"pane","id":"p2","kind":"supervisor","supervisor":{"id":"E-u1","goal":"E","workers":2,"cap":90,"state":"\#(state)","session":"sup-1"}}"#
            return SpaceInfo(id: "s1", name: "tmp", cwd: "/tmp", root: try JSONDecoder().decode(Node.self, from: Data(text.utf8)), focus: "p2")
        }
        func info(_ state: SupervisorRun) -> [String: SupervisorInfo] {
            ["p2": SupervisorInfo(space: "s1", pane: "p2", supervisor: supervisor(state), running: [worker("t-1"), worker("t-2")], inbox: 2)]
        }
        let running = SpaceMarks.of(try space("running"), infos: info(.running))
        #expect((running.stateText, running.forYouText) == ("▶ 2", "2 for you"))
        #expect(SpaceMarks.of(try space("paused"), infos: info(.paused)).stateText == "‖")
        #expect(SpaceMarks.of(try space("held"), infos: info(.held)).stateText == "‖ 90%")
        #expect(SpaceMarks.of(try space("stopped"), infos: [:]).isEmpty)
    }
}
