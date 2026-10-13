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
        #expect(try ClientMsg.sessionAttach(req: 5, session: "lead").json() == #"{"op":"session_attach","req":5,"session":"lead"}"#)
        #expect(try ClientMsg.sessionSend(req: nil, session: "w5", text: "hi").json() == #"{"op":"session_send","session":"w5","text":"hi"}"#)
        #expect(try ClientMsg.sessionCancel(req: 6, session: "w5").json() == #"{"op":"session_cancel","req":6,"session":"w5"}"#)
        #expect(try ClientMsg.sessionDetach(session: "w5").json() == #"{"op":"session_detach","session":"w5"}"#)
    }

    @Test func decodesTerminalOutput() throws {
        guard case let .termOutput(id, data) = try ServerMsg.decode(#"{"ev":"term_output","id":"t1","data":"bWFjLTQy"}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(id == "t1")
        #expect(String(decoding: data, as: UTF8.self) == "mac-42")
    }

    @Test func decodesStatusWithLeadAndPerms() throws {
        let text = #"{"ev":"status","req":1,"version":"0.1.0","uptime_secs":3,"log_path":"/l","terms":[{"id":"t1","pid":5,"cwd":"/tmp","cols":80,"rows":24,"alive":true}],"lead":{"phase":"busy","queued":2},"perms":[{"id":"p1","session":"lead","title":"rm -rf build","options":[{"id":"a","name":"Allow","kind":"allow_once"}]}]}"#
        guard case let .status(req, _, _, _, terms, lead, perms, spaces, active) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(req == 1)
        #expect(terms.first?.id == "t1")
        #expect(lead == .busy(queued: 2))
        #expect(perms.first?.title == "rm -rf build")
        #expect(perms.first?.toolCall == nil)
        #expect(spaces.isEmpty)
        #expect(active == nil)
    }

    /// `watches` is for the CLI and tests; the client decodes the rest of the status past it.
    @Test func decodesStatusCarryingWebPaneWatches() throws {
        let text = #"{"ev":"status","req":2,"version":"0.1.0","uptime_secs":3,"log_path":"/l","terms":[],"lead":null,"perms":[],"spaces":[],"active":null,"watches":[{"pane":"p2","scope":"repo","name":"site","dir":"/tmp/p/.wheel/artifacts/site","watching":"/private/tmp/p","waiting":true}]}"#
        guard case let .status(req, _, logPath, _, _, _, _, _, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(req == 2)
        #expect(logPath == "/l")
    }

    @Test func decodesAPermissionRequestWithItsToolCall() throws {
        guard case let .permRequested(perm) = try ServerMsg.decode(
            #"{"ev":"perm_requested","perm":{"id":"p2","session":"lead","title":"rm -rf build","options":[],"tool_call":"call-1"}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(perm.toolCall == "call-1")
    }

    @Test func decodesToolContentAndItsReplacement() throws {
        guard case let .session(_, call) = try ServerMsg.decode(
            #"{"ev":"session","session":"lead","event":{"kind":"tool_call","id":"call-2","title":"Edit src/a.rs","tool_kind":"edit","status":"pending","content":[{"type":"text","text":"editing a.rs"},{"type":"diff","path":"src/a.rs","old_text":"one\n","new_text":"two\n"}]}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(call == .toolCall(id: "call-2", title: "Edit src/a.rs", kind: "edit", status: "pending",
                                  content: [.text("editing a.rs"), .diff(path: "src/a.rs", oldText: "one\n", newText: "two\n")]))
        guard case let .session(_, update) = try ServerMsg.decode(
            #"{"ev":"session","session":"lead","event":{"kind":"tool_update","id":"call-2","status":"completed","content":[{"type":"diff","path":"new.rs","new_text":"x"},{"type":"terminal","id":"term-1"},{"type":"hologram"}]}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(update == .toolUpdate(id: "call-2", title: nil, status: "completed",
                                      content: [.diff(path: "new.rs", oldText: nil, newText: "x"), .terminal(id: "term-1"), .other("hologram")]))
        // Without content, a call has none and an update leaves the row's content alone.
        guard case let .session(_, bare) = try ServerMsg.decode(#"{"ev":"session","session":"lead","event":{"kind":"tool_update","id":"c1","status":"failed"}}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(bare == .toolUpdate(id: "c1", title: nil, status: "failed", content: nil))
        guard case let .session(_, plain) = try ServerMsg.decode(
            #"{"ev":"session","session":"lead","event":{"kind":"tool_call","id":"c1","title":"ls","tool_kind":"execute","status":"pending"}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(plain == .toolCall(id: "c1", title: "ls", kind: "execute", status: "pending", content: []))
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

    /// A browser pane carries the page it is on; one with no `url` is a pane this build cannot
    /// draw rather than a browser on nothing.
    @Test func decodesABrowserPane() throws {
        let one = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p3","kind":"browser","url":"https://example.com/x"}"#.utf8))
        #expect(one == .pane(id: "p3", kind: .browser(url: "https://example.com/x")))
        guard case let .pane(_, kind) = one else { Issue.record("wrong case"); return }
        #expect(kind.name == "browser")
        let bare = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p4","kind":"browser"}"#.utf8))
        #expect(bare == .pane(id: "p4", kind: .unknown(kind: "browser", raw: ["kind": .string("browser")])))
    }

    @Test func decodesSessionEvents() throws {
        guard case let .sessionAttached(_, session, state, events, _) = try ServerMsg.decode(
            #"{"ev":"session_attached","session":"w5","state":{"phase":"ready"},"events":[{"kind":"user","turn":1,"text":"hi"},{"kind":"turn_start","turn":1},{"kind":"text","text":"hello"},{"kind":"turn_end","turn":1,"stop_reason":"end_turn"},{"kind":"error","message":"agent exited"}]}"#
        ) else { Issue.record("wrong case"); return }
        #expect(session == "w5")
        #expect(state == .ready)
        guard case let .sessionQueued(req, queued, turn) = try ServerMsg.decode(#"{"ev":"session_queued","req":3,"session":"w5","turn":2}"#),
              case let .sessionStateChanged(changed, busy) = try ServerMsg.decode(
                  #"{"ev":"session_state_changed","session":"lead","state":{"phase":"busy","queued":1}}"#)
        else { Issue.record("wrong case"); return }
        #expect(req == 3 && queued == "w5" && turn == 2)
        #expect(changed == "lead" && busy == .busy(queued: 1))
        guard case let .sessionStateChanged(_, gaveUp) = try ServerMsg.decode(
            #"{"ev":"session_state_changed","session":"w5","state":{"phase":"gave_up","tries":5,"message":"x is not a directory"}}"#)
        else { Issue.record("wrong case"); return }
        #expect(gaveUp == .gaveUp(tries: 5, message: "x is not a directory"))
        #expect(gaveUp.label == "Gave up after 5 tries: x is not a directory")
        #expect(events == [.user(turn: 1, text: "hi"), .turnStart(turn: 1), .text("hello"), .turnEnd(turn: 1, stopReason: "end_turn"), .error(turn: nil, message: "agent exited")])
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

    @Test func decodesTheConfigOptionsAnAgentOffers() throws {
        let json = #"""
        {"ev":"session_config","session":"lead","options":[
          {"id":"model","name":"Model","category":"model","description":"Which model","type":"select","current":"sonnet",
           "choices":[{"value":"sonnet","name":"Sonnet"},{"value":"haiku","name":"Haiku","description":"Fastest"}]},
          {"id":"fast","name":"Fast mode","category":"model_config","type":"boolean","current":true},
          {"id":"dial","name":"Dial","type":"something_new"}]}
        """#
        guard case let .sessionConfig(session, options) = try ServerMsg.decode(json) else { Issue.record("wrong case"); return }
        #expect(session == "lead")
        #expect(options.map(\.id) == ["model", "fast", "dial"])

        guard case let .select(current, choices) = options[0].kind else { Issue.record("model is not a select"); return }
        #expect(current == "sonnet")
        #expect(choices.map(\.value) == ["sonnet", "haiku"])
        #expect(choices[1].description == "Fastest")
        // The control shows the choice's name, not the raw value the agent matches on.
        #expect(options[0].currentLabel == "Sonnet")

        #expect(options[1].kind == .boolean(current: true))
        #expect(options[1].currentLabel == "On")

        // A control kind this version does not know still arrives, so the card can show that the
        // agent has one rather than silently dropping it.
        #expect(options[2].kind == .unknown)
        #expect(options[2].currentLabel == nil)
    }

    /// An agent that offers no controls, and one too old to send the field at all, both decode.
    @Test func anAttachWithoutConfigOptionsDecodes() throws {
        let json = #"{"ev":"session_attached","session":"lead","state":{"phase":"ready"},"events":[]}"#
        guard case let .sessionAttached(_, _, _, _, config) = try ServerMsg.decode(json) else { Issue.record("wrong case"); return }
        #expect(config.isEmpty)
    }

    /// A malformed option must not take the frame down with it: config options ride inside
    /// `session_attached`, so throwing here would drop the transcript and the session's state.
    @Test func aMalformedConfigOptionDoesNotDropTheWholeMessage() throws {
        let json = #"""
        {"ev":"session_attached","session":"lead","state":{"phase":"ready"},
         "events":[{"kind":"text","text":"still here"}],
         "config":[
           {"id":"a","name":"A select with no current","type":"select","choices":[{"value":"x","name":"X"}]},
           {"id":"b","name":"A boolean that is not one","type":"boolean","current":"yes"},
           {"id":"c","name":"Fine","type":"select","current":"x","choices":[{"value":"x","name":"X"}]}]}
        """#
        guard case let .sessionAttached(_, _, state, events, config) = try ServerMsg.decode(json) else {
            Issue.record("wrong case"); return
        }
        #expect(state == .ready)
        #expect(events.count == 1)
        #expect(config.map(\.id) == ["a", "b", "c"])
        #expect(config[0].kind == .unknown)
        #expect(config[1].kind == .unknown)
        #expect(config[2].kind == .select(current: "x", choices: [ConfigChoice(value: "x", name: "X", description: nil)]))
    }

    @Test func encodesAConfigSet() throws {
        let json = try ClientMsg.sessionConfigSet(req: 7, session: "lead", config: "model", value: "haiku").json()
        #expect(json == #"{"config":"model","op":"session_config_set","req":7,"session":"lead","value":"haiku"}"#)
    }
}
