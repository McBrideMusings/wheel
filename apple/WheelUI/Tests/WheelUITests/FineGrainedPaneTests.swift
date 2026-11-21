import Foundation
import Testing
import WheelKit
@testable import WheelUI

/// A mounted pane script (one that binds `ui.state`): `view` runs once and a change writes only the nodes that read it.
@MainActor
struct FineGrainedPaneTests {
    static let source = """
    local function init(p) return p or {
        count = ui.state(0),
        q = ui.state(""),
        order = ui.state({ "a", "b", "c" }),
    } end
    local function update(state, msg)
        if msg.type == "inc" then state.count:set(state.count:get() + 1) end
        if msg.type == "q" then state.q:set(msg.value) end
        if msg.type == "reverse" then
            local o, out = state.order:get(), {}
            for i = #o, 1, -1 do table.insert(out, o[i]) end
            state.order:set(out)
        end
        if msg.type == "drop" then
            local out = {}
            for _, k in state.order:get() do if k ~= msg.key then table.insert(out, k) end end
            state.order:set(out)
        end
    end
    local function view(state)
        return ui.Column{
            ui.Text("heading"),
            ui.Text{ text = ui.computed(function() return "Count " .. state.count:get() end) },
            ui.Button{ label = "Add", action = { type = "inc" } },
            ui.TextField{ label = "Search", value = state.q, on_change = { type = "q" } },
            ui.Text{ text = ui.computed(function() return "q=" .. state.q:get() end) },
            ui.Column{ ui.each(state.order, nil, function(item, key)
                return ui.TextField{ label = "Field " .. key }
            end) },
        }
    end
    return { init = init, update = update, view = view }
    """

    func load() -> PluginPane {
        let pane = PluginPane()
        pane.load(source: Self.source, chunk: "t.luau")
        #expect(pane.error == nil)
        return pane
    }

    @Test func aPressWritesOneNodeAndRunsNoView() throws {
        let pane = load()
        #expect(pane.perf.renders == 1)
        let heading = try #require(pane.tree.node("root/Text0"))
        #expect(pane.press(component: "root/Button0"))
        #expect(pane.error == nil)
        #expect(pane.perf.renders == 1)
        #expect(pane.perf.flushes == 1)
        #expect(pane.perf.nodes == 1)
        #expect(pane.tree.node("root/Text1")?.props["text"] == .string("Count 1"))
        #expect(pane.tree.node("root/Text0") === heading)
    }

    @Test func typingKeepsTheDraftAndWritesTheReader() throws {
        let pane = load()
        #expect(pane.set(component: "root/TextField0", value: "ab"))
        #expect(pane.perf.renders == 1)
        #expect(pane.tree.node("root/TextField0")?.value == .string("ab"))
        #expect(pane.tree.node("root/Text2")?.props["text"] == .string("q=ab"))
        // The field's value comes back equal to its draft: only the reader is written.
        #expect(pane.perf.nodes == 1)
    }

    @Test func aReorderedEachKeepsEachFieldAndItsText() throws {
        let pane = load()
        let ids = pane.componentTexts.map { $0["id"] ?? "" }.filter { $0.hasPrefix("root/Column0/") }
        #expect(ids.count == 3)
        let a = try #require(pane.tree.node(ids[0]))
        #expect(pane.set(component: a.id, value: "typed in a"))
        #expect(pane.press(component: "root/Button0"))
        pane.tree.onRaise?(.object(["type": .string("reverse")]))
        #expect(pane.error == nil)
        #expect(pane.perf.renders == 1)
        let after = pane.componentTexts.map { $0["id"] ?? "" }.filter { $0.hasPrefix("root/Column0/") }
        #expect(after == ids.reversed())
        #expect(pane.tree.node(ids[0]) === a)
        #expect(a.value == .string("typed in a"))
        // A dropped key leaves the tree and the index.
        pane.tree.onRaise?(.object(["type": .string("drop"), "key": .string("b")]))
        #expect(pane.tree.node(ids[1]) == nil)
        #expect(pane.tree.node(ids[0]) === a)
    }

    @Test func aReloadMountsAgainAndKeepsNodes() throws {
        let pane = load()
        let heading = try #require(pane.tree.node("root/Text0"))
        #expect(pane.press(component: "root/Button0"))
        pane.load(source: Self.source.replacingOccurrences(of: "\"Count \"", with: "\"N \""), chunk: "t.luau")
        #expect(pane.error == nil)
        #expect(pane.perf.renders == 2)
        #expect(pane.tree.node("root/Text1")?.props["text"] == .string("N 1"))
        #expect(pane.tree.node("root/Text0") === heading)
        #expect(pane.press(component: "root/Button0"))
        #expect(pane.tree.node("root/Text1")?.props["text"] == .string("N 2"))
    }

    @Test func aBoundPropHoldingAComponentSwapsIt() throws {
        let source = """
        local function init(p) return p or { on = ui.state(false) } end
        local function update(state) state.on:set(not state.on:get()) end
        local function view(state)
            return ui.Column{
                ui.Button{ action = { type = "flip" }, child = ui.computed(function()
                    return if state.on:get() then ui.Text("on") else ui.Image{ name = "off" }
                end) },
            }
        end
        return { init = init, update = update, view = view }
        """
        let pane = PluginPane()
        pane.load(source: source, chunk: "t.luau")
        #expect(pane.tree.node("root/Button0@child")?.component == "Image")
        #expect(pane.press(component: "root/Button0"))
        #expect(pane.error == nil)
        #expect(pane.tree.node("root/Button0@child")?.component == "Text")
        #expect(pane.tree.node("root/Button0@child")?.props["text"] == .string("on"))
        #expect(pane.press(component: "root/Button0"))
        #expect(pane.tree.node("root/Button0@child")?.component == "Image")
    }

    @Test func aBoundPresentedShowsAndHidesTheModal() throws {
        let source = """
        local function init(p) return p or { open = ui.state(false) } end
        local function update(state) state.open:set(not state.open:get()) end
        local function view(state)
            return ui.Column{
                ui.Button{ label = "Open", action = { type = "flip" } },
                ui.Modal{ presented = state.open, content = ui.Text("inside") },
            }
        end
        return { init = init, update = update, view = view }
        """
        let pane = PluginPane()
        pane.load(source: source, chunk: "t.luau")
        #expect(pane.tree.modals.isEmpty)
        #expect(pane.press(component: "root/Button0"))
        #expect(pane.tree.modals.map(\.id) == ["root/Modal0"])
        #expect(pane.press(component: "root/Button0"))
        #expect(pane.tree.modals.isEmpty)
    }

    @Test func boundModalsStackInViewOrder() throws {
        let source = """
        local function init(p) return p or { open = ui.state(false) } end
        local function update(state) state.open:set(true) end
        local function view(state)
            return ui.Column{
                ui.Button{ label = "Open", action = { type = "open" } },
                ui.Modal{ presented = state.open, content = ui.Text("first") },
                ui.Modal{ key = "a", presented = state.open, content = ui.Text("second") },
            }
        end
        return { init = init, update = update, view = view }
        """
        let pane = PluginPane()
        pane.load(source: source, chunk: "t.luau")
        #expect(pane.press(component: "root/Button0"))
        #expect(pane.perf.renders == 1)
        #expect(pane.tree.modals.map(\.id) == ["root/Modal0", "root/=a"])
    }

    @Test func aFeedChangeRewritesItsReader() throws {
        let source = """
        local function view(state, data)
            local tick = ui.state(0)
            return ui.Column{ ui.Text{ text = ui.computed(function()
                tick:get()
                local v = data.feed("demo.items", {})
                return if v == nil then "none" else v
            end) } }
        end
        return { view = view }
        """
        let pane = PluginPane()
        var subscribed: [String] = []
        pane.onFeeds = { sub, _ in subscribed += sub.map(\.0) }
        pane.load(source: source, chunk: "t.luau")
        #expect(subscribed == ["demo.items"])
        pane.feedUpdate(feed: "demo.items", params: [String: Any](), value: "hello", patch: nil)
        #expect(pane.error == nil)
        #expect(pane.perf.renders == 1)
        #expect(pane.tree.node("root/Text0")?.props["text"] == .string("hello"))
    }
}
