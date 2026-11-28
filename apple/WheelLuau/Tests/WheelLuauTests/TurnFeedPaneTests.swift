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
      {"post":"t2","agent_pane":"p1","space":"s1","dir":"/p","at":"2026-11-16T16:40:09.123Z","at_local":"4:40pm","title":"Which one?","body":"First **bold**.\\n\\nSecond.",
       "body_blocks":[{"kind":"paragraph","text":"First **bold**."},{"kind":"paragraph","text":"Second."}],
       "question":"1. Which?","options":[
         {"id":"1A","action":"do a","case":"c1","cost":"k1","checked":"x1","pick":true},
         {"id":"1B","action":"do b","case":"c2","cost":"k2","checked":"x2"}],"answer":null}]}
    """
    static let agents = #"[{"pane":"p1","space":"s1","kind":"agent","agent":"claude-1","status":"idle"}]"#
    /// The two icon buttons every card's header has, whatever the post asks.
    static let itemActions: Set<String> = ["Delete this post", "Show Agent p1's pane"]
    static func answering(_ tree: PaneNode) -> [(help: String, action: JSONValue, disabled: Bool)] { buttons(tree).filter { !itemActions.contains($0.help) } }

    @Test func aPostDrawsItsOptionsAndAPickGoesToTheAgent() throws {
        var calls: [(String, JSONValue)] = []
        let runtime = try Self.pane(posts: Self.open, agents: Self.agents, actions: { calls.append(($0, $1)) })
        var tree = try runtime.view(Self.data)
        let all = Self.texts(tree)
        #expect(all.contains("Which one?") && all.contains("Agent p1") && all.contains("4:40pm") && all.contains("1A · my pick"))
        #expect(all.contains("First **bold**.") && all.contains("Second."))
        #expect(Self.nodes(tree, "StructuredText").isEmpty)
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

        let answered = Self.open.replacingOccurrences(of: #""answer":null"#, with: #""answer":{"pick":"1A","at":"2026-11-16T16:41:00.000Z","at_local":"4:41pm","by_pane":"p5"}"#)
        let locked = try Self.pane(posts: answered, agents: Self.agents)
        tree = try locked.view(Self.data)
        #expect(Self.texts(tree).contains("Picked 1A · 4:41pm"))
        let buttons = Self.answering(tree)
        #expect(buttons.count == 2 && buttons.allSatisfy { $0.disabled })
        #expect(Self.buttons(tree).filter { Self.itemActions.contains($0.help) }.allSatisfy { !$0.disabled })
    }

    @Test func anUndeliveredAnswerCanBeSentAgainAndADeliveredOneCannot() throws {
        var calls: [(String, JSONValue)] = []
        let stored = #""answer":{"pick":"1A","at":"2026-11-16T16:41:00.000Z","at_local":"4:41pm","by_pane":"p5","text":"Turn feed: picked 1A","delivered":false}"#
        let undelivered = Self.open.replacingOccurrences(of: #""answer":null"#, with: stored)
        let runtime = try Self.pane(posts: undelivered, agents: Self.agents, actions: { calls.append(($0, $1)) })
        var tree = try runtime.view(Self.data)
        #expect(Self.texts(tree).contains("Saved, but the agent did not get it"))
        #expect(!Self.buttons(tree).contains { $0.help == "Send the reply" })
        let again = try #require(Self.buttons(tree).first { $0.help == "Send the saved answer to the agent again" })
        #expect(!again.disabled)
        #expect(Self.texts(tree).contains("Send again"))
        #expect(Self.answering(tree).filter { !$0.disabled }.count == 1)
        try runtime.send(again.action)
        #expect(calls.count == 1 && calls[0].0 == "wheel.agent.send")
        #expect(calls[0].1 == .object(["agent": .string("p1"), "text": .string("Turn feed: picked 1A")]))
        try runtime.send(try Self.json(#"{"action":"wheel.agent.send","result":{}}"#))
        #expect(calls.count == 2 && calls[1].0 == "turn-feed.delivered")
        #expect(calls[1].1 == .object(["post": .string("t2")]))

        let gone = try Self.pane(posts: undelivered, agents: "[]").view(Self.data)
        #expect(Self.answering(gone).allSatisfy { $0.disabled })

        let done = Self.open.replacingOccurrences(of: #""answer":null"#, with: stored.replacingOccurrences(of: "false", with: "true"))
        tree = try Self.pane(posts: done, agents: Self.agents).view(Self.data)
        #expect(Self.texts(tree).contains("Picked 1A · 4:41pm"))
        #expect(!Self.buttons(tree).contains { $0.help == "Send the reply" } && Self.answering(tree).allSatisfy { $0.disabled })
    }

    @Test func aMarkdownPageIsDrawnFromItsBlocksWithImagesSizedByThePage() throws {
        let blocks = #""page":"notes","page_blocks":[{"kind":"heading","level":1,"text":"Hi"},{"kind":"image","url":"file:///p/a.png","alt":"A","width":1594,"height":800},{"kind":"table","align":["none"],"header":["H"],"rows":[["1"]]},{"kind":"code","lang":"","text":"let x"}],"question":"1. Which?""#
        let paged = Self.open.replacingOccurrences(of: #""question":"1. Which?""#, with: blocks)
        let tree = try Self.pane(posts: paged, agents: Self.agents).view(Self.data)
        #expect(Self.nodes(tree, "WebView").isEmpty && Self.nodes(tree, "StructuredText").isEmpty)
        let image = try #require(Self.nodes(tree, "Image").first)
        #expect(image.props["url"] == .string("file:///p/a.png"))
        var maxWidth: JSONValue?
        if case let .array(mods)? = image.props["modifiers"] {
            for case let .object(m) in mods { if case let .object(f)? = m["frame"] { maxWidth = f["maxWidth"] } }
        }
        #expect(maxWidth == .number(1594))
        #expect(Self.nodes(tree, "Grid").count == 1 && Self.texts(tree).contains("let x") && Self.texts(tree).contains("Hi"))
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
        #expect(Self.answering(tree).allSatisfy { $0.disabled })
        let show = try #require(Self.buttons(tree).first { $0.help == "Show Agent p1's pane" })
        #expect(show.disabled)
        let empty = try Self.pane(posts: #"{"dir":"/p","posts":[]}"#, agents: "[]")
        tree = try empty.view(Self.data)
        #expect(Self.texts(tree).first?.hasPrefix("No posts yet from this space's agents. Agents post with: wheel plugin action") == true)
    }

    @Test func theListDrawsThirtyCardsAndShowOlderAddsThirtyMore() throws {
        let posts = (1...40).map { i in
            #"{"post":"t\#(i)","agent_pane":"p1","space":"s1","dir":"/p","at":"2026-11-16T16:40:09.123Z","at_local":"4:40pm","title":"Post \#(i)","body":"b","body_blocks":[],"answer":null}"#
        }
        let runtime = try Self.pane(posts: #"{"dir":"/p","posts":["# + posts.joined(separator: ",") + "]}", agents: Self.agents)
        var tree = try runtime.view(Self.data)
        var titles = Self.texts(tree).filter { $0.hasPrefix("Post ") }
        #expect(titles.count == 30 && titles.first == "Post 1" && titles.last == "Post 30")
        let older = try #require(Self.buttons(tree).first { $0.help == "Show 10 older of 10 not shown" })
        try runtime.send(older.action)
        tree = try runtime.view(Self.data)
        titles = Self.texts(tree).filter { $0.hasPrefix("Post ") }
        #expect(titles.count == 40 && !Self.buttons(tree).contains { $0.help.hasSuffix(" not shown") })
        try runtime.send(try Self.json(#"{"type":"query","value":""}"#))
        #expect(Self.texts(try runtime.view(Self.data)).filter { $0.hasPrefix("Post ") }.count == 30)
    }
}
