import Foundation
import Testing
@testable import WheelKit

struct OrchestratorSpaceTests {
    @Test func decodesTheOrchestratorSpaceAndItsGlobalOrchestrators() throws {
        let frame = #"""
        {"ev":"spaces","spaces":[
          {"id":"s1","name":"wheel","cwd":"/w","root":{"node":"pane","id":"p1","kind":"agent","agent":"w1","harness":"claude","orchestrator":"space"}},
          {"id":"so","name":"Orchestrator","cwd":"/o","orchestrator":true,"root":{"node":"split","id":"x","axis":"horizontal","ratio":0.5,
            "a":{"node":"pane","id":"pe","kind":"empty"},
            "b":{"node":"pane","id":"pg","kind":"agent","agent":"w2","harness":"claude","orchestrator":"global"}}}
        ]}
        """#
        guard case let .spaces(spaces, _) = try ServerMsg.decode(frame) else { Issue.record("wrong case"); return }
        #expect(spaces.map(\.orchestrator) == [false, true])
        #expect(spaces[0].firstGlobalOrchestrator == nil, "a space orchestrator is not a global one")
        #expect(spaces[1].firstGlobalOrchestrator == "pg", "the empty pane before it is skipped")
        #expect(spaces[1].orchestratorScopes == ["pg": "global"])
    }

    @Test func theOrchestratorSessionIsLeftOutSoNoViewFallsBackToIt() {
        let spaces = [SpaceInfo(id: "s1", name: "wheel", cwd: "/w", root: .pane(id: "p1", kind: .empty), focus: "p1"),
                      SpaceInfo(id: "so", name: "Orchestrator", cwd: "/o", root: .pane(id: "p2", kind: .empty), focus: "p2", orchestrator: true)]
        let sessions = [SessionInfo(id: "n1", name: "Main", spaces: ["s1"]), SessionInfo(id: "no", name: "Orchestrator", spaces: ["so"])]
        let kept = sessions.leavingOut(orchestratorOf: spaces)
        #expect(kept.map(\.id) == ["n1"])
        // Main closes: the view has no session to fall back to, never the Orchestrator one.
        var view = SessionView()
        view.show(space: "s1", in: kept)
        view.layoutChanged(from: kept, to: [SessionInfo(id: "no", name: "Orchestrator", spaces: ["so"])].leavingOut(orchestratorOf: spaces))
        #expect(view.space != "so" && view.session != "no")
    }

    @Test func encodesTheGlobalStartAndDecodesItsReply() throws {
        #expect(try ClientMsg.orchestratorStartGlobal(req: 7).json() == #"{"if_none":true,"op":"orchestrator_start","req":7,"scope":"global"}"#)
        guard case let .orchestratorStarted(req, pane, agent) = try ServerMsg.decode(#"{"ev":"orchestrator_started","req":7,"pane":"p9","agent":"w9"}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(req == 7 && pane == "p9" && agent == "w9")
    }
}
