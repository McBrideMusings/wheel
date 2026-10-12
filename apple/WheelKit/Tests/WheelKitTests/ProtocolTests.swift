import Foundation
import Testing
@testable import WheelKit

/// The shapes below are copied from what wheeld sends (proto/src/lib.rs, serde snake_case).
struct ProtocolTests {
    @Test func encodesClientMessagesWithOpTags() throws {
        #expect(try ClientMsg.termOpen(req: 3, cwd: nil, cols: 80, rows: 24).json() == #"{"cols":80,"op":"term_open","req":3,"rows":24}"#)
        #expect(try ClientMsg.termInput(req: nil, id: "t1", data: Data("ls\r".utf8)).json() == #"{"data":"bHMN","id":"t1","op":"term_input"}"#)
        #expect(try ClientMsg.permAnswer(req: 9, id: "p1", choice: "allow").json() == #"{"choice":"allow","id":"p1","op":"perm_answer","req":9}"#)
        #expect(try ClientMsg.termClose(req: 4, id: "t1").json() == #"{"id":"t1","op":"term_close","req":4}"#)
    }

    @Test func decodesTerminalOutput() throws {
        guard case let .termOutput(id, data) = try ServerMsg.decode(#"{"ev":"term_output","id":"t1","data":"bWFjLTQy"}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(id == "t1")
        #expect(String(decoding: data, as: UTF8.self) == "mac-42")
    }

    @Test func decodesStatusWithLeadAndPerms() throws {
        let text = #"{"ev":"status","req":1,"version":"0.1.0","uptime_secs":3,"log_path":"/l","terms":[{"id":"t1","pid":5,"cwd":"/tmp","cols":80,"rows":24,"alive":true,"buffered_bytes":10}],"lead":{"phase":"busy","queued":2},"perms":[{"id":"p1","session":"lead","title":"rm -rf build","options":[{"id":"a","name":"Allow","kind":"allow_once"}]}]}"#
        guard case let .status(req, _, _, terms, lead, perms, spaces, active) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(req == 1)
        #expect(terms.first?.id == "t1")
        #expect(lead == .busy(queued: 2))
        #expect(perms.first?.title == "rm -rf build")
        #expect(spaces.isEmpty)
        #expect(active == nil)
    }

    @Test func encodesSpaceOps() throws {
        #expect(try ClientMsg.spaceCreate(req: 1, cwd: "/tmp").json() == #"{"cwd":"\/tmp","op":"space_create","req":1}"#)
        #expect(try ClientMsg.focus(req: nil, space: nil, pane: "p2").json() == #"{"op":"focus","pane":"p2"}"#)
        #expect(try ClientMsg.paneSplit(req: 2, pane: "p2", dir: .right).json() == #"{"dir":"right","op":"pane_split","pane":"p2","req":2}"#)
        #expect(try ClientMsg.paneSetKind(req: nil, pane: "p2", kind: .terminal).json() == #"{"kind":"terminal","op":"pane_set_kind","pane":"p2"}"#)
        #expect(try ClientMsg.splitRatio(req: nil, split: "x3", ratio: 0.25).json() == #"{"op":"split_ratio","ratio":0.25,"split":"x3"}"#)
    }

    /// A pane kind this build lacks (wheel-0py) decodes as unknown with every field kept.
    @Test func decodesSpacesWithASplitAndAnUnknownPane() throws {
        let text = #"{"ev":"spaces","active":"s1","spaces":[{"id":"s1","name":"tmp","cwd":"/tmp","focus":"p2","root":{"node":"split","id":"x3","axis":"horizontal","ratio":0.3,"a":{"node":"pane","id":"p2","kind":"terminal","term":"t1"},"b":{"node":"pane","id":"p9","kind":"hologram","depth":3}}}]}"#
        guard case let .spaces(spaces, active) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(active == "s1")
        #expect(spaces.first?.focus == "p2")
        #expect(spaces.first?.root == .split(id: "x3", axis: .horizontal, ratio: 0.3,
                                            a: .pane(id: "p2", kind: .terminal(term: "t1")),
                                            b: .pane(id: "p9", kind: .unknown(kind: "hologram", raw: ["kind": .string("hologram"), "depth": .number(3)]))))
        // A known kind missing its fields is unsupported too, not a reason to drop the frame.
        let broken = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p4","kind":"terminal"}"#.utf8))
        #expect(broken == .pane(id: "p4", kind: .unknown(kind: "terminal", raw: ["kind": .string("terminal")])))
    }

    @Test func decodesLeadEvents() throws {
        guard case let .leadAttached(_, state, events) = try ServerMsg.decode(
            #"{"ev":"lead_attached","state":{"phase":"ready"},"events":[{"kind":"user","turn":1,"text":"hi"},{"kind":"text","text":"hello"},{"kind":"turn_end","turn":1,"stop_reason":"end_turn"},{"kind":"error","message":"agent exited"}]}"#
        ) else { Issue.record("wrong case"); return }
        #expect(state == .ready)
        #expect(events == [.user(turn: 1, text: "hi"), .text("hello"), .turnEnd(turn: 1, stopReason: "end_turn"), .error(turn: nil, message: "agent exited")])
    }

    @Test func decodesErrorsWithAndWithoutATerminal() throws {
        guard case let .error(req, term, message) = try ServerMsg.decode(#"{"ev":"error","term":"t-missing","message":"no terminal t-missing"}"#) else { Issue.record("wrong case"); return }
        #expect(req == nil && term == "t-missing" && message == "no terminal t-missing")
        guard case let .error(req, term, _) = try ServerMsg.decode(#"{"ev":"error","req":4,"message":"no space s9"}"#) else { Issue.record("wrong case"); return }
        #expect(req == 4 && term == nil)
    }

    @Test func unknownEventsDoNotFail() throws {
        guard case let .unknown(ev) = try ServerMsg.decode(#"{"ev":"something_new","x":1}"#) else { Issue.record("wrong case"); return }
        #expect(ev == "something_new")
    }
}
