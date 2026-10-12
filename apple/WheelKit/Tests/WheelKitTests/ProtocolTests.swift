import Foundation
import Testing
@testable import WheelKit

/// The shapes below are copied from what wheeld sends (proto/src/lib.rs, serde snake_case).
struct ProtocolTests {
    @Test func encodesClientMessagesWithOpTags() throws {
        #expect(try ClientMsg.termOpen(req: 3, cwd: nil, cols: 80, rows: 24).json() == #"{"cols":80,"op":"term_open","req":3,"rows":24}"#)
        #expect(try ClientMsg.termInput(req: nil, id: "t1", data: Data("ls\r".utf8)).json() == #"{"data":"bHMN","id":"t1","op":"term_input"}"#)
        #expect(try ClientMsg.permAnswer(req: 9, id: "p1", choice: "allow").json() == #"{"choice":"allow","id":"p1","op":"perm_answer","req":9}"#)
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
        guard case let .status(req, _, _, terms, lead, perms) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(req == 1)
        #expect(terms.first?.id == "t1")
        #expect(lead == .busy(queued: 2))
        #expect(perms.first?.title == "rm -rf build")
    }

    @Test func decodesLeadEvents() throws {
        guard case let .leadAttached(_, state, events) = try ServerMsg.decode(
            #"{"ev":"lead_attached","state":{"phase":"ready"},"events":[{"kind":"user","turn":1,"text":"hi"},{"kind":"text","text":"hello"},{"kind":"turn_end","turn":1,"stop_reason":"end_turn"},{"kind":"error","message":"agent exited"}]}"#
        ) else { Issue.record("wrong case"); return }
        #expect(state == .ready)
        #expect(events == [.user(turn: 1, text: "hi"), .text("hello"), .turnEnd(turn: 1, stopReason: "end_turn"), .error(turn: nil, message: "agent exited")])
    }

    @Test func unknownEventsDoNotFail() throws {
        guard case let .unknown(ev) = try ServerMsg.decode(#"{"ev":"something_new","x":1}"#) else { Issue.record("wrong case"); return }
        #expect(ev == "something_new")
    }
}
