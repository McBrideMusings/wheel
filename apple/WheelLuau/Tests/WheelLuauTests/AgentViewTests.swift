import Foundation
import Testing
import WheelKit
@testable import WheelLuau

/// The built-in agent view (`daemon/plugins/agent`) run through the pane runtime against fake feeds.
@Suite struct AgentViewTests {
    static let package = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("daemon/plugins/agent")

    static func modules() throws -> [String: String] {
        var out: [String: String] = [:]
        let lib = package.appendingPathComponent("lib")
        for name in try FileManager.default.contentsOfDirectory(atPath: lib.path) where name.hasSuffix(".luau") {
            out["lib/\(name)"] = try String(contentsOf: lib.appendingPathComponent(name), encoding: .utf8)
        }
        return out
    }

    static func json(_ text: String) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
    }

    static func texts(_ node: PaneNode) -> [String] {
        var out: [String] = []
        if case let .string(t)? = node.props["text"] { out.append(t) }
        for child in node.children { out += texts(child) }
        return out
    }

    static let transcript = """
    {"agent":"claude-1","events":[
      {"kind":"turn_start","turn":1},
      {"kind":"user","turn":1,"text":"fix the bug"},
      {"kind":"text","text":"On it."},
      {"kind":"thought","text":"look at a.rs first"},
      {"kind":"tool_call","id":"c1","title":"Edit a.rs","tool_kind":"edit","status":"pending","content":[]},
      {"kind":"tool_update","id":"c1","status":"completed","content":[
        {"type":"diff","path":"a.rs","old_text":"one\\ntwo\\nthree","new_text":"one\\n2\\nthree"},
        {"type":"text","text":"ok"}]},
      {"kind":"plan","entries":[{"content":"write test","status":"pending"},{"content":"fix","status":"completed"}]},
      {"kind":"error","message":"boom"},
      {"kind":"turn_end","turn":1,"stop_reason":"end_turn"}
    ]}
    """

    @Test func theAgentViewDrawsEveryEventKindAndSendsTheDraft() throws {
        var actions: [(String, JSONValue)] = []
        let events = try Self.json(Self.transcript)
        let status = try Self.json(#"{"agent":"claude-1","state":{"phase":"busy","queued":2}}"#)
        let requests = try Self.json(#"{"agent":"claude-1","requests":[{"id":"r1","agent":"claude-1","title":"Edit a.rs","tool_call":"c1","options":[]}]}"#)
        let host = PaneRuntime.Host(
            feed: { name, params in
                #expect(params == .object(["agent": .string("p3")]))
                switch name {
                case "wheel.agent.transcript": return PaneRuntime.FeedRead(value: events)
                case "wheel.agent.status": return PaneRuntime.FeedRead(value: status)
                case "wheel.agent.requests": return PaneRuntime.FeedRead(value: requests)
                default: return PaneRuntime.FeedRead(value: .null)
                }
            },
            action: { name, args in actions.append((name, args)) })
        let source = try String(contentsOf: Self.package.appendingPathComponent("main.luau"), encoding: .utf8)
        let runtime = try PaneRuntime(source: source, modules: Self.modules(), host: host)
        let data: JSONValue = .object(["pane": .string("p3"), "space": .string("s1")])
        var tree = try runtime.view(data)
        #expect(tree.children.count == 4)
        let list = tree.children[1]
        #expect(list.component == "List")
        // user, text, thought, tool card, plan, error
        #expect(list.children.count == 6)
        #expect(list.props["scrollTo"] == .string("root.1.5"))
        let all = Self.texts(list)
        #expect(all.contains("fix the bug"))
        #expect(all.contains("On it."))
        #expect(all.contains("Waiting for permission"))
        #expect(all.contains("- two"))
        #expect(all.contains("+ 2"))
        #expect(all.contains("☑ fix"))
        #expect(all.contains("boom"))
        let header = Self.texts(tree.children[0])
        #expect(header.contains("p3") && header.contains("busy") && header.contains("2 queued"))

        try runtime.send(.object(["type": .string("draft"), "value": .string("hi")]))
        try runtime.send(.object(["type": .string("send")]))
        #expect(actions.count == 1)
        #expect(actions[0].0 == "wheel.agent.send")
        #expect(actions[0].1 == .object(["agent": .string("p3"), "text": .string("hi")]))
        tree = try runtime.view(data)
        #expect(tree.children.count == 4)
        // the send's result clears the sending note; an error shows in the fixed line
        try runtime.send(.object(["action": .string("wheel.agent.send"), "error": .string("agent is gone")]))
        tree = try runtime.view(data)
        #expect(Self.texts(tree.children[3]) == ["agent is gone"])
        // an empty draft sends nothing
        try runtime.send(.object(["type": .string("draft"), "value": .string("   ")]))
        try runtime.send(.object(["type": .string("send")]))
        #expect(actions.count == 1)
    }

    @Test func anEmptyTranscriptDrawsOneRow() throws {
        let host = PaneRuntime.Host(feed: { _, _ in PaneRuntime.FeedRead(value: .null) })
        let source = try String(contentsOf: Self.package.appendingPathComponent("main.luau"), encoding: .utf8)
        let runtime = try PaneRuntime(source: source, modules: Self.modules(), host: host)
        let tree = try runtime.view(.object(["pane": .string("p3")]))
        #expect(tree.children[1].children.count == 1)
        #expect(Self.texts(tree.children[1]) == ["Give this agent a task."])
        #expect(tree.children[1].props["scrollTo"] == .string("root.1.0"))
    }

    @Test func aLargeDiffIsNotDiffedAndALongTextIsCapped() throws {
        let big = (1...401).map(String.init).joined(separator: "\\n")
        let long = (1...30).map { "line \($0)" }.joined(separator: "\\n")
        let events = try Self.json("""
        {"agent":"a","events":[
          {"kind":"tool_call","id":"c","title":"t","tool_kind":"edit","status":"completed","content":[
            {"type":"diff","path":"big.txt","old_text":"\(big)","new_text":"x"},
            {"type":"text","text":"\(long)"}]}]}
        """)
        let host = PaneRuntime.Host(feed: { name, _ in
            PaneRuntime.FeedRead(value: name == "wheel.agent.transcript" ? events : .null)
        })
        let source = try String(contentsOf: Self.package.appendingPathComponent("main.luau"), encoding: .utf8)
        let runtime = try PaneRuntime(source: source, modules: Self.modules(), host: host)
        let all = Self.texts(try runtime.view(.object(["pane": .string("p3")])).children[1])
        #expect(all.contains("diff too large: 401 lines"))
        #expect(all.contains { $0.hasSuffix("… 10 more lines") && $0.hasPrefix("line 1\n") })
    }

    @Test func aLongSessionOfToolCardsStaysInsideTheValueLimit() throws {
        // 300 cards of 12 diffs, each changing every other line of 60, so each diff draws about 60 runs: far over the
        // runtime's 100,000 values if every row were drawn.
        let old = (1...60).map(String.init).joined(separator: "\n")
        let new = (1...60).map { $0 % 2 == 0 ? "x\($0)" : String($0) }.joined(separator: "\n")
        let diff: [String: Any] = ["type": "diff", "path": "f", "old_text": old, "new_text": new]
        let cards: [[String: Any]] = (1...300).map {
            ["kind": "tool_call", "id": "c\($0)", "title": "t\($0)", "tool_kind": "edit", "status": "completed",
             "content": Array(repeating: diff, count: 12)]
        }
        let data = try JSONSerialization.data(withJSONObject: ["agent": "a", "events": cards])
        let events = try JSONDecoder().decode(JSONValue.self, from: data)
        let host = PaneRuntime.Host(feed: { name, _ in
            PaneRuntime.FeedRead(value: name == "wheel.agent.transcript" ? events : .null)
        })
        let source = try String(contentsOf: Self.package.appendingPathComponent("main.luau"), encoding: .utf8)
        let runtime = try PaneRuntime(source: source, modules: Self.modules(), host: host)
        let list = try runtime.view(.object(["pane": .string("p3")])).children[1]
        #expect(list.children.count > 1 && list.children.count < 300)
        #expect(Self.texts(list.children.last!).contains("t300"))
        #expect(Self.texts(list).contains("… 4 more items"))
    }
}
