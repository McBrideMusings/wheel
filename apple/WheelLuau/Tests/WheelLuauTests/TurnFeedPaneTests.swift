import Foundation
import Testing
import WheelKit
@testable import WheelLuau

/// The Turn feed pane (`plugins/turn-feed/panes/feed.luau`) run through the pane runtime against fake
/// `turn-feed.posts` and `wheel.agents` feeds, pressed the way a client presses its buttons.
@Suite struct TurnFeedPaneTests {
    static let package = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("plugins/turn-feed")

    static func json(_ text: String) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
    }

    static func texts(_ node: PaneNode) -> [String] {
        var out: [String] = []
        if case let .string(t)? = node.props["text"] { out.append(t) }
        if let child = node.props["child"], let n = PaneNode(child) { out += texts(n) }
        for child in node.children { out += texts(child) }
        return out
    }

    /// Every node of `component` below `node`.
    static func nodes(_ node: PaneNode, _ component: String) -> [PaneNode] {
        var out = node.component == component ? [node] : []
        if let child = node.props["child"], let n = PaneNode(child) { out += nodes(n, component) }
        for child in node.children { out += nodes(child, component) }
        return out
    }

    /// Every Button below `node`: its hover text, its action and whether it is disabled.
    static func buttons(_ node: PaneNode) -> [(help: String, action: JSONValue, disabled: Bool)] {
        var out: [(String, JSONValue, Bool)] = []
        if node.component == "Button", let action = node.props["action"] {
            var help = "", disabled = false
            if case let .array(mods)? = node.props["modifiers"] {
                for case let .object(m) in mods {
                    if case let .string(h)? = m["help"] { help = h }
                    if case .bool(true)? = m["disabled"] { disabled = true }
                }
            }
            out.append((help, action, disabled))
        }
        for child in node.children { out += buttons(child) }
        return out
    }

    static func pane(posts: String, agents: String, actions: @escaping (String, JSONValue) -> Void = { _, _ in }) throws -> PaneRuntime {
        let p = try json(posts), a = try json(agents)
        let host = PaneRuntime.Host(feed: { name, _ in PaneRuntime.FeedRead(value: name == "turn-feed.posts" ? p : a) }, action: actions)
        let source = try String(contentsOf: package.appendingPathComponent("panes/feed.luau"), encoding: .utf8)
        return try PaneRuntime(source: source, host: host)
    }

    static let data: JSONValue = .object(["pane": .string("p5"), "space": .string("s1"), "cwd": .string("/p")])

    static let open = """
    {"dir":"/p","posts":[
      {"post":"t2","agent_pane":"p1","space":"s1","dir":"/p","at":"2026-11-16T16:40:09.123Z","title":"Which one?","body":"First **bold**.\\n\\nSecond.",
       "question":"1. Which?","options":[
         {"id":"1A","action":"do a","case":"c1","cost":"k1","checked":"x1","pick":true},
         {"id":"1B","action":"do b","case":"c2","cost":"k2","checked":"x2"}],"answer":null}]}
    """
    static let agents = #"[{"pane":"p1","space":"s1","kind":"agent","agent":"claude-1","status":"idle"}]"#

    @Test func aPostDrawsItsOptionsAndAPickGoesToTheAgent() throws {
        var calls: [(String, JSONValue)] = []
        let runtime = try Self.pane(posts: Self.open, agents: Self.agents, actions: { calls.append(($0, $1)) })
        var tree = try runtime.view(Self.data)
        let all = Self.texts(tree)
        #expect(all.contains("Which one?") && all.contains("16:40 UTC · p1") && all.contains("my pick"))
        let body = try #require(Self.nodes(tree, "StructuredText").first)
        #expect(body.props["markdown"] == .string("First **bold**.\n\nSecond."))
        #expect(all.contains("Case: c1") && all.contains("Cost: k2") && all.contains("Checked: x1"))
        let pick = try #require(Self.buttons(tree).first { $0.help == "Pick 1B" })
        #expect(!pick.disabled)
        try runtime.send(pick.action)
        #expect(calls.count == 1 && calls[0].0 == "turn-feed.answer")
        #expect(calls[0].1 == .object(["post": .string("t2"), "pick": .string("1B")]))
        try runtime.send(try Self.json(#"{"action":"turn-feed.answer","result":{"post":"t2","agent_pane":"p1","text":"Turn feed, \"Which one?\": picked 1B"}}"#))
        #expect(calls.count == 2 && calls[1].0 == "wheel.agent.send")
        #expect(calls[1].1 == .object(["agent": .string("p1"), "text": .string("Turn feed, \"Which one?\": picked 1B")]))
        try runtime.send(try Self.json(#"{"action":"wheel.agent.send","error":"no agent p1"}"#))
        tree = try runtime.view(Self.data)
        #expect(Self.texts(tree).contains("Answer saved, but the agent did not get it: no agent p1"))
    }

    @Test func aReplySendsTheDraftAndAnAnswerLocksTheControls() throws {
        var calls: [(String, JSONValue)] = []
        let runtime = try Self.pane(posts: Self.open, agents: Self.agents, actions: { calls.append(($0, $1)) })
        var tree = try runtime.view(Self.data)
        let send = try #require(Self.buttons(tree).first { $0.help == "Send the reply" })
        try runtime.send(send.action)
        #expect(calls.isEmpty)
        try runtime.send(try Self.json(#"{"type":"draft","post":"t2","value":"use C"}"#))
        try runtime.send(send.action)
        #expect(calls.count == 1 && calls[0].1 == .object(["post": .string("t2"), "reply": .string("use C")]))
        try runtime.send(try Self.json(#"{"action":"turn-feed.answer","error":"answer refused: post t2 was already answered: 1A at x"}"#))
        tree = try runtime.view(Self.data)
        #expect(Self.texts(tree).contains("answer refused: post t2 was already answered: 1A at x"))

        let answered = Self.open.replacingOccurrences(of: #""answer":null"#, with: #""answer":{"pick":"1A","at":"2026-11-16T16:41:00.000Z","by_pane":"p5"}"#)
        let locked = try Self.pane(posts: answered, agents: Self.agents)
        tree = try locked.view(Self.data)
        #expect(Self.texts(tree).contains("Picked 1A · 16:41 UTC"))
        let buttons = Self.buttons(tree)
        #expect(buttons.count == 3 && buttons.allSatisfy { $0.disabled })
    }

    @Test func aPostsPageIsTheArtifactEmbedded() throws {
        let paged = Self.open.replacingOccurrences(of: #""question":"1. Which?""#, with: #""page":"mockup","question":"1. Which?""#)
        let tree = try Self.pane(posts: paged, agents: Self.agents).view(Self.data)
        let web = try #require(Self.nodes(tree, "WebView").first)
        #expect(web.props["artifact"] == .string("mockup") && web.props["height"] == .number(360))
    }

    @Test func aGoneAgentDisablesTheButtonsAndAnEmptyFeedSaysHowToPost() throws {
        let gone = try Self.pane(posts: Self.open, agents: "[]")
        var tree = try gone.view(Self.data)
        #expect(Self.texts(tree).contains("Agent pane p1 is gone"))
        #expect(Self.buttons(tree).allSatisfy { $0.disabled })
        let empty = try Self.pane(posts: #"{"dir":"/p","posts":[]}"#, agents: "[]")
        tree = try empty.view(Self.data)
        #expect(Self.texts(tree).first?.hasPrefix("No posts yet for /p. Agents post with: wheel plugin action") == true)
    }
}
