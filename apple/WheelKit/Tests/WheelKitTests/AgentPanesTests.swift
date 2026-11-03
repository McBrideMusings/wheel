import Foundation
import Testing
@testable import WheelKit

struct AgentPanesTests {
    @Test func decodesAnAgentPaneAndListsItsAgent() throws {
        let text = #"{"ev":"spaces","spaces":[{"id":"s1","name":"tmp","cwd":"/tmp","focus":"p2","root":{"node":"split","id":"x3","axis":"horizontal","ratio":0.5,"a":{"node":"pane","id":"p2","kind":"agent","agent":"w7","harness":"claude"},"b":{"node":"pane","id":"p4","kind":"terminal","term":"t1"}}}]}"#
        guard case let .spaces(spaces, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(spaces.first?.root.panes.first?.kind == .agent(agent: "w7", harness: "claude"))
        #expect(AgentPanes.agents(spaces) == ["p2": "w7"])
        // An agent pane missing its agent is unsupported, not a reason to drop the frame.
        let broken = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p5","kind":"agent"}"#.utf8))
        #expect(broken == .pane(id: "p5", kind: .unknown(kind: "agent", raw: ["kind": .string("agent")])))
        // The agent's saved conversation id is the daemon's to keep; a client shows the pane as before.
        let saved = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p6","kind":"agent","agent":"w8","harness":"claude","conversation":"abc"}"#.utf8))
        #expect(saved == .pane(id: "p6", kind: .agent(agent: "w8", harness: "claude")))
    }

    /// A waiting request counts against the space whose agent pane runs the agent that asked.
    @Test func countsWaitingRequestsBySpace() throws {
        let text = #"{"ev":"spaces","spaces":[{"id":"s1","name":"a","cwd":"/a","focus":"p2","root":{"node":"pane","id":"p2","kind":"agent","agent":"w7","harness":"claude"}},{"id":"s2","name":"b","cwd":"/b","focus":"p4","root":{"node":"pane","id":"p4","kind":"agent","agent":"w9","harness":"claude"}}]}"#
        guard case let .spaces(spaces, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        let ask = { (id: String, agent: String) in
            try JSONDecoder().decode(PermInfo.self, from: Data(#"{"id":"\#(id)","agent":"\#(agent)","title":"rm","options":[]}"#.utf8))
        }
        let perms = [try ask("q1", "w9"), try ask("q2", "w9"), try ask("q3", "w404")]
        #expect(AgentPanes.waiting(perms, in: spaces) == ["s2": 2])
        #expect(AgentPanes.waiting([], in: spaces).isEmpty)
    }

    /// A web pane's refresh command asks with the pane id as its agent.
    @Test func countsAWebPanesRefreshRequestAgainstItsSpace() throws {
        let text = #"{"ev":"spaces","spaces":[{"id":"s1","name":"a","cwd":"/a","focus":"p2","root":{"node":"pane","id":"p2","kind":"web","scope":"project","name":"dash"}}]}"#
        guard case let .spaces(spaces, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        let perm = try JSONDecoder().decode(PermInfo.self, from: Data(#"{"id":"q1","agent":"p2","title":"Refresh","options":[]}"#.utf8))
        #expect(AgentPanes.waiting([perm], in: spaces) == ["s1": 1])
    }

    @Test func encodesStartingAnAgent() throws {
        #expect(try ClientMsg.paneStart(req: 4, pane: "p2", kind: .agent, message: "go").json()
            == #"{"kind":"agent","message":"go","op":"pane_start","pane":"p2","req":4}"#)
        #expect(try ClientMsg.paneStart(req: 5, pane: "p2", kind: .agent, agent: "codex").json()
            == #"{"agent":"codex","kind":"agent","op":"pane_start","pane":"p2","req":5}"#)
    }

    @Test func decodesAStartedAgent() throws {
        let msg = try JSONDecoder().decode(ServerMsg.self, from: Data(#"{"ev":"started","req":4,"pane":"p2","agent":"w5","state":{"phase":"ready"}}"#.utf8))
        guard case let .started(req, pane, agent) = msg else { Issue.record("not started: \(msg)"); return }
        #expect((req, pane, agent) == (4, "p2", "w5"))
    }
}

struct AgentCatalogTests {
    @Test func pickerOffersOnlyFoundAgents() {
        let claude = AgentInfo(name: "claude", command: "claude", path: "/bin/claude", found: true)
        let codex = AgentInfo(name: "codex", command: "codex", found: false, why: "not on PATH")
        #expect(AgentInfo.pickable([claude, codex]).map(\.name) == ["claude"])
        #expect(AgentInfo.pickable([claude, AgentInfo(name: "codex", command: "codex", path: "/x", found: true)]).map(\.name) == ["claude", "codex"])
        #expect(AgentInfo.pickable([codex]).isEmpty)
    }
}
