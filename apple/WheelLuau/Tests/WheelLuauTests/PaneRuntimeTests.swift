import Testing
import WheelKit
@testable import WheelLuau

private let counter = """
local function init(previous)
    return previous or { count = 0 }
end
local function update(state, msg)
    if msg.type == "inc" then state.count += msg.by or 1 end
    return state
end
local function view(state, data)
    return ui.VStack{
        ui.Text("Count " .. state.count),
        ui.Button{ label = "Add", action = { type = "inc" } },
    }
end
return { init = init, update = update, view = view }
"""

private func text(_ node: PaneNode) -> String? {
    if case let .string(s)? = node.props["text"] { s } else { nil }
}

@Suite struct PaneRuntimeTests {
    @Test func counterIncrementsOnAMessageAndViewShowsTheCount() throws {
        let pane = try PaneRuntime(source: counter)
        #expect(text(try pane.view().children[0]) == "Count 0")
        try pane.send(.object(["type": .string("inc")]))
        try pane.send(.object(["type": .string("inc"), "by": .number(2)]))
        let tree = try pane.view()
        #expect(tree.component == "VStack")
        #expect(tree.children.map(\.component) == ["Text", "Button"])
        #expect(text(tree.children[0]) == "Count 3")
        #expect(tree.children[1].props["action"] == .object(["type": .string("inc")]))
    }

    @Test func reloadKeepsTheStateThroughInitPrevious() throws {
        let pane = try PaneRuntime(source: counter)
        try pane.send(.object(["type": .string("inc")]))
        try pane.send(.object(["type": .string("inc")]))
        try pane.reload(source: counter.replacingOccurrences(of: "\"Count \"", with: "\"Total \""))
        #expect(text(try pane.view().children[0]) == "Total 2")
        #expect(try pane.state == .object(["count": .number(2)]))
    }

    @Test func anInfiniteLoopIsStoppedByTheStepBudget() throws {
        let pane = try PaneRuntime(source: """
        return {
            update = function(state, msg)
                while true do
                    pcall(function() while true do end end)
                end
            end,
            view = function() return ui.Text("alive") end,
        }
        """, limits: .init(steps: 100_000))
        #expect {
            try pane.send(.null)
        } throws: { error in
            guard case let .stepBudget(message)? = error as? PaneRuntime.Failure else { return false }
            return message.contains("step budget")
        }
        #expect(text(try pane.view()) == "alive")
    }

    @Test func allocatingPastTheMemoryLimitIsStopped() throws {
        let pane = try PaneRuntime(source: """
        return {
            update = function(state, msg)
                local t = {}
                for i = 1, 10000000 do t[i] = string.rep("x", 64) .. i end
            end,
            view = function() return ui.Text("alive") end,
        }
        """, limits: .init(memory: 4 << 20, steps: 100_000_000))
        #expect(throws: PaneRuntime.Failure.memory("the script ran past its memory limit of 4194304 bytes")) {
            try pane.send(.null)
        }
        #expect(pane.memoryUsed < 4 << 20)
        #expect(text(try pane.view()) == "alive")
    }

    @Test func ioOsAndRequireAreAbsent() throws {
        let pane = try PaneRuntime(source: """
        return {
            view = function()
                return ui.Text(table.concat({ type(io), type(os), type(require), type(getfenv and getfenv().os) }, " "))
            end,
        }
        """)
        #expect(text(try pane.view()) == "nil nil nil nil")
    }

    @Test func globalsAndLibrariesAreFrozen() throws {
        let pane = try PaneRuntime(source: """
        return {
            view = function()
                local ok = pcall(function() string.rep = nil end)
                local uiOk = pcall(function() ui.Text = 1 end)
                return ui.Text(tostring(ok) .. " " .. tostring(uiOk))
            end,
        }
        """)
        #expect(text(try pane.view()) == "false false")
    }

    @Test func aSyntaxErrorOnReloadLeavesThePreviousModuleRunning() throws {
        let pane = try PaneRuntime(source: counter)
        try pane.send(.object(["type": .string("inc")]))
        #expect {
            try pane.reload(source: "return { view = function( }")
        } throws: { error in
            guard case .syntax? = error as? PaneRuntime.Failure else { return false }
            return true
        }
        #expect(text(try pane.view().children[0]) == "Count 1")
        try pane.send(.object(["type": .string("inc")]))
        #expect(text(try pane.view().children[0]) == "Count 2")
    }

    @Test func aRuntimeErrorInInitOnReloadLeavesThePreviousModuleRunning() throws {
        let pane = try PaneRuntime(source: counter)
        #expect(throws: PaneRuntime.Failure.self) {
            try pane.reload(source: "return { init = function() error('boom') end, view = function() return ui.Text('new') end }")
        }
        #expect(text(try pane.view().children[0]) == "Count 0")
    }

    @Test func aModuleWithoutViewIsRefused() {
        #expect(throws: PaneRuntime.Failure.module("the module has no view function")) {
            try PaneRuntime(source: "return { init = function() return {} end }")
        }
    }

    @Test func viewReturningANonTreeIsAnError() throws {
        let pane = try PaneRuntime(source: "return { view = function() return 42 end }")
        #expect(throws: PaneRuntime.Failure.view("view returned something that is not a component tree")) {
            try pane.view()
        }
    }

    @Test func feedAndActionReachTheHost() throws {
        var actions: [JSONValue] = []
        let host = PaneRuntime.Host(
            feed: { name, params in
                PaneRuntime.FeedRead(value: name == "build" ? .object(["progress": .number(0.5), "target": params]) : .null)
            },
            action: { name, args in actions.append(.array([.string(name), args])) })
        let pane = try PaneRuntime(source: """
        return {
            update = function(state, msg) wheel.action("rebuild", { target = msg.target }) end,
            view = function(state, data)
                local build = data.feed("build", "wheeld")
                return ui.ProgressView{ value = build.progress, label = build.target .. " " .. data.title }
            end,
        }
        """, host: host)
        let tree = try pane.view(.object(["title": .string("Build")]))
        #expect(tree.props["value"] == .number(0.5))
        #expect(tree.props["label"] == .string("wheeld Build"))
        try pane.send(.object(["target": .string("wheeld")]))
        #expect(actions == [.array([.string("rebuild"), .object(["target": .string("wheeld")])])])
    }

    @Test func aHostCallFromACoroutineReadsTheCoroutinesArguments() throws {
        let host = PaneRuntime.Host(feed: { name, params in PaneRuntime.FeedRead(value: .string("\(name):\(params)")) })
        let pane = try PaneRuntime(source: """
        return { view = function(state, data)
            local co = coroutine.wrap(function(feedName) return data.feed(feedName, true) end)
            return ui.Text(co("log"))
        end }
        """, host: host)
        #expect(text(try pane.view()) == "log:bool(true)")
    }

    @Test func aFeedReadReturnsItsValueAndWhetherItIsStale() throws {
        let host = PaneRuntime.Host(feed: { name, _ in
            PaneRuntime.FeedRead(value: .string(name), stale: name == "down")
        })
        let pane = try PaneRuntime(source: """
        return { view = function(state, data)
            local up, upStale = data.feed("up")
            local down, downStale = data.feed("down")
            return ui.Text(up .. tostring(upStale) .. " " .. down .. tostring(downStale))
        end }
        """, host: host)
        #expect(text(try pane.view()) == "upfalse downtrue")
    }

    @Test func aHostCallWithoutANameIsAScriptError() throws {
        let pane = try PaneRuntime(source: "return { view = function(state, data) return ui.Text(data.feed(1)) end }")
        #expect(throws: PaneRuntime.Failure.self) { try pane.view() }
    }
}

@Suite struct PaneNodeTests {
    @Test func flattensToA2UIComponentsWithIDs() throws {
        let pane = try PaneRuntime(source: """
        return { view = function()
            return ui.VStack{
                modifiers = { { padding = 12 } },
                "Hello",
                ui.Button{ child = ui.Text("Go"), action = { type = "go" } },
            }
        end }
        """)
        let components = try pane.view().a2uiComponents()
        #expect(components == [
            .object(["id": .string("root"), "component": .string("VStack"),
                     "modifiers": .array([.object(["padding": .number(12)])]),
                     "children": .array([.string("root.0"), .string("root.1")])]),
            .object(["id": .string("root.0"), "component": .string("Text"), "text": .string("Hello")]),
            .object(["id": .string("root.1"), "component": .string("Button"),
                     "action": .object(["type": .string("go")]), "child": .string("root.1@child")]),
            .object(["id": .string("root.1@child"), "component": .string("Text"), "text": .string("Go")]),
        ])
    }

    @Test func generatedIDsWinOverScriptProps() throws {
        let pane = try PaneRuntime(source: """
        return { view = function() return ui.VStack{ ui.Text{ id = "root", text = "a" } } end }
        """)
        let ids = try pane.view().a2uiComponents().map { value -> JSONValue? in
            if case let .object(f) = value { f["id"] } else { nil }
        }
        #expect(ids == [.string("root"), .string("root.0")])
    }
}

@Suite struct PaneRuntimeHostileScriptTests {
    @Test func aModuleWithAMetatableRunsNoScriptOutsideTheLimits() throws {
        #expect(throws: PaneRuntime.Failure.module("the module has no view function")) {
            try PaneRuntime(source: "return setmetatable({}, { __index = function() error('boom') end })")
        }
        #expect(throws: PaneRuntime.Failure.module("the module has no view function")) {
            try PaneRuntime(source: "return setmetatable({}, { __index = function() while true do end end })")
        }
        let pane = try PaneRuntime(source: """
        return setmetatable({ view = function() return ui.Text("ok") end },
            { __index = function() while true do end end })
        """)
        try pane.send(.null)
        #expect(text(try pane.view()) == "ok")
    }

    @Test func aSharedOrCyclicTableInViewIsAnErrorNotAHang() throws {
        let shared = try PaneRuntime(source: """
        return { view = function()
            local t = ui.Text("x")
            for i = 1, 40 do t = ui.VStack{ t, t } end
            return t
        end }
        """)
        #expect(throws: PaneRuntime.Failure.self) { try shared.view() }
        let cyclic = try PaneRuntime(source: """
        return { view = function()
            local t = { component = "Text" }
            t.a = t
            t.b = t
            return t
        end }
        """)
        #expect(throws: PaneRuntime.Failure.self) { try cyclic.view() }
        let feedArgs = try PaneRuntime(source: """
        return { view = function(state, data)
            local t = {}
            t.self = t
            return ui.Text(tostring(data.feed("x", t)))
        end }
        """)
        #expect(throws: PaneRuntime.Failure.self) { try feedArgs.view() }
    }

    @Test func garbageFromAnEarlierEntryDoesNotCountAgainstTheNext() throws {
        let pane = try PaneRuntime(source: """
        return {
            init = function()
                local t = {}
                for i = 1, 200000 do t[i] = string.rep("x", 100) .. i end
                return { n = #t }
            end,
            update = function(state) state.n += #string.rep("y", 15 * 1024 * 1024) - 15 * 1024 * 1024 + 1 end,
            view = function(state) return ui.Text(tostring(state.n)) end,
        }
        """, limits: .init(memory: 40 << 20, steps: 100_000_000))
        try pane.send(.null)
        #expect(text(try pane.view()) == "200001")
    }

    @Test func tablesConvertWithoutDroppingKeys() throws {
        let pane = try PaneRuntime(source: """
        return {
            init = function() return { list = {}, sparse = { 1, 2, nil, 4 }, mixed = { 1, x = 2 } } end,
            view = function() return ui.Text("x") end,
        }
        """)
        #expect(try pane.state == .object([
            "list": .array([]),
            "sparse": .object(["1": .number(1), "2": .number(2), "4": .number(4)]),
            "mixed": .object(["1": .number(1), "x": .number(2)]),
        ]))
    }
}
