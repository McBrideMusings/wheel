import Foundation
import Testing
@testable import WheelKit

struct AgentPanesTests {
    @Test func decodesAnAgentPaneAndListsItsSession() throws {
        let text = #"{"ev":"spaces","active":"s1","spaces":[{"id":"s1","name":"tmp","cwd":"/tmp","focus":"p2","root":{"node":"split","id":"x3","axis":"horizontal","ratio":0.5,"a":{"node":"pane","id":"p2","kind":"agent","session":"w7"},"b":{"node":"pane","id":"p4","kind":"terminal","term":"t1"}}}]}"#
        guard case let .spaces(spaces, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(spaces.first?.root.panes.first?.kind == .agent(session: "w7"))
        #expect(AgentPanes.sessions(spaces) == ["p2": "w7"])
        // An agent pane missing its session is unsupported, not a reason to drop the frame.
        let broken = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p5","kind":"agent"}"#.utf8))
        #expect(broken == .pane(id: "p5", kind: .unknown(kind: "agent", raw: ["kind": .string("agent")])))
    }

    @Test func encodesAnAgentKind() throws {
        #expect(try ClientMsg.paneSetKind(req: 4, pane: "p2", kind: .agent).json()
            == #"{"kind":"agent","op":"pane_set_kind","pane":"p2","req":4}"#)
    }
}
