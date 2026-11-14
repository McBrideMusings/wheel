import Foundation
import Testing
import WheelKit
@testable import WheelLuau

/// The Runner pane (`plugins/runner/panes/runner.luau`) run through the pane runtime against fake
/// `runner.commands` and `runner.log` feeds, pressed the way a client presses its buttons.
@Suite struct RunnerPaneTests {
    static let package = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("plugins/runner")

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

    /// Every Button below `node`: its hover text (the `help` modifier) and its action.
    static func buttons(_ node: PaneNode) -> [(help: String, action: JSONValue)] {
        var out: [(String, JSONValue)] = []
        if node.component == "Button", let action = node.props["action"] {
            var help = ""
            if case let .array(mods)? = node.props["modifiers"] {
                for case let .object(m) in mods { if case let .string(h)? = m["help"] { help = h } }
            }
            out.append((help, action))
        }
        for child in node.children { out += buttons(child) }
        return out
    }

    static func pane(feeds: @escaping (String, JSONValue) -> JSONValue, actions: @escaping (String, JSONValue) -> Void = { _, _ in }) throws -> PaneRuntime {
        let host = PaneRuntime.Host(feed: { name, params in PaneRuntime.FeedRead(value: feeds(name, params)) }, action: actions)
        let source = try String(contentsOf: package.appendingPathComponent("panes/runner.luau"), encoding: .utf8)
        return try PaneRuntime(source: source, host: host)
    }

    static let dir = "/p"
    static let data: JSONValue = .object(["pane": .string("p1"), "space": .string("s1"), "cwd": .string(dir)])

    /// Four commands: `build` and `deploy` (asks first, has a status check) at the top, `prototypes.terrarium.dev`
    /// and `machine.hello` in groups. `deploy` ran once and ended with 65, `build` goes now.
    static let commands = """
    {"dir":"/p","now":1000,"runners":[
      {"name":"build","label":"Build","group":[],"desc":"Builds it","command":"make","confirm":null,"origin":"project","worktree":false,"status":false},
      {"name":"deploy","label":"Deploy","group":[],"desc":null,"command":"make deploy","confirm":"Ends every agent.","origin":"project","worktree":false,"status":true},
      {"name":"prototypes.terrarium.dev","label":"dev","group":["prototypes","terrarium"],"desc":null,"command":"bun run dev","confirm":null,"origin":"project","worktree":false,"status":false},
      {"name":"machine.hello","label":"hello","group":["machine"],"desc":null,"command":"echo hi","confirm":null,"origin":"machine","worktree":false,"status":false}],
     "runs":[
      {"id":"r2","name":"build","state":"running","exit_code":null,"started":940,"ended":null},
      {"id":"r1","name":"deploy","state":"exited","exit_code":65,"started":100,"ended":110}],
     "checks":{"deploy":{"up":true,"text":"v42"}}}
    """

    static func log(_ lines: [String], state: String = "running") -> JSONValue {
        let quoted = lines.map { "\"\($0)\"" }.joined(separator: ",")
        return try! json("""
        {"id":"r2","name":"build","state":"\(state)","exit_code":null,"started":940,"ended":null,"lines":[\(quoted)],"error":null}
        """)
    }

    static func press(_ runtime: PaneRuntime, _ tree: PaneNode, help: String) throws -> PaneNode {
        guard let b = buttons(tree).first(where: { $0.help == help }) else {
            throw Failure("no button \"\(help)\" in \(buttons(tree).map(\.help))")
        }
        try runtime.send(b.action)
        return try runtime.view(data)
    }

    struct Failure: Error, CustomStringConvertible {
        var description: String
        init(_ d: String) { description = d }
    }

    @Test func theListDrawsCommandsGroupsTheRunningSectionAndEachRow() throws {
        let commands = try Self.json(Self.commands)
        let runtime = try Self.pane(feeds: { _, _ in commands })
        let tree = try runtime.view(Self.data)
        let all = Self.texts(tree)
        #expect(tree.children.count == 4)
        // The Running section pinned above the list holds the run that goes, with its duration.
        let running = Self.texts(tree.children[2])
        #expect(running == ["Running", "Build", "1:00", "Open", "Stop"])
        // The tree: commands first, then groups with their counts, the machine's commands included.
        #expect(all.contains("Build") && all.contains("Deploy"))
        #expect(all.contains("prototypes  1") && all.contains("terrarium  1") && all.contains("machine  1"))
        #expect(all.contains("dev") && all.contains("hello"))
        // Each row's latest run, and a status check's reading.
        #expect(all.contains("running 1:00"))
        #expect(all.contains("exit 65 · 00:01:40"))
        #expect(all.contains("up · v42"))
        #expect(all.contains("Builds it"))
    }

    @Test func runStartsACommandThroughTheBackend() throws {
        let commands = try Self.json(Self.commands)
        var actions: [(String, JSONValue)] = []
        let runtime = try Self.pane(feeds: { _, _ in commands }, actions: { actions.append(($0, $1)) })
        var tree = try runtime.view(Self.data)
        tree = try Self.press(runtime, tree, help: "Run Build")
        #expect(actions.count == 1 && actions[0].0 == "runner.run")
        #expect(actions[0].1 == .object(["dir": .string("/p"), "name": .string("build")]))
        // The answer clears nothing to show; a guard that failed shows its message in the fixed line.
        try runtime.send(try Self.json(#"{"action":"runner.run","result":{"started":false,"guard":"the tree is dirty"}}"#))
        tree = try runtime.view(Self.data)
        #expect(Self.texts(tree).contains("build did not start: the tree is dirty"))
        // Open reads a run's log.
        tree = try Self.press(runtime, tree, help: "Open the log of Build")
        #expect(Self.texts(tree).contains("Back"))
    }

    @Test func aCommandWithAQuestionAsksInItsRowAndCancelStartsNothing() throws {
        let commands = try Self.json(Self.commands)
        var actions: [(String, JSONValue)] = []
        let runtime = try Self.pane(feeds: { _, _ in commands }, actions: { actions.append(($0, $1)) })
        var tree = try runtime.view(Self.data)
        tree = try Self.press(runtime, tree, help: "Run Deploy")
        #expect(actions.isEmpty, "pressing Run on a command with a question starts nothing")
        #expect(Self.texts(tree).contains("Ends every agent."))
        let helps = Self.buttons(tree).map(\.help)
        #expect(helps.contains("Run Deploy: Ends every agent.") && helps.contains("Do not run Deploy"))
        // Cancel: the question goes, nothing started.
        tree = try Self.press(runtime, tree, help: "Do not run Deploy")
        #expect(actions.isEmpty)
        #expect(!Self.texts(tree).contains("Ends every agent."))
        // Ask again and answer: the run starts with the command text it was asked about.
        tree = try Self.press(runtime, tree, help: "Run Deploy")
        tree = try Self.press(runtime, tree, help: "Run Deploy: Ends every agent.")
        #expect(actions.count == 1 && actions[0].0 == "runner.run")
        #expect(actions[0].1 == .object(["dir": .string("/p"), "name": .string("deploy"), "confirmed": .string("make deploy")]))
        try runtime.send(try Self.json(#"{"action":"runner.run","result":{"started":true,"id":"r3"}}"#))
        tree = try runtime.view(Self.data)
        #expect(!Self.texts(tree).contains("Ends every agent."))
    }

    @Test func searchNarrowsTheListAndFoldingAGroupHidesItsRows() throws {
        let commands = try Self.json(Self.commands)
        let runtime = try Self.pane(feeds: { _, _ in commands })
        var tree = try runtime.view(Self.data)
        // Folding `prototypes` hides terrarium and dev; the other rows stay.
        tree = try Self.press(runtime, tree, help: "Fold prototypes")
        var all = Self.texts(tree)
        #expect(all.contains("prototypes  1") && !all.contains("terrarium  1") && !all.contains("dev"))
        #expect(all.contains("hello") && all.contains("Build"))
        // Opening it again shows them.
        tree = try Self.press(runtime, tree, help: "Open prototypes")
        #expect(Self.texts(tree).contains("dev"))
        // Search narrows to what holds the text, groups with nothing left go, and a fold never hides a match.
        tree = try Self.press(runtime, tree, help: "Fold machine")
        try runtime.send(try Self.json(#"{"type":"query","value":"HELLO"}"#))
        tree = try runtime.view(Self.data)
        // The Running section stays above the list whatever the search says; the list is the fourth child.
        all = Self.texts(tree.children[3])
        #expect(all.contains("hello") && all.contains("machine  1"))
        #expect(!all.contains("Build") && !all.contains("Deploy") && !all.contains("dev") && !all.contains("prototypes  1"))
        try runtime.send(try Self.json(#"{"type":"query","value":"zzz"}"#))
        tree = try runtime.view(Self.data)
        #expect(Self.texts(tree.children[3]) == ["Nothing matches the search."])
    }

    @Test func theLogFillsThePaneFollowsNewLinesAndStops() throws {
        let commands = try Self.json(Self.commands)
        var lines = ["12:00:01  one", "12:00:02  two"]
        var actions: [(String, JSONValue)] = []
        let runtime = try Self.pane(
            feeds: { name, params in
                if name == "runner.log" {
                    #expect(params == .object(["dir": .string("/p"), "id": .string("r2")]))
                    return Self.log(lines)
                }
                return commands
            },
            actions: { actions.append(($0, $1)) })
        var tree = try runtime.view(Self.data)
        tree = try Self.press(runtime, tree, help: "Open the log of Build")
        #expect(tree.children.count == 3 && tree.children[2].component == "List")
        #expect(Self.texts(tree.children[2]) == lines)
        #expect(tree.children[2].props["scrollTo"] == .string("root.2.1"))
        let head = Self.texts(tree.children[0])
        #expect(head.contains("Back") && head.contains("Build") && head.contains("running 1:00") && head.contains("Stop"))
        // The log grows: the view follows its last line.
        lines.append("12:00:03  three")
        tree = try runtime.view(Self.data)
        #expect(Self.texts(tree.children[2]).last == "12:00:03  three")
        #expect(tree.children[2].props["scrollTo"] == .string("root.2.2"))
        // Stop asks the backend to end that run.
        tree = try Self.press(runtime, tree, help: "Stop Build")
        #expect(actions.count == 1 && actions[0].0 == "runner.stop")
        #expect(actions[0].1 == .object(["dir": .string("/p"), "id": .string("r2")]))
        // Back returns to the list.
        tree = try Self.press(runtime, tree, help: "Back to the commands")
        #expect(tree.children.count == 4)
    }

    @Test func theLogOfAStoppedRunShowsItsStateAndNoStop() throws {
        let commands = try Self.json(Self.commands)
        let ended = try Self.json(#"{"id":"r2","name":"build","state":"stopped","exit_code":143,"started":940,"ended":950,"lines":["12:00:01  one"],"error":null}"#)
        let runtime = try Self.pane(feeds: { name, _ in name == "runner.log" ? ended : commands })
        var tree = try runtime.view(Self.data)
        tree = try Self.press(runtime, tree, help: "Open the log of Build")
        let head = Self.texts(tree.children[0])
        #expect(head.contains("stopped") && !head.contains("Stop"))
    }

    @Test func aLogThatCannotBeReadSaysWhy() throws {
        let commands = try Self.json(Self.commands)
        let broken = try Self.json(#"{"id":"r2","name":null,"state":null,"exit_code":null,"started":null,"ended":null,"lines":[],"error":"no such run"}"#)
        let runtime = try Self.pane(feeds: { name, _ in name == "runner.log" ? broken : commands })
        var tree = try runtime.view(Self.data)
        tree = try Self.press(runtime, tree, help: "Open the log of Build")
        #expect(Self.texts(tree.children[2]) == ["no such run"])
    }

    @Test func aBackendErrorShowsInTheFixedLineAndNothingIsReadYetDrawsAPlaceholder() throws {
        let commands = try Self.json(Self.commands)
        let runtime = try Self.pane(feeds: { _, _ in commands })
        var tree = try runtime.view(Self.data)
        tree = try Self.press(runtime, tree, help: "Run Build")
        try runtime.send(try Self.json(#"{"action":"runner.run","error":"run failed: sh: no such file"}"#))
        tree = try runtime.view(Self.data)
        #expect(Self.texts(tree.children[1]) == ["run failed: sh: no such file"])
        let empty = try Self.pane(feeds: { _, _ in .null })
        #expect(Self.texts(try empty.view(Self.data)) == ["Reading the commands…"])
    }
}
