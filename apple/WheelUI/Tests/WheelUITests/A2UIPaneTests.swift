import A2UISwiftCore
import Foundation
import Testing
import WheelKit
@testable import WheelUI

/// The A2UI pane against the replies the daemon sends for the example server
/// (`daemon/examples/mcp_a2ui_test_server.rs`): its tool list, layout resource and tool results.
@MainActor @Suite struct A2UIPaneTests {
    static let layout = """
    [{"version":"v0.9","createSurface":{"surfaceId":"notes","catalogId":"https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"}},
     {"version":"v0.9","updateComponents":{"surfaceId":"notes","components":[
       {"id":"root","component":"Column","children":["title","status","gizmo","save"]},
       {"id":"title","component":"Text","text":"Notes"},
       {"id":"status","component":"Text","text":{"path":"/status"}},
       {"id":"gizmo","component":"Gizmo","size":3},
       {"id":"save","component":"Button","child":"label","action":{"event":{"name":"save_note","context":{"text":{"path":"/draft"}}}}},
       {"id":"label","component":"Text","text":"Add note"}]}}]
    """
    static let first = #"{"version":"v0.9","updateDataModel":{"surfaceId":"notes","path":"/","value":{"draft":"Ship the spike","status":"no notes yet","count":"0 notes"}}}"#
    static let saved = #"[{"version":"v0.9","updateDataModel":{"surfaceId":"notes","path":"/status","value":"saved: Ship the spike"}}]"#

    func frame(_ json: String) throws -> ServerMsg { try ServerMsg.decode(json) }

    func tools(linking uri: String?) throws -> ServerMsg {
        let meta = uri.map { #","meta":{"ui":{"resourceUri":"\#($0)","mimeType":"application/a2ui+json"}}"# } ?? ""
        return try frame(#"{"ev":"tools","req":1,"tools":[{"name":"get_notes","input_schema":{}\#(meta)}]}"#)
    }

    func result(_ text: String) throws -> ServerMsg {
        let escaped = String(decoding: try JSONSerialization.data(withJSONObject: [text], options: [.fragmentsAllowed]), as: UTF8.self).dropFirst().dropLast()
        return try frame(#"{"ev":"tool_result","req":3,"result":{"content":[{"type":"resource","resource":{"uri":"a2ui://notes","mimeType":"application/a2ui+json","text":\#(escaped)}}],"is_error":false}}"#)
    }

    /// A pane wired to a recorder of what it sends, with request ids 1, 2, 3 ...
    func pane() -> (A2UIPane, Recorder) {
        let recorder = Recorder()
        let pane = A2UIPane(pane: "p1", artifact: ArtifactRef(scope: .project, name: "notes"), space: "s1",
                            send: { recorder.sent.append($0) }, nextReq: { recorder.next() })
        return (pane, recorder)
    }

    final class Recorder {
        var sent: [ClientMsg] = []
        private var n: UInt64 = 0
        func next() -> UInt64 { n += 1; return n }
    }

    func value(_ pane: A2UIPane, _ path: String) -> String? {
        pane.surface?.surface.dataModel.get(path)?.stringValue
    }

    @Test func aServerThatLinksALayoutBecomesANativePaneWithItsFirstData() throws {
        let (pane, sent) = pane()
        pane.start()
        #expect(sent.sent == [.artifactTools(req: 1, name: "notes", scope: .project, space: "s1")])
        #expect(pane.handle(try tools(linking: "a2ui://notes")))
        #expect(pane.state == .loading)
        #expect(sent.sent.last == .artifactRead(req: 2, name: "notes", scope: .project, space: "s1", uri: "a2ui://notes"))

        let escaped = String(decoding: try JSONSerialization.data(withJSONObject: [Self.layout]), as: UTF8.self).dropFirst().dropLast()
        #expect(pane.handle(try frame(#"{"ev":"resource","req":2,"contents":[{"uri":"a2ui://notes","mimeType":"application/a2ui+json","text":\#(escaped)}]}"#)))
        #expect(sent.sent.last == .artifactCall(req: 3, name: "notes", scope: .project, space: "s1", tool: "get_notes", arguments: nil))

        #expect(pane.handle(try result(Self.first)))
        #expect(pane.state == .ready)
        #expect(value(pane, "/status") == "no notes yet")
        #expect(value(pane, "/draft") == "Ship the spike")
    }

    @Test func aComponentOutsideTheCatalogIsKeptAsAnUnsupportedPlaceholder() throws {
        let names = A2UIPane.catalogNames
        let decoded = try A2UIMessages.decode(text: Self.layout, catalog: names)
        #expect(decoded.unsupported == ["Gizmo"])
        let (pane, _) = pane()
        pane.start()
        _ = pane.handle(try tools(linking: "a2ui://notes"))
        let escaped = String(decoding: try JSONSerialization.data(withJSONObject: [Self.layout]), as: UTF8.self).dropFirst().dropLast()
        _ = pane.handle(try frame(#"{"ev":"resource","req":2,"contents":[{"text":\#(escaped)}]}"#))
        #expect(pane.unsupported == ["Gizmo"])
        let root = pane.surface?.componentTree
        #expect(root?.children.count == 4)
    }

    @Test func aButtonActionCallsTheNamedToolAndAStreamedUpdateChangesOnlyItsPath() throws {
        let (pane, sent) = pane()
        pane.start()
        _ = pane.handle(try tools(linking: "a2ui://notes"))
        let escaped = String(decoding: try JSONSerialization.data(withJSONObject: [Self.layout]), as: UTF8.self).dropFirst().dropLast()
        _ = pane.handle(try frame(#"{"ev":"resource","req":2,"contents":[{"text":\#(escaped)}]}"#))
        _ = pane.handle(try result(Self.first))

        pane.surface?.surface.dispatchAction(name: "save_note", sourceComponentId: "save", context: ["text": .string("Ship the spike")])
        guard case let .artifactCall(req, _, _, _, tool, arguments)? = sent.sent.last else { Issue.record("no call sent"); return }
        #expect(tool == "save_note")
        #expect(arguments == #"{"text":"Ship the spike"}"#)

        let escapedSaved = String(decoding: try JSONSerialization.data(withJSONObject: [Self.saved]), as: UTF8.self).dropFirst().dropLast()
        #expect(pane.handle(try frame(#"{"ev":"tool_result","req":\#(req ?? 0),"result":{"content":[{"type":"resource","resource":{"mimeType":"application/a2ui+json","text":\#(escapedSaved)}}],"is_error":false}}"#)))
        #expect(value(pane, "/status") == "saved: Ship the spike")
        #expect(value(pane, "/count") == "0 notes")
        #expect(value(pane, "/draft") == "Ship the spike")
    }

    @Test func aFailedActionIsShownAndTheSurfaceStays() throws {
        let (pane, sent) = pane()
        pane.start()
        _ = pane.handle(try tools(linking: "a2ui://notes"))
        let escaped = String(decoding: try JSONSerialization.data(withJSONObject: [Self.layout]), as: UTF8.self).dropFirst().dropLast()
        _ = pane.handle(try frame(#"{"ev":"resource","req":2,"contents":[{"text":\#(escaped)}]}"#))
        _ = pane.handle(try result(Self.first))
        pane.surface?.surface.dispatchAction(name: "save_note", sourceComponentId: "save")
        guard case let .artifactCall(req, _, _, _, _, _)? = sent.sent.last else { Issue.record("no call sent"); return }
        _ = pane.handle(try frame(#"{"ev":"error","req":\#(req ?? 0),"message":"the MCP server closed"}"#))
        #expect(pane.notice == "save_note: the MCP server closed")
        #expect(pane.state == .ready)
        #expect(pane.surface != nil)
    }

    @Test func anArtifactWithNoLinkedLayoutStaysAWebPane() throws {
        let (pane, _) = pane()
        pane.start()
        _ = pane.handle(try tools(linking: nil))
        #expect(pane.state == .web)
        let (other, _) = self.pane()
        other.start()
        _ = other.handle(try frame(#"{"ev":"error","req":1,"message":"the entry names no MCP server"}"#))
        #expect(other.state == .web)
    }

    @Test func theLinkOfAToolIsRecognisedByItsMimeTypeOrItsScheme() throws {
        let ui = try ServerMsg.decode(#"{"ev":"tools","req":1,"tools":[{"name":"app","input_schema":{},"meta":{"ui":{"resourceUri":"ui://x/view"}}},{"name":"a","input_schema":{},"meta":{"ui":{"resourceUri":"a2ui://x","mimeType":"application/json+a2ui"}}}]}"#)
        guard case let .tools(_, list) = ui else { Issue.record("not tools"); return }
        #expect(A2UIMessages.link(in: list) == A2UIMessages.Link(tool: "a", uri: "a2ui://x"))
    }
}
