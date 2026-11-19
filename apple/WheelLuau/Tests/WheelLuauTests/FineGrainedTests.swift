import Foundation
import Testing
import WheelKit
@testable import WheelLuau

/// `ui.state`, `ui.computed` and `ui.each`: a view that binds one runs once, and a change writes only the props that read it.
@Suite struct FineGrainedTests {
    static let counter = """
    local function init(p) return p or { count = ui.state(0), other = ui.state("x") } end
    local function update(state, msg)
        if msg.type == "inc" then state.count:set(state.count:get() + 1) end
        if msg.type == "other" then state.other:set(msg.value) end
    end
    local function view(state)
        local label = ui.computed(function() return "Count " .. state.count:get() end)
        return ui.Column{
            ui.Text{ text = label },
            ui.Text{ text = state.other },
            ui.Text("static"),
            ui.Button{ label = "Add", action = { type = "inc" } },
        }
    end
    return { init = init, update = update, view = view }
    """

    @Test func oneStateChangeWritesOneProp() throws {
        let rt = try PaneRuntime(source: Self.counter)
        let tree = try rt.view()
        #expect(rt.mounted)
        #expect(rt.viewRuns == 1)
        #expect(tree.children[0].props["text"] == .string("Count 0"))
        #expect(tree.children[1].props["text"] == .string("x"))
        let label = try #require(tree.children[0].handle)
        try rt.send(.object(["type": .string("inc")]))
        let changes = try rt.flush()
        #expect(changes.ops == [.set(node: label, prop: "text", value: .string("Count 1"))])
        #expect(rt.viewRuns == 1)
    }

    @Test func aSetToTheSameValueWritesNothing() throws {
        let rt = try PaneRuntime(source: Self.counter)
        _ = try rt.view()
        try rt.send(.object(["type": .string("other"), "value": .string("x")]))
        #expect(try rt.flush().ops.isEmpty)
    }

    @Test func aViewWithoutBindingsIsTheWholeViewModel() throws {
        let rt = try PaneRuntime(source: "return { view = function() return ui.Text('hi') end }")
        let tree = try rt.view()
        #expect(!rt.mounted)
        #expect(tree.handle == nil)
        #expect(try rt.flush().ops.isEmpty)
    }

    @Test func stateJSONIsTheStatesValues() throws {
        let rt = try PaneRuntime(source: Self.counter)
        _ = try rt.view()
        try rt.send(.object(["type": .string("inc")]))
        #expect(try rt.state == .object(["count": .number(1), "other": .string("x")]))
    }

    static let list = """
    local function init(p) return p or { items = ui.state({ { id = "a", n = 1 }, { id = "b", n = 2 }, { id = "c", n = 3 } }), runs = 0 } end
    local function update(state, msg)
        local items = state.items:get()
        if msg.type == "reverse" then
            local out = {}
            for i = #items, 1, -1 do table.insert(out, items[i]) end
            state.items:set(out)
        elseif msg.type == "bump" then
            local out = {}
            for _, it in items do table.insert(out, { id = it.id, n = if it.id == msg.id then it.n + 10 else it.n }) end
            state.items:set(out)
        elseif msg.type == "drop" then
            local out = {}
            for _, it in items do if it.id ~= msg.id then table.insert(out, it) end end
            state.items:set(out)
        elseif msg.type == "add" then
            local out = table.clone(items)
            table.insert(out, { id = msg.id, n = 0 })
            state.items:set(out)
        end
    end
    local function view(state)
        return ui.Column{
            ui.Text("head"),
            ui.each(state.items, "id", function(item, key)
                state.runs += 1
                local local_state = ui.state(key .. "!")
                return ui.Row{
                    ui.Text{ text = ui.computed(function() return item:get().id .. "=" .. item:get().n end) },
                    ui.Text{ text = local_state },
                }
            end),
        }
    end
    return { init = init, update = update, view = view }
    """

    @Test func eachKeepsItemsByKey() throws {
        let rt = try PaneRuntime(source: Self.list)
        let tree = try rt.view()
        #expect(tree.children.map { $0.props["key"] } == [nil, .string("a"), .string("b"), .string("c")])
        let column = try #require(tree.handle)
        let handles = tree.children.map(\.handle)

        // Reverse: one children write, every item kept, no item function run.
        try rt.send(.object(["type": .string("reverse")]))
        let reversed = try rt.flush().ops
        #expect(reversed.count == 1)
        guard case let .children(node, kids) = reversed[0] else { Issue.record("not a children op"); return }
        #expect(node == column)
        #expect(kids.map(\.keep) == [true, true, true, true])
        #expect(kids.map(\.handle) == [handles[0], handles[3], handles[2], handles[1]])
        #expect(try rt.state.dictionaryField("runs") == .number(3))

        // A changed item writes its own text only.
        try rt.send(.object(["type": .string("bump"), "id": .string("b")]))
        let bumped = try rt.flush().ops
        #expect(bumped.count == 1)
        if case let .set(_, prop, value) = bumped.first { #expect(prop == "text"); #expect(value == .string("b=12")) }
        else { Issue.record("not a set op") }

        // A new key runs the function once and comes complete; a dropped key leaves.
        try rt.send(.object(["type": .string("add"), "id": .string("d")]))
        let added = try rt.flush().ops
        guard case let .children(_, withD) = added.first else { Issue.record("not a children op"); return }
        #expect(withD.last?.keep == false)
        #expect(withD.last?.props["key"] == .string("d"))
        #expect(withD.last?.children.last?.props["text"] == .string("d!"))
        #expect(try rt.state.dictionaryField("runs") == .number(4))
        try rt.send(.object(["type": .string("drop"), "id": .string("a")]))
        guard case let .children(_, withoutA) = try rt.flush().ops.first else { Issue.record("not a children op"); return }
        #expect(withoutA.map { $0.props["key"] } == [nil, .string("c"), .string("b"), .string("d")])
        #expect(withoutA.map(\.keep) == [true, true, true, true])
    }

    @Test func aDuplicateEachKeyIsAnError() throws {
        let source = """
        return { view = function() return ui.Column{ ui.each(ui.state({ "a", "a" }), nil, function(_, k) return ui.Text(k) end) } end }
        """
        let rt = try PaneRuntime(source: source)
        #expect(throws: PaneRuntime.Failure.self) { try rt.view() }
    }

    static let feeds = """
    local function view(state, data)
        local q = ui.state("one")
        state.q = q
        return ui.Column{
            ui.Text{ text = ui.computed(function()
                local v = data.feed("items", { q = q:get() })
                return if v == nil then "none" else v
            end) },
        }
    end
    local function update(state, msg) state.q:set(msg.q) end
    return { view = view, update = update }
    """

    @Test func aFeedReadInAComputedIsLiveAndItsChangeRewrites() throws {
        var values: [String: JSONValue] = [:]
        let host = PaneRuntime.Host(feed: { name, params in
            let key = name + " " + (params.dictionaryField("q")?.stringField ?? "")
            return PaneRuntime.FeedRead(value: values[key] ?? .null, key: key)
        })
        let rt = try PaneRuntime(source: Self.feeds, host: host)
        let tree = try rt.view()
        #expect(rt.feeds == ["items one"])
        #expect(tree.children[0].props["text"] == .string("none"))
        values["items one"] = .string("first")
        let changes = try rt.flush(changed: "feed:items one")
        #expect(changes.ops.count == 1)
        if case let .set(_, _, value) = changes.ops.first { #expect(value == .string("first")) }
        // A changed param moves the computation to the other feed.
        try rt.send(.object(["q": .string("two")]))
        #expect(try rt.flush().feeds == ["items two"])
        // The feed it left no longer re-runs anything.
        #expect(try rt.flush(changed: "feed:items one").ops.isEmpty)
    }

    @Test func aReloadMountsTheNewView() throws {
        let rt = try PaneRuntime(source: Self.counter)
        _ = try rt.view()
        try rt.send(.object(["type": .string("inc")]))
        try rt.reload(source: Self.counter.replacingOccurrences(of: "\"Count \"", with: "\"N \""))
        #expect(!rt.mounted)
        let tree = try rt.view()
        #expect(tree.children[0].props["text"] == .string("N 1"))
        #expect(rt.viewRuns == 2)
    }

    @Test func aCanvasInAMountedViewDrawsAgainOnANewSize() throws {
        let source = """
        local function view(state)
            state.s = ui.state(1)
            return ui.Column{ ui.Text{ text = state.s }, ui.Canvas{ key = "c", height = 10, draw = function(ctx, w, h) ctx:rect(0, 0, w, h) end } }
        end
        return { view = view }
        """
        let rt = try PaneRuntime(source: source)
        let tree = try rt.view()
        #expect(tree.children[1].props["commands"] == .array([.array([.string("rect"), .number(0), .number(0), .number(0), .number(10)])]))
        #expect(rt.setCanvasSize(key: "c", width: 50, height: 10))
        let changes = try rt.flush(changed: "size:c")
        #expect(changes.ops == [.set(node: tree.children[1].handle!, prop: "commands",
                                     value: .array([.array([.string("rect"), .number(0), .number(0), .number(50), .number(10)])]))])
    }

    @Test func aFailingComputedThrowsAndRetriesOnTheNextFlush() throws {
        let source = """
        local function view(state)
            state.n = ui.state(0)
            return ui.Column{ ui.Text{ text = ui.computed(function()
                if state.n:get() == 1 then error("boom") end
                return tostring(state.n:get())
            end) } }
        end
        local function update(state, msg) state.n:set(msg.n) end
        return { view = view, update = update }
        """
        let rt = try PaneRuntime(source: source)
        _ = try rt.view()
        try rt.send(.object(["n": .number(1)]))
        #expect(throws: PaneRuntime.Failure.self) { try rt.flush() }
        try rt.send(.object(["n": .number(2)]))
        let ops = try rt.flush().ops
        #expect(ops.count == 1)
        if case let .set(_, _, value) = ops.first { #expect(value == .string("2")) }
    }
}

private extension JSONValue {
    func dictionaryField(_ k: String) -> JSONValue? { if case let .object(o) = self { o[k] } else { nil } }
    var stringField: String? { if case let .string(s) = self { s } else { nil } }
}
