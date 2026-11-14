import Foundation
import Testing
import WheelKit
@testable import WheelLuau

@Suite struct EmbedFixtureTests {
    @Test func theEmbedFixtureNamesItsSpacesTerminalsAWebViewAndTheFirstAgent() throws {
        let path = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("tools/fixtures/plugins/embed/main.luau")
        let row = { (id: String, space: String, kind: String) -> JSONValue in
            .object(["id": .string(id), "space": .string(space), "kind": .string(kind)])
        }
        let host = PaneRuntime.Host(feed: { name, _ in
            switch name {
            case "wheel.panes":
                PaneRuntime.FeedRead(value: .array([row("p1", "s1", "plugin"), row("p2", "s1", "terminal"),
                                                    row("p3", "s1", "terminal"), row("p4", "s2", "terminal")]))
            case "wheel.agents":
                PaneRuntime.FeedRead(value: .array([.object(["pane": .string("p9"), "space": .string("s2")]),
                                                    .object(["pane": .string("p7"), "space": .string("s1")])]))
            default: PaneRuntime.FeedRead(value: .null)
            }
        })
        let runtime = try PaneRuntime(source: String(contentsOf: path, encoding: .utf8), host: host)
        let data: JSONValue = .object(["space": .string("s1"), "pane": .string("p1")])
        var tree = try runtime.view(data)
        #expect(tree.children[0].children[0].children.count == 2)
        #expect(tree.children[0].children[1].component == "Terminal")
        #expect(tree.children[0].children[1].props["pane"] == .string("p2"))
        #expect(tree.children[1].component == "WebView")
        #expect(tree.children[1].props["url"] == .string("https://example.com"))
        #expect(tree.children[2].component == "AgentChat")
        #expect(tree.children[2].props["pane"] == .string("p7"))
        #expect(tree.children[3].props["text"] == .string("found 2 terminal panes"))
        try runtime.send(.object(["type": .string("select"), "pane": .string("p3")]))
        tree = try runtime.view(data)
        #expect(tree.children[0].children[1].props["pane"] == .string("p3"))
    }
}
