import Foundation
import Testing
@testable import WheelKit

struct WorkerPanesTests {
    private func board(workers: [WorkerInfo]) -> SupervisorBoard {
        let supervisor = Supervisor(id: "E-u1", goal: "E", workers: 2, cap: nil, state: .running, session: "sup-1")
        return SupervisorBoard(info: SupervisorInfo(space: "s1", pane: "p2", supervisor: supervisor, running: workers), events: [])
    }

    @Test func decodesAWorkerPaneAndTheSessionItShows() throws {
        let node = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p5","kind":"worker","session":"worker-t-2"}"#.utf8))
        guard case let .pane(_, .worker(session), _) = node else { Issue.record("not a worker pane: \(node)"); return }
        #expect(session == "worker-t-2")
        let space = SpaceInfo(id: "s1", name: "tmp", cwd: "/tmp", root: node, focus: "p5")
        #expect(AgentPanes.sessions([space]) == ["p5": "worker-t-2"])
    }

    @Test func aWorkerRowReadsInAPaneOnceThePaneExists() throws {
        let info = #"{"session":"worker-t-2","bead":"t-2","title":"T","model":"sonnet","worktree":"/w","branch":"t-2","continues":0,"restarts":0,"pane":"p5"}"#
        let pushed = try JSONDecoder().decode(WorkerInfo.self, from: Data(info.utf8))
        let open = WorkerInfo(session: "worker-t-1", bead: "t-1", title: "T", model: "sonnet")
        let lines = board(workers: [open, pushed]).lines(now: Date())
        #expect(lines.map(\.inPane) == [false, true])
        #expect(lines.map(\.laneText) == ["Worker L1", "Worker L2"])
        #expect(board(workers: [open, pushed]).line(session: "worker-t-2", now: Date())?.worker.pane == "p5")
        #expect(board(workers: [open]).line(session: "worker-t-2", now: Date()) == nil)
    }

    @Test func encodesThePushRequest() throws {
        #expect(try ClientMsg.workerPush(req: 4, id: "E-u1", session: "worker-t-2").json() == #"{"id":"E-u1","op":"worker_push","req":4,"session":"worker-t-2"}"#)
    }
}
