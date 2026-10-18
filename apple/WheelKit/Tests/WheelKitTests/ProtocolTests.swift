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
        let text = #"{"ev":"status","req":2,"version":"0.1.0","uptime_secs":3,"log_path":"/l","terms":[],"lead":null,"perms":[],"spaces":[],"active":null,"watches":[{"pane":"p2","scope":"project","name":"site","dir":"/tmp/p/.wheel/artifacts/site","watching":"/private/tmp/p","waiting":true}]}"#
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

    @Test func decodesAModeTheAgentChangedItself() throws {
        guard case let .session(_, mode) = try ServerMsg.decode(#"{"ev":"session","session":"lead","event":{"kind":"mode","mode":"plan"}}"#)
        else { Issue.record("wrong case"); return }
        #expect(mode.event == .mode("plan"))
        var t = Transcript()
        t.apply(mode)
        #expect(t.rows.map(\.kind) == [.mode("plan")])
    }

    @Test func decodesThoughtsAndBlocks() throws {
        guard case let .session(_, thought) = try ServerMsg.decode(#"{"ev":"session","session":"lead","event":{"kind":"thought","text":"hmm"}}"#)
        else { Issue.record("wrong case"); return }
        #expect(thought.event == .thought("hmm"))
        guard case let .session(_, link) = try ServerMsg.decode(#"{"ev":"session","session":"lead","event":{"kind":"block","block_kind":"resource_link","name":"notes.md"}}"#)
        else { Issue.record("wrong case"); return }
        #expect(link.event == .block(kind: "resource_link", name: "notes.md"))
        guard case let .session(_, image) = try ServerMsg.decode(#"{"ev":"session","session":"lead","event":{"kind":"block","block_kind":"image"}}"#)
        else { Issue.record("wrong case"); return }
        #expect(image.event == .block(kind: "image", name: nil))
    }

    @Test func decodesToolContentAndItsReplacement() throws {
        guard case let .session(_, call) = try ServerMsg.decode(
            #"{"ev":"session","session":"lead","event":{"kind":"tool_call","id":"call-2","title":"Edit src/a.rs","tool_kind":"edit","status":"pending","content":[{"type":"text","text":"editing a.rs"},{"type":"diff","path":"src/a.rs","old_text":"one\n","new_text":"two\n"}]}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(call.event == .toolCall(id: "call-2", title: "Edit src/a.rs", kind: "edit", status: "pending",
                                  content: [.text("editing a.rs"), .diff(path: "src/a.rs", oldText: "one\n", newText: "two\n")]))
        guard case let .session(_, update) = try ServerMsg.decode(
            #"{"ev":"session","session":"lead","event":{"kind":"tool_update","id":"call-2","status":"completed","content":[{"type":"diff","path":"new.rs","new_text":"x"},{"type":"terminal","id":"term-1"},{"type":"hologram"}]}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(update.event == .toolUpdate(id: "call-2", title: nil, status: "completed",
                                      content: [.diff(path: "new.rs", oldText: nil, newText: "x"), .terminal(id: "term-1"), .other("hologram")]))
        // Without content, a call has none and an update leaves the row's content alone.
        guard case let .session(_, bare) = try ServerMsg.decode(#"{"ev":"session","session":"lead","event":{"kind":"tool_update","id":"c1","status":"failed"}}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(bare.event == .toolUpdate(id: "c1", title: nil, status: "failed", content: nil))
        guard case let .session(_, plain) = try ServerMsg.decode(
            #"{"ev":"session","session":"lead","event":{"kind":"tool_call","id":"c1","title":"ls","tool_kind":"execute","status":"pending"}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(plain.event == .toolCall(id: "c1", title: "ls", kind: "execute", status: "pending", content: []))
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

    /// A pane says who made it; one saved before panes recorded it is the person's, and an unknown
    /// pane's kept fields do not include it.
    @Test func decodesWhoMadeAPane() throws {
        let lead = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p3","created_by":"lead","kind":"empty"}"#.utf8))
        #expect(lead == .pane(id: "p3", kind: .empty, createdBy: .lead))
        let older = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p4","kind":"empty"}"#.utf8))
        #expect(older == .pane(id: "p4", kind: .empty, createdBy: .person))
        let unknown = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p9","created_by":"lead","kind":"hologram"}"#.utf8))
        #expect(unknown == .pane(id: "p9", kind: .unknown(kind: "hologram", raw: ["kind": .string("hologram")]), createdBy: .lead))
    }

    /// A browser pane carries the page it is on; one with no `url` is a pane this build cannot
    /// draw rather than a browser on nothing.
    @Test func decodesABrowserPane() throws {
        let one = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p3","kind":"browser","url":"https://example.com/x"}"#.utf8))
        #expect(one == .pane(id: "p3", kind: .browser(url: "https://example.com/x")))
        guard case let .pane(_, kind, _) = one else { Issue.record("wrong case"); return }
        #expect(kind.name == "browser")
        let bare = try JSONDecoder().decode(Node.self, from: Data(#"{"node":"pane","id":"p4","kind":"browser"}"#.utf8))
        #expect(bare == .pane(id: "p4", kind: .unknown(kind: "browser", raw: ["kind": .string("browser")])))
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
        guard case let .sessionAttached(_, session, state, events, _, _) = try ServerMsg.decode(
            #"{"ev":"session_attached","session":"w5","state":{"phase":"ready"},"events":[{"kind":"user","turn":1,"text":"hi"},{"kind":"turn_start","turn":1},{"kind":"text","text":"hello"},{"kind":"turn_end","turn":1,"stop_reason":"end_turn"},{"kind":"error","message":"agent exited"}],"now":1000}"#
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
        #expect(events.map(\.event) == [.user(turn: 1, text: "hi"), .turnStart(turn: 1), .text("hello"), .turnEnd(turn: 1, stopReason: "end_turn"), .error(turn: nil, message: "agent exited")])
    }

    /// The daemon's stamp sits beside the event's fields; replayed history has none.
    @Test func decodesWhenTheDaemonRecordedAnEvent() throws {
        guard case let .session(_, stamped) = try ServerMsg.decode(
            #"{"ev":"session","session":"lead","event":{"at":1760000000500,"kind":"turn_start","turn":2}}"#
        ) else { Issue.record("wrong case"); return }
        #expect(stamped.event == .turnStart(turn: 2))
        #expect(stamped.at == Date(timeIntervalSince1970: 1_760_000_000.5))
        guard case let .session(_, replayed) = try ServerMsg.decode(#"{"ev":"session","session":"lead","event":{"kind":"text","text":"hi"}}"#)
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
        let json = #"{"ev":"session_attached","session":"lead","state":{"phase":"ready"},"events":[],"now":1000}"#
        guard case let .sessionAttached(_, _, _, _, config, _) = try ServerMsg.decode(json) else { Issue.record("wrong case"); return }
        #expect(config.isEmpty)
    }

    /// `now` is the daemon's clock in milliseconds, read as a date.
    @Test func anAttachCarriesTheDaemonsClock() throws {
        let json = #"{"ev":"session_attached","session":"lead","state":{"phase":"ready"},"events":[],"now":1700000000500}"#
        guard case let .sessionAttached(_, _, _, _, _, now) = try ServerMsg.decode(json) else { Issue.record("wrong case"); return }
        #expect(now == Date(timeIntervalSince1970: 1_700_000_000.5))
    }

    /// A malformed option must not take the frame down with it: config options ride inside
    /// `session_attached`, so throwing here would drop the transcript and the session's state.
    @Test func aMalformedConfigOptionDoesNotDropTheWholeMessage() throws {
        let json = #"""
        {"ev":"session_attached","session":"lead","state":{"phase":"ready"},"now":1000,
         "events":[{"kind":"text","text":"still here"}],
         "config":[
           {"id":"a","name":"A select with no current","type":"select","choices":[{"value":"x","name":"X"}]},
           {"id":"b","name":"A boolean that is not one","type":"boolean","current":"yes"},
           {"id":"c","name":"Fine","type":"select","current":"x","choices":[{"value":"x","name":"X"}]}]}
        """#
        guard case let .sessionAttached(_, _, state, events, config, _) = try ServerMsg.decode(json) else {
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

    @Test func encodesSettingsOpsAndDecodesTheirReply() throws {
        #expect(try ClientMsg.settingsLs(req: 1, space: nil).json() == #"{"op":"settings_ls","req":1}"#)
        #expect(try ClientMsg.settingSet(req: 2, key: "browser.homepage", value: "about:blank", space: "s1", machine: false).json()
            == #"{"key":"browser.homepage","op":"setting_set","req":2,"space":"s1","value":"about:blank"}"#)
        #expect(try ClientMsg.settingSet(req: 3, key: "browser.homepage", value: "about:blank", space: nil, machine: true).json()
            == #"{"key":"browser.homepage","machine":true,"op":"setting_set","req":3,"value":"about:blank"}"#)
        #expect(try ClientMsg.settingUnset(req: 4, key: "browser.homepage", space: "s1", machine: false).json()
            == #"{"key":"browser.homepage","op":"setting_unset","req":4,"space":"s1"}"#)
        let text = #"{"ev":"settings","space":"s1","settings":[{"key":"browser.homepage","value":"https://docs.rs","origin":"machine","default":"about:blank","help":"The page.","machine":"https://docs.rs","problem":"/p/wheel.toml: bad"}]}"#
        guard case let .settings(req, space, settings) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(req == nil)
        #expect(space == "s1")
        #expect(settings == [SettingInfo(key: "browser.homepage", value: "https://docs.rs", origin: .machine, default: "about:blank",
                                         help: "The page.", machine: "https://docs.rs", project: nil, problem: "/p/wheel.toml: bad")])
    }

    @Test func listsNewsAndResumesConversations() throws {
        #expect(try ClientMsg.sessionConversationLs(req: 1, session: "lead").json() == #"{"op":"session_conversation_ls","req":1,"session":"lead"}"#)
        #expect(try ClientMsg.sessionConversationNew(req: 2, session: "lead").json() == #"{"op":"session_conversation_new","req":2,"session":"lead"}"#)
        #expect(try ClientMsg.sessionConversationResume(req: 3, session: "lead", id: "c1").json() == #"{"id":"c1","op":"session_conversation_resume","req":3,"session":"lead"}"#)
        guard case let .conversations(req, _, list) = try ServerMsg.decode(
            #"{"ev":"conversations","req":1,"session":"lead","conversations":[{"id":"c2","cwd":"/tmp","title":"hello","updated_at":"2026-10-13T21:56:25.120Z","current":true},{"id":"c1","cwd":"/tmp","current":false}]}"#
        ) else { Issue.record("wrong case"); return }
        #expect(req == 1)
        #expect(list == [ConversationInfo(id: "c2", cwd: "/tmp", title: "hello", updatedAt: "2026-10-13T21:56:25.120Z", current: true),
                         ConversationInfo(id: "c1", cwd: "/tmp", title: nil, updatedAt: nil, current: false)])
        guard case let .conversation(_, _, id) = try ServerMsg.decode(#"{"ev":"conversation","req":3,"session":"lead","id":"c1"}"#) else {
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

    @Test func encodesTheLeadLoginOpsAndDecodesTheLoginAUserSees() throws {
        #expect(try ClientMsg.leadLoginStatus(req: 1).json() == #"{"op":"lead_login_status","req":1}"#)
        #expect(try ClientMsg.leadLogin(req: 2, method: "claude-ai-login").json() == #"{"method":"claude-ai-login","op":"lead_login","req":2}"#)
        #expect(try ClientMsg.leadLogin(req: 3, method: nil).json() == #"{"op":"lead_login","req":3}"#)
        let text = #"{"ev":"lead_login_changed","login":{"state":"needed","preflight":{"logged_in":false,"auth_method":"none","raw":"{}"},"agent_required":true,"methods":[{"id":"claude-ai-login","name":"Claude Subscription","description":"Use Claude subscription"}],"kept_messages":["hello"],"disagreement":null,"last_exit":{"method":"claude-ai-login","pane":"p2","code":3}}}"#
        guard case let .leadLoginChanged(login) = try ServerMsg.decode(text) else { Issue.record("wrong case"); return }
        #expect(login.state == "needed")
        #expect(login.showsPicker)
        #expect(login.preflight?.loggedIn == false)
        #expect(login.methods.map(\.id) == ["claude-ai-login"])
        #expect(login.keptMessages == ["hello"])
        #expect(login.lastExitCode == 3)
        guard case let .leadLoginFinished(req, code) = try ServerMsg.decode(#"{"ev":"lead_login_finished","req":2,"method":"x","pane":"p2","code":0}"#) else {
            Issue.record("wrong case"); return
        }
        #expect(req == 2)
        #expect(code == 0)
    }
}
