import Foundation
import Testing
import WheelKit
@testable import WheelUI

/// `PluginPaneLink` against wheeld's messages as the phone decodes them: the script loads and reloads with its state kept,
/// a broken save keeps the view and sets the error, feeds and actions go out and their replies come back, and a dropped
/// connection asks for everything again on the next one.
@MainActor @Suite struct PluginPaneLinkTests {
    static let script = """
    return {
        init = function(previous) return previous or { count = 0 } end,
        update = function(state, msg)
            if msg.type == "add" then state.count += 1; wheel.action("p.go", { n = state.count }) end
            if msg.action == "p.go" then state.said = if msg.error then "failed: " .. msg.error else "done " .. tostring(msg.result.ok) end
        end,
        view = function(state, data)
            local f = data.feed("p.f", { k = 1 })
            return ui.Column{
                ui.Text("Count " .. state.count),
                ui.Button{ child = ui.Text("Add"), action = { type = "add" } },
                ui.Text("feed " .. tostring(if f == nil then "nil" else f.n)),
                ui.Text(state.said or "none"),
            }
        end,
    }
    """

    final class Wire {
        var sent: [ClientMsg] = []
        var req: UInt64 = 0
    }

    func make(_ wire: Wire) -> PluginPaneLink {
        let link = PluginPaneLink(pane: "p1", context: ["pane": .string("p1"), "plugin": .string("p"), "plugin_pane": .string("c")], send: { wire.sent.append($0) }, nextReq: { wire.req += 1; return wire.req })
        link.start()
        return link
    }

    func msg(_ text: String) throws -> ServerMsg { try ServerMsg.decode(text) }

    func source(_ s: String, req: UInt64? = nil, plugin: String = "p") throws -> ServerMsg {
        let body = try JSONSerialization.data(withJSONObject: ["ev": "plugin_source", "pane": "p1", "plugin": plugin, "plugin_pane": "c", "chunk": "p/main.luau", "source": s]
            .merging(req.map { ["req": $0] } ?? [:]) { a, _ in a })
        return try msg(String(decoding: body, as: UTF8.self))
    }

    func texts(_ link: PluginPaneLink) -> [String] { link.plugin.componentTexts.compactMap { $0["text"] } }

    @Test func theScriptLoadsReloadsWithStateKeptAndABrokenSaveKeepsTheView() throws {
        let wire = Wire()
        let link = make(wire)
        #expect(wire.sent == [.pluginSource(req: 1, pane: "p1")])
        #expect(link.handle(try source(Self.script, req: 1)))
        #expect(texts(link).first == "Count 0")
        #expect(link.plugin.press(component: "root.1"))
        #expect(texts(link).first == "Count 1")
        // A save on the Mac: pushed with no req, the counter kept.
        #expect(link.handle(try source(Self.script.replacingOccurrences(of: "\"Count \"", with: "\"Total \""))))
        #expect(texts(link).first == "Total 1")
        #expect(link.plugin.reloads == 1)
        #expect(link.plugin.error == nil)
        #expect(link.handle(try source("return {")))
        #expect(link.plugin.error?.contains("p/main.luau") == true)
        #expect(texts(link).first == "Total 1")
        #expect(link.handle(try source(Self.script)))
        #expect(link.plugin.error == nil)
        #expect(texts(link).first == "Count 1")
        // A script for the plugin the pane ran before is taken and dropped.
        #expect(link.handle(try source("return {", plugin: "old")))
        #expect(link.plugin.error == nil)
        // Another pane's script is not this one's.
        #expect(!link.handle(try msg(#"{"ev":"plugin_source","pane":"p2","plugin":"p","plugin_pane":"c","chunk":"x","source":"return {"}"#)))
        #expect(link.plugin.error == nil)
    }

    @Test func feedsAndActionsGoOutAndTheirRepliesComeBack() throws {
        let wire = Wire()
        let link = make(wire)
        link.handle(try source(Self.script, req: 1))
        #expect(wire.sent.last == .feedSubscribe(req: 2, pane: "p1", feed: "p.f", params: .object(["k": .number(1)])))
        #expect(link.handle(try msg(#"{"ev":"feed_update","req":2,"pane":"p1","feed":"p.f","params":{"k":1},"value":{"n":5}}"#)))
        #expect(texts(link)[2] == "feed 5")
        #expect(link.handle(try msg(#"{"ev":"feed_update","pane":"p1","feed":"p.f","params":{"k":1},"patch":[{"op":"replace","path":"/n","value":6}]}"#)))
        #expect(texts(link)[2] == "feed 6")
        // A null value is a value: the backend has sent none yet.
        #expect(link.handle(try msg(#"{"ev":"feed_update","pane":"p1","feed":"p.f","params":{"k":1},"value":null}"#)))
        #expect(texts(link)[2] == "feed nil")
        link.plugin.press(component: "root.1")
        #expect(wire.sent.last == .pluginAction(req: 3, pane: "p1", action: "p.go", args: .object(["n": .number(1)])))
        #expect(link.handle(try msg(#"{"ev":"plugin_action","req":3,"pane":"p1","action":"p.go","result":{"ok":true}}"#)))
        #expect(texts(link)[3] == "done true")
        link.plugin.press(component: "root.1")
        #expect(link.handle(try msg(#"{"ev":"error","req":4,"message":"refused"}"#)))
        #expect(texts(link)[3] == "failed: refused")
        // An error for a request this pane did not send is not its.
        #expect(!link.handle(try msg(#"{"ev":"error","req":99,"message":"x"}"#)))
    }

    @Test func aDroppedConnectionFailsWhatWaitedAndTheNextAsksAgain() async throws {
        let wire = Wire()
        let link = make(wire)
        link.handle(try source(Self.script, req: 1))
        link.plugin.press(component: "root.1")
        link.dropped()
        #expect(link.plugin.error == "lost the connection to wheeld, retrying")
        #expect(texts(link)[3] == "failed: lost the connection to wheeld")
        // A late reply to a request of the dropped connection is not taken.
        #expect(!link.handle(try msg(#"{"ev":"plugin_action","req":3,"pane":"p1","action":"p.go","result":{"ok":true}}"#)))
        // An action while no connection is up fails without being sent.
        wire.sent = []
        link.plugin.press(component: "root.1")
        #expect(wire.sent.isEmpty)
        try await Task.sleep(nanoseconds: 50_000_000)
        #expect(texts(link)[3] == "failed: no connection to wheeld")
        wire.sent = []
        link.start()
        #expect(wire.sent.count == 2)
        #expect(wire.sent.first == .pluginSource(req: 4, pane: "p1"))
        #expect(wire.sent.last == .feedSubscribe(req: 5, pane: "p1", feed: "p.f", params: .object(["k": .number(1)])))
        link.handle(try source(Self.script, req: 4))
        #expect(link.plugin.error == nil)
        #expect(texts(link).first == "Count 2")
    }

    @Test func thePhoneReadsAPluginPaneFromTheLayout() throws {
        let text = #"{"ev":"spaces","spaces":[{"id":"s1","name":"a","cwd":"/a","focus":"p1","root":{"node":"pane","id":"p1","kind":"plugin","plugin":"sample","pane":"counter"}}]}"#
        guard case let .spaces(spaces, _) = try msg(text) else { Issue.record("wrong case"); return }
        let kind = try #require(spaces.first?.root.panes.first?.kind)
        #expect(kind.plugin?.plugin == "sample")
        #expect(kind.plugin?.pane == "counter")
        #expect(PaneKind.empty.plugin == nil)
    }
}
