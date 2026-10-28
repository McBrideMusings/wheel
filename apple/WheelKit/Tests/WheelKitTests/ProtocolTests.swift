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
        #expect(try ClientMsg.sessionAttach(req: 5, session: "w1").json() == #"{"op":"session_attach","req":5,"session":"w1"}"#)
        #expect(try ClientMsg.sessionSend(req: nil, session: "w5", text: "hi").json() == #"{"op":"session_send","session":"w5","text":"hi"}"#)
        #expect(try ClientMsg.sessionCancel(req: 6, session: "w5").json() == #"{"op":"session_cancel","req":6,"session":"w5"}"#)
        #expect(try ClientMsg.sessionDetach(session: "w5").json() == #"{"op":"session_detach","session":"w5"}"#)
    }

    /// The built-in messages as wheeld's serde writes and reads them.
    @Test func builtinMessagesMatchTheDaemonsSpelling() throws {
        let update = try JSONSerialization.jsonObject(with: Data(ClientMsg.builtinUpdate(req: 4, name: "sample", onEdited: .keep).json().utf8)) as? [String: Any]
        #expect(update?["op"] as? String == "builtin_update")
        #expect(update?["on_edited"] as? String == "keep")
        let listed = try ServerMsg.decode(#"{"ev":"builtins","req":4,"builtins":[{"name":"sample","shipped_version":2,"installed_version":1,"edited_files":["index.html"]},{"name":"diff","shipped_version":1,"edited_files":[]}]}"#)
        guard case let .builtins(_, builtins) = listed else { Issue.record("not builtins: \(listed)"); return }
        #expect(builtins.map(\.action) == [.update, .install])
        #expect(builtins[0].editedFiles == ["index.html"])
        let edited = try ServerMsg.decode(#"{"ev":"builtin_edited","req":5,"name":"sample","edited":[{"path":"index.html","diff":"-a\n+b\n"}]}"#)
        guard case let .builtinEdited(_, name, files) = edited else { Issue.record("not builtin_edited: \(edited)"); return }
        #expect(name == "sample")
        #expect(files == [EditedFile(path: "index.html", diff: "-a\n+b\n")])
    }

    @Test func decodesTerminalOutput() throws {
        guard case let .termOutput(id, data) = try ServerMsg.decode(#"{"ev":"term_output","id":"t1","data":"bWFjLTQy"}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(id == "t1")
        #expect(String(decoding: data, as: UTF8.self) == "mac-42")
    }

    @Test func decodesStatusWithPerms() throws {
        let text = #"{"ev":"status","req":1,"version":"0.1.0","uptime_secs":3,"log_path":"/l","agents":[],"terms":[{"id":"t1","pid":5,"cwd":"/tmp","cols":80,"rows":24,"alive":true}],"perms":[{"id":"p1","session":"w1","title":"rm -rf build","options":[{"id":"a","name":"Allow","kind":"allow_once"}]}]}"#
        guard case let .status(req, _, _, _, terms, perms, spaces, active, _, _, _, _, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(req == 1)
        #expect(terms.first?.id == "t1")
        #expect(perms.first?.title == "rm -rf build")
        #expect(perms.first?.toolCall == nil)
        #expect(spaces.isEmpty)
        #expect(active == nil)
    }

    @Test func statusListsTheAgentsAndWhetherTheirProgramsWereFound() throws {
        let base = #"{"ev":"status","req":1,"version":"0.1.0","uptime_secs":3,"log_path":"/l","terms":[],"perms":[],"spaces":[],"active":null"#
        let text = base + #","agents":[{"name":"claude","command":"claude","path":"/usr/local/bin/claude","found":true},{"name":"codex","command":"codex","found":false,"why":"codex is not on PATH"}]}"#
        guard case let .status(_, _, _, _, _, _, _, _, agents, _, _, _, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(agents == [
            AgentInfo(name: "claude", command: "claude", path: "/usr/local/bin/claude", found: true),
            AgentInfo(name: "codex", command: "codex", found: false, why: "codex is not on PATH"),
        ])
    }

    @Test func statusCarriesTheShellIntegrationProblem() throws {
        let base = #"{"ev":"status","req":1,"version":"0.1.0","uptime_secs":3,"log_path":"/l","agents":[],"terms":[],"perms":[],"spaces":[],"active":null"#
        guard case let .status(_, _, _, _, _, _, _, _, _, problem, _, _, _) = try ServerMsg.decode(base + #","shell_integration":"no wheel"}"#) else { Issue.record("wrong case"); return }
        #expect(problem == "no wheel")
        guard case let .status(_, _, _, _, _, _, _, _, _, none, _, _, _) = try ServerMsg.decode(base + "}") else { Issue.record("wrong case"); return }
        #expect(none == nil)
        guard case let .status(_, _, _, _, _, _, _, _, _, _, login, _, _) = try ServerMsg.decode(base + #","login_path":{"path":"/usr/bin","how":"started with","problem":"zsh hung"}}"#) else { Issue.record("wrong case"); return }
        #expect(login == LoginPath(path: "/usr/bin", how: "started with", problem: "zsh hung"))
        guard case let .status(_, _, _, _, _, _, _, _, _, _, _, logs, _) = try ServerMsg.decode(base + #","logs":{"url":"http://l/push","state":"failing","last_ok_ms":7,"waiting_bytes":512,"error":"refused"}}"#) else { Issue.record("wrong case"); return }
        #expect(logs == LogShipping(url: "http://l/push", state: "failing", lastOkMs: 7, waitingBytes: 512, error: "refused"))
        guard case let .status(_, _, _, _, _, _, _, _, _, _, _, _, host) = try ServerMsg.decode(base + #","host":{"exe":"/Applications/Wheel.app/Contents/MacOS/wheeld","folders":[{"path":"/Users/p/Downloads","allowed":false,"error":"Operation not permitted (os error 1)"},{"path":"/Users/p/Desktop","allowed":true}]}}"#) else { Issue.record("wrong case"); return }
        #expect(host == HostAccess(exe: "/Applications/Wheel.app/Contents/MacOS/wheeld", folders: [FolderAccess(path: "/Users/p/Downloads", allowed: false, error: "Operation not permitted (os error 1)"), FolderAccess(path: "/Users/p/Desktop", allowed: true)]))
    }

    /// `watches` is for the CLI and tests; the client decodes the rest of the status past it.
    @Test func decodesStatusCarryingWebPaneWatches() throws {
        let text = #"{"ev":"status","req":2,"version":"0.1.0","uptime_secs":3,"log_path":"/l","agents":[],"terms":[],"perms":[],"spaces":[],"active":null,"watches":[{"pane":"p2","scope":"project","name":"site","dir":"/tmp/p/.wheel/artifacts/site","watching":"/private/tmp/p","waiting":true}]}"#
        guard case let .status(req, _, logPath, _, _, _, _, _, _, _, _, _, _) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(req == 2)
        #expect(logPath == "/l")
    }

    @Test func decodesAPermissionRequestWithItsToolCall() throws {
        guard case let .permRequested(perm) = try ServerMsg.decode(
            #"{"ev":"perm_requested","perm":{"id":"p2","session":"w1","title":"rm -rf build","options":[],"tool_call":"call-1"}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(perm.toolCall == "call-1")
    }

    @Test func decodesAModeTheAgentChangedItself() throws {
        guard case let .session(_, mode) = try ServerMsg.decode(#"{"ev":"session","session":"w1","event":{"kind":"mode","mode":"plan"}}"#)
        else { Issue.record("wrong case"); return }
        #expect(mode.event == .mode("plan"))
        var t = Transcript()
        t.apply(mode)
        #expect(t.rows.map(\.kind) == [.mode("plan")])
    }

    @Test func decodesThoughts() throws {
        guard case let .session(_, thought) = try ServerMsg.decode(#"{"ev":"session","session":"w1","event":{"kind":"thought","text":"hmm"}}"#)
        else { Issue.record("wrong case"); return }
        #expect(thought.event == .thought("hmm"))
    }

    @Test func decodesToolContentAndItsReplacement() throws {
        guard case let .session(_, call) = try ServerMsg.decode(
            #"{"ev":"session","session":"w1","event":{"kind":"tool_call","id":"call-2","title":"Edit src/a.rs","tool_kind":"edit","status":"pending","content":[{"type":"text","text":"editing a.rs"},{"type":"diff","path":"src/a.rs","old_text":"one\n","new_text":"two\n"}]}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(call.event == .toolCall(id: "call-2", title: "Edit src/a.rs", kind: "edit", status: "pending",
                                  content: [.text("editing a.rs"), .diff(path: "src/a.rs", oldText: "one\n", newText: "two\n")]))
        guard case let .session(_, update) = try ServerMsg.decode(
            #"{"ev":"session","session":"w1","event":{"kind":"tool_update","id":"call-2","status":"completed","content":[{"type":"diff","path":"new.rs","new_text":"x"},{"type":"hologram"}]}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(update.event == .toolUpdate(id: "call-2", title: nil, status: "completed",
                                      content: [.diff(path: "new.rs", oldText: nil, newText: "x"), .other("hologram")]))
        // Without content, a call has none and an update leaves the row's content alone.
        guard case let .session(_, bare) = try ServerMsg.decode(#"{"ev":"session","session":"w1","event":{"kind":"tool_update","id":"c1","status":"failed"}}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(bare.event == .toolUpdate(id: "c1", title: nil, status: "failed", content: nil))
        guard case let .session(_, plain) = try ServerMsg.decode(
            #"{"ev":"session","session":"w1","event":{"kind":"tool_call","id":"c1","title":"ls","tool_kind":"execute","status":"pending"}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(plain.event == .toolCall(id: "c1", title: "ls", kind: "execute", status: "pending", content: []))
    }

    @Test func encodesSpaceOps() throws {
        #expect(try ClientMsg.spaceCreate(req: 1, cwd: "/tmp").json() == #"{"cwd":"\/tmp","op":"space_create","req":1}"#)
        #expect(try ClientMsg.spaceRecent(req: 2).json() == #"{"op":"space_recent","req":2}"#)
        let recent = try ServerMsg.decode(#"{"ev":"space_recent","req":2,"dirs":[{"dir":"/tmp/b","opened_ms":9},{"dir":"/tmp/a","opened_ms":4}]}"#)
        guard case let .spaceRecent(2, dirs) = recent else { Issue.record("not space_recent: \(recent)"); return }
        #expect(dirs == [RecentDir(dir: "/tmp/b", openedMs: 9), RecentDir(dir: "/tmp/a", openedMs: 4)])
        #expect(try ClientMsg.focus(req: nil, space: nil, pane: "p2").json() == #"{"op":"focus","pane":"p2"}"#)
        #expect(try ClientMsg.paneSwap(req: 3, pane: "p1", with: "p2").json() == #"{"op":"pane_swap","pane":"p1","req":3,"with":"p2"}"#)
        #expect(try ClientMsg.paneMove(req: 4, pane: "p1", target: "p2", dir: .up).json() == #"{"op":"pane_move","pane":"p1","req":4,"to":{"dir":"up","kind":"beside","pane":"p2"}}"#)
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

    /// Pairing: the token the daemon issues, the devices it lists, and the credential a phone is
    /// handed as its connection's first frame.
    @Test func decodesThePairingFrames() throws {
        let pairing = try ServerMsg.decode(#"{"ev":"pairing","req":4,"token":"abc","expires_ms":1700000300000}"#)
        guard case let .pairing(req, token, expires) = pairing else { Issue.record("wrong case"); return }
        #expect((req, token, expires) == (4, "abc", 1_700_000_300_000))
        let devices = try ServerMsg.decode(#"{"ev":"devices_changed","devices":[{"id":"d1","name":"iPhone","paired_ms":5,"last_seen_ms":9,"connected":1}]}"#)
        guard case let .devicesChanged(list) = devices else { Issue.record("wrong case"); return }
        #expect(list == [DeviceInfo(id: "d1", name: "iPhone", pairedMs: 5, lastSeenMs: 9, connected: 1)])
        let paired = try ServerMsg.decode(#"{"ev":"paired","device":"d1","credential":"secret"}"#)
        guard case let .paired(device, credential) = paired else { Issue.record("wrong case"); return }
        #expect((device, credential) == ("d1", "secret"))
    }

    /// `pane_url` carries one pane's new page, and applying it leaves the rest of the tree alone.
    @Test func decodesAndAppliesAPaneUrl() throws {
        guard case let .paneUrl(pane, url) = try ServerMsg.decode(#"{"ev":"pane_url","pane":"p3","url":"https://example.com/two"}"#)
        else { Issue.record("wrong case"); return }
        #expect(pane == "p3" && url == "https://example.com/two")

        let tree = Node.split(id: "x1", axis: .horizontal, ratio: 0.5,
                              a: .pane(id: "p2", kind: .terminal(term: "t1")),
                              b: .pane(id: "p3", kind: .browser(url: "https://example.com/one")))
        #expect(tree.replacing(pane: "p3", with: .browser(url: url))
                == .split(id: "x1", axis: .horizontal, ratio: 0.5,
                          a: .pane(id: "p2", kind: .terminal(term: "t1")),
                          b: .pane(id: "p3", kind: .browser(url: "https://example.com/two"))))
        // A pane the tree does not hold changes nothing, which is what lets a client apply a
        // broadcast about a space it is not showing.
        #expect(tree.replacing(pane: "p9", with: .browser(url: url)) == tree)
    }

    /// The daemon asks for a pane drawn with `render_pane`; the client's answers carry the same id.
    @Test func rendersAPaneOnRequest() throws {
        guard case let .renderPane(id, pane) = try ServerMsg.decode(#"{"ev":"render_pane","id":4,"pane":"p6"}"#)
        else { Issue.record("wrong case"); return }
        #expect(id == 4 && pane == "p6")
        #expect(try ClientMsg.renderOffer.json() == #"{"op":"render_offer"}"#)
        #expect(try ClientMsg.rendered(id: 4, png: Data([0x89, 0x50])).json() == #"{"id":4,"op":"rendered","png":"iVA="}"#)
        #expect(try ClientMsg.renderRefused(id: 4, message: RenderRefusal.notShown).json()
                == #"{"id":4,"message":"not shown","op":"render_refused"}"#)
    }

    @Test func decodesSessionEvents() throws {
        guard case let .sessionAttached(_, session, state, events, _, _, _) = try ServerMsg.decode(
            #"{"ev":"session_attached","session":"w5","state":{"phase":"ready"},"events":[{"kind":"user","turn":1,"text":"hi"},{"kind":"turn_start","turn":1},{"kind":"text","text":"hello"},{"kind":"turn_end","turn":1,"stop_reason":"end_turn"},{"kind":"error","message":"agent exited"}],"now":1000}"#
        ) else { Issue.record("wrong case"); return }
        #expect(session == "w5")
        #expect(state == .ready)
        guard case let .sessionQueued(req, queued, turn) = try ServerMsg.decode(#"{"ev":"session_queued","req":3,"session":"w5","turn":2}"#),
              case let .sessionStateChanged(changed, busy) = try ServerMsg.decode(
                  #"{"ev":"session_state_changed","session":"w1","state":{"phase":"busy","queued":1}}"#)
        else { Issue.record("wrong case"); return }
        #expect(req == 3 && queued == "w5" && turn == 2)
        #expect(changed == "w1" && busy == .busy(queued: 1))
        guard case let .sessionStateChanged(_, gaveUp) = try ServerMsg.decode(
            #"{"ev":"session_state_changed","session":"w5","state":{"phase":"gave_up","tries":5,"message":"x is not a directory"}}"#)
        else { Issue.record("wrong case"); return }
        #expect(gaveUp == .gaveUp(tries: 5, message: "x is not a directory"))
        #expect(gaveUp.label == "Gave up after 5 tries: x is not a directory")
        guard case let .sessionStateChanged(_, switching) = try ServerMsg.decode(
            #"{"ev":"session_state_changed","session":"w5","state":{"phase":"switching"}}"#)
        else { Issue.record("wrong case"); return }
        #expect(switching == .switching && switching.label == "Switching to the terminal")
        #expect(events.map(\.event) == [.user(turn: 1, text: "hi"), .turnStart(turn: 1), .text("hello"), .turnEnd(turn: 1, stopReason: "end_turn"), .error(turn: nil, message: "agent exited")])
    }

    /// The daemon's stamp sits beside the event's fields; replayed history has none.
    @Test func decodesWhenTheDaemonRecordedAnEvent() throws {
        guard case let .session(_, stamped) = try ServerMsg.decode(
            #"{"ev":"session","session":"w1","event":{"at":1760000000500,"kind":"turn_start","turn":2}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(stamped.event == .turnStart(turn: 2))
        #expect(stamped.at == Date(timeIntervalSince1970: 1_760_000_000.5))
        guard case let .session(_, replayed) = try ServerMsg.decode(#"{"ev":"session","session":"w1","event":{"kind":"text","text":"hi"}}"#)
        else { Issue.record("wrong case"); return }
        #expect(replayed.at == nil)
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
        {"ev":"session_config","session":"w1","options":[
          {"id":"model","name":"Model","category":"model","description":"Which model","type":"select","current":"sonnet",
           "choices":[{"value":"sonnet","name":"Sonnet"},{"value":"haiku","name":"Haiku","description":"Fastest"}]},
          {"id":"fast","name":"Fast mode","category":"model_config","type":"boolean","current":true},
          {"id":"dial","name":"Dial","type":"something_new"}]}
        """#
        guard case let .sessionConfig(session, options) = try ServerMsg.decode(json) else { Issue.record("wrong case"); return }
        #expect(session == "w1")
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
        let json = #"{"ev":"session_attached","session":"w1","state":{"phase":"ready"},"events":[],"now":1000}"#
        guard case let .sessionAttached(_, _, _, _, config, _, _) = try ServerMsg.decode(json) else { Issue.record("wrong case"); return }
        #expect(config.isEmpty)
    }

    /// `now` is the daemon's clock in milliseconds, read as a date.
    @Test func anAttachCarriesTheDaemonsClock() throws {
        let json = #"{"ev":"session_attached","session":"w1","state":{"phase":"ready"},"events":[],"now":1700000000500}"#
        guard case let .sessionAttached(_, _, _, _, _, _, now) = try ServerMsg.decode(json) else { Issue.record("wrong case"); return }
        #expect(now == Date(timeIntervalSince1970: 1_700_000_000.5))
    }

    /// A malformed option must not take the frame down with it: config options ride inside
    /// `session_attached`, so throwing here would drop the transcript and the session's state.
    @Test func aMalformedConfigOptionDoesNotDropTheWholeMessage() throws {
        let json = #"""
        {"ev":"session_attached","session":"w1","state":{"phase":"ready"},"now":1000,
         "events":[{"kind":"text","text":"still here"}],
         "config":[
           {"id":"a","name":"A select with no current","type":"select","choices":[{"value":"x","name":"X"}]},
           {"id":"b","name":"A boolean that is not one","type":"boolean","current":"yes"},
           {"id":"c","name":"Fine","type":"select","current":"x","choices":[{"value":"x","name":"X"}]}]}
        """#
        guard case let .sessionAttached(_, _, state, events, config, _, _) = try ServerMsg.decode(json) else {
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
        let json = try ClientMsg.sessionConfigSet(req: 7, session: "w1", config: "model", value: "haiku").json()
        #expect(json == #"{"config":"model","op":"session_config_set","req":7,"session":"w1","value":"haiku"}"#)
    }

    @Test func decodesATitleAndAPaneInfo() throws {
        guard case let .sessionTitle(session, title) = try ServerMsg.decode(#"{"ev":"session_title","session":"w1","title":"Run an improve critique"}"#) else { Issue.record("wrong case"); return }
        #expect(session == "w1")
        #expect(title == "Run an improve critique")
        let info = #"{"ev":"pane_info","req":3,"pane":"p2","kind":"agent","fields":[{"label":"Session","value":"w1","hint":"Wheel's name"},{"label":"Pane","value":"p2"}]}"#
        guard case let .paneInfo(req, pane, kind, fields) = try ServerMsg.decode(info) else { Issue.record("wrong case"); return }
        #expect(req == 3)
        #expect(pane == "p2")
        #expect(kind == "agent")
        #expect(fields == [InfoField(label: "Session", value: "w1", hint: "Wheel's name"), InfoField(label: "Pane", value: "p2")])
        #expect(try ClientMsg.paneInfo(req: 3, pane: "p2").json() == #"{"op":"pane_info","pane":"p2","req":3}"#)
    }

    @Test func decodesANoticeGoingUpAndComingDown() throws {
        let up = #"{"ev":"session_notice","session":"w1","notice":"session ended: could not reopen that conversation, so this is a new one"}"#
        guard case let .sessionNotice(session, notice) = try ServerMsg.decode(up) else { Issue.record("wrong case"); return }
        #expect(session == "w1")
        #expect(notice == "session ended: could not reopen that conversation, so this is a new one")
        guard case let .sessionNotice(_, down) = try ServerMsg.decode(#"{"ev":"session_notice","session":"w1"}"#) else { Issue.record("wrong case"); return }
        #expect(down == nil)
    }

    @Test func aTerminalPanesAgentCarriesItsContextUse() throws {
        let with = #"{"node":"pane","id":"p2","kind":"terminal","term":"t1","agent":{"name":"claude","session":"s","context":{"used":74100,"size":200000}}}"#
        guard case let .pane(_, .terminal(_, agent)) = try JSONDecoder().decode(Node.self, from: Data(with.utf8)) else { Issue.record("wrong case"); return }
        #expect(agent?.context == ContextUse(used: 74_100, size: 200_000))
        let without = #"{"node":"pane","id":"p2","kind":"terminal","term":"t1","agent":{"name":"claude","session":"s"}}"#
        guard case let .pane(_, .terminal(_, bare)) = try JSONDecoder().decode(Node.self, from: Data(without.utf8)) else { Issue.record("wrong case"); return }
        #expect(bare?.context == nil)
    }

    @Test func decodesTheContextUseGoingUpAndBeingCleared() throws {
        let up = #"{"ev":"session_context","session":"w3","context":{"used":53000,"size":200000}}"#
        guard case let .sessionContext(session, context) = try ServerMsg.decode(up) else { Issue.record("wrong case"); return }
        #expect(session == "w3")
        #expect(context == ContextUse(used: 53_000, size: 200_000))
        #expect(context?.percent == 27)
        #expect(context?.shortText == "27%")
        #expect(context?.rawText == "53k / 200k")
        #expect(context?.text(raw: true) == "53k / 200k")
        #expect(context?.text(raw: false) == "27%")
        #expect(ContextUse(used: 420_110, size: 1_000_000).rawText == "420k / 1M")
        #expect(ContextUse(used: 950, size: 200_000).rawText == "950 / 200k")
        #expect(context?.helpText == "Context window: 53,000 of 200,000 tokens used (27%)")
        guard case let .sessionContext(_, cleared) = try ServerMsg.decode(#"{"ev":"session_context","session":"w3"}"#) else { Issue.record("wrong case"); return }
        #expect(cleared == nil)
    }

    @Test func decodesTheSlashCommandsAnAgentAccepts() throws {
        let json = #"{"ev":"session_commands","session":"w3","commands":["clear","compact","review"]}"#
        guard case let .sessionCommands(session, commands) = try ServerMsg.decode(json) else { Issue.record("wrong case"); return }
        #expect(session == "w3")
        #expect(commands == ["clear", "compact", "review"])
    }

    @Test func encodesANoticeDismissal() throws {
        let json = try ClientMsg.sessionNoticeDismiss(req: 4, session: "w1").json()
        #expect(json == #"{"op":"session_notice_dismiss","req":4,"session":"w1"}"#)
    }

    @Test func encodesSettingsOpsAndDecodesTheirReply() throws {
        #expect(try ClientMsg.settingsLs(req: 1, space: nil).json() == #"{"op":"settings_ls","req":1}"#)
        #expect(try ClientMsg.settingSet(req: 2, key: "browser.homepage", value: "about:blank", space: "s1", machine: false).json()
            == #"{"key":"browser.homepage","op":"setting_set","req":2,"space":"s1","value":"about:blank"}"#)
        #expect(try ClientMsg.settingSet(req: 3, key: "browser.homepage", value: "about:blank", space: nil, machine: true).json()
            == #"{"key":"browser.homepage","machine":true,"op":"setting_set","req":3,"value":"about:blank"}"#)
        #expect(try ClientMsg.settingUnset(req: 4, key: "browser.homepage", space: "s1", machine: false).json()
            == #"{"key":"browser.homepage","op":"setting_unset","req":4,"space":"s1"}"#)
        let text = #"{"ev":"settings","space":"s1","settings":[{"key":"browser.homepage","value":"https://docs.rs","origin":"machine","default":"about:blank","help":"The page.","machine":"https://docs.rs","machine_only":true,"problem":"/p/wheel.toml: bad"}]}"#
        guard case let .settings(req, space, settings) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(req == nil)
        #expect(space == "s1")
        #expect(settings == [SettingInfo(key: "browser.homepage", value: "https://docs.rs", origin: .machine, default: "about:blank",
                                         help: "The page.", machine: "https://docs.rs", project: nil, machineOnly: true, problem: "/p/wheel.toml: bad")])
    }

    @Test func listsNewsAndResumesConversations() throws {
        #expect(try ClientMsg.sessionConversationLs(req: 1, session: "w1").json() == #"{"op":"session_conversation_ls","req":1,"session":"w1"}"#)
        #expect(try ClientMsg.sessionConversationNew(req: 2, session: "w1").json() == #"{"op":"session_conversation_new","req":2,"session":"w1"}"#)
        #expect(try ClientMsg.sessionConversationResume(req: 3, session: "w1", id: "c1").json() == #"{"id":"c1","op":"session_conversation_resume","req":3,"session":"w1"}"#)
        guard case let .conversations(req, _, list) = try ServerMsg.decode(
            #"{"ev":"conversations","req":1,"session":"w1","conversations":[{"id":"c2","cwd":"/tmp","title":"hello","updated_at":"2026-10-13T21:56:25.120Z","current":true},{"id":"c1","cwd":"/tmp","current":false}]}"#
        ) else { Issue.record("wrong case"); return }
        #expect(req == 1)
        #expect(list == [ConversationInfo(id: "c2", cwd: "/tmp", title: "hello", updatedAt: "2026-10-13T21:56:25.120Z", current: true),
                         ConversationInfo(id: "c1", cwd: "/tmp", title: nil, updatedAt: nil, current: false)])
        guard case let .conversation(_, _, id) = try ServerMsg.decode(#"{"ev":"conversation","req":3,"session":"w1","id":"c1"}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(id == "c1")
    }

    /// A row says how long ago its conversation was worked in, whether the agent's timestamp has
    /// fractional seconds or not.
    @Test func aConversationsAgeReadsAsTimeAgo() {
        let now = ISO8601DateFormatter().date(from: "2026-10-13T22:00:00Z")!
        let row = { (at: String?) in ConversationInfo(id: "c", cwd: "/", title: nil, updatedAt: at, current: false).age(now: now) }
        #expect(row("2026-10-13T21:59:30.5Z") == "just now")
        #expect(row("2026-10-13T21:55:00Z") == "5 min. ago")
        #expect(row("2026-10-11T22:00:00Z") == "2 days ago")
        #expect(row(nil) == nil)
        #expect(row("yesterday") == nil)
    }
}
