import Foundation
import SwiftUI
import Testing
import WheelKit
@testable import WheelUI

/// A pane's view reconciled into the keyed tree: what a render writes, which identities survive, and what a dropped node does.
@MainActor
struct ViewTreeTests {
    /// A search field over a filtered list: one keystroke changes the field and the rows that leave or stay.
    static let search = """
    return {
        init = function(p) return p or { q = "", items = { "apple", "banana", "cherry" } } end,
        update = function(state, msg) if msg.type == "q" then state.q = msg.value end end,
        view = function(state)
            local rows = {}
            for _, it in state.items do
                if string.find(it, state.q, 1, true) then table.insert(rows, ui.Text{ key = it, text = it }) end
            end
            return ui.Column{
                ui.Text("Fruit"),
                ui.TextField{ label = "Search", value = state.q, on_change = { type = "q" } },
                ui.Column(rows),
            }
        end,
    }
    """

    func load(_ source: String) -> PluginPane {
        let pane = PluginPane()
        pane.load(source: source, chunk: "t.luau")
        #expect(pane.error == nil)
        return pane
    }

    @Test func aHeldPointerStaysInTheClient() throws {
        let pane = load(Self.search)
        let renders = pane.perf.renders
        #expect(pane.hover(component: "root/TextField0"))
        #expect(pane.tree.hover.held == "root/TextField0")
        #expect(!pane.hover(component: "root/nothing"))
        #expect(pane.tree.hover.held == "root/TextField0")
        #expect(pane.hover(component: nil))
        #expect(pane.tree.hover.held == nil)
        #expect(pane.perf.renders == renders)
    }

    @Test func idsComeFromKeysElseComponentAndPlace() throws {
        let pane = load(Self.search)
        let ids = pane.componentTexts.map { $0["id"] ?? "" }
        #expect(ids == ["root", "root/Text0", "root/TextField0", "root/Column0", "root/Column0/=apple", "root/Column0/=banana", "root/Column0/=cherry"])
    }

    @Test func aKeystrokeWritesOnlyTheNodesThatChanged() throws {
        let pane = load(Self.search)
        let heading = try #require(pane.tree.node("root/Text0"))
        let cherry = try #require(pane.tree.node("root/Column0/=cherry"))
        // "an" keeps banana; the field's value comes back equal to its draft, so the field is not written again by the render.
        #expect(pane.set(component: "root/TextField0", value: "an"))
        #expect(pane.tree.node("root/Column0/=banana") != nil)
        #expect(pane.tree.node("root/Column0/=cherry") == nil)
        // Only the list's child order changed: the list node is the one node written.
        #expect(pane.perf.nodes == 1)
        #expect(pane.tree.node("root/Text0") === heading)
        // Clearing the search brings cherry back as a new node; the heading is the same object throughout.
        #expect(pane.set(component: "root/TextField0", value: ""))
        #expect(pane.tree.node("root/Column0/=cherry") !== cherry)
        #expect(pane.perf.nodes == 3)
        #expect(pane.tree.node("root/Text0") === heading)
    }

    @Test func aRenderThatChangesNothingWritesNothing() throws {
        let pane = load(Self.search)
        pane.actionResult(action: "noop", result: nil)
        #expect(pane.perf.nodes == 0)
    }

    @Test func aKeyedRowKeepsItsDraftWhenTheListReorders() throws {
        let pane = load("""
        return {
            init = function(p) return p or { order = { "a", "b" } } end,
            update = function(state, msg) if msg.type == "flip" then state.order = { state.order[2], state.order[1] } end end,
            view = function(state)
                local rows = {}
                for _, k in state.order do table.insert(rows, ui.TextField{ key = k, label = k }) end
                rows.key = "list"
                return ui.Column{ ui.Column(rows), ui.Button{ child = ui.Text("Flip"), action = { type = "flip" } } }
            end,
        }
        """)
        let b = try #require(pane.tree.node("root/=list/=b"))
        #expect(pane.set(component: "root/=list/=b", value: "typed"))
        #expect(pane.press(component: "root/Button0"))
        #expect(pane.tree.node("root/=list")?.children.map(\.id) == ["root/=list/=b", "root/=list/=a"])
        #expect(pane.tree.node("root/=list/=b") === b)
        #expect(b.value == .string("typed"))
    }

    @Test func anUnkeyedInsertionBeforeSiblingsRebuildsTheLaterOnes() throws {
        let pane = load("""
        return {
            init = function(p) return p or { first = false } end,
            update = function(state, msg) state.first = true end,
            view = function(state)
                local rows = {}
                if state.first then table.insert(rows, ui.TextField{ label = "new" }) end
                table.insert(rows, ui.TextField{ label = "old" })
                table.insert(rows, ui.Button{ child = ui.Text("Add"), action = { type = "add" } })
                return ui.Column(rows)
            end,
        }
        """)
        #expect(pane.set(component: "root/TextField0", value: "draft"))
        #expect(pane.press(component: "root/Button0"))
        // The old field is now the second TextField: a new node, its draft gone; the first is the one inserted.
        #expect(pane.tree.node("root/TextField1")?.props["label"] == .string("old"))
        #expect(pane.tree.node("root/TextField1")?.value == .string(""))
        #expect(pane.tree.node("root/TextField0")?.props["label"] == .string("new"))
    }

    @Test func aDuplicateKeyDrawsTheFirstAndErrors() throws {
        let pane = PluginPane()
        pane.load(source: """
        return { view = function() return ui.Column{ ui.Text{ key = "x", text = "one" }, ui.Text{ key = "x", text = "two" } } end }
        """, chunk: "dup.luau")
        #expect(pane.error?.contains("duplicate key root x") == true)
        #expect(pane.tree.node("root")?.children.map(\.id) == ["root/=x"])
        #expect(pane.tree.node("root/=x")?.props["text"] == .string("one"))
    }

    @Test func aKeyNeverTakesAGeneratedIdOrAPath() throws {
        let pane = load("""
        return { view = function() return ui.Column{ ui.Text("plain"), ui.Text{ key = "Text0", text = "keyed" }, ui.Text{ key = "a/b@c", text = "odd" } } end }
        """)
        #expect(pane.tree.node("root")?.children.map(\.id) == ["root/Text0", "root/=Text0", "root/=a%2Fb%40c"])
        #expect(pane.tree.node("root/Text0")?.props["text"] == .string("plain"))
        #expect(pane.tree.node("root/=Text0")?.props["text"] == .string("keyed"))
    }

    @Test func presentedModalsStackInTheOrderTheViewGivesThem() throws {
        let pane = load("""
        return { view = function()
            local cs = {}
            for i = 1, 11 do table.insert(cs, ui.Modal{ presented = true, ui.Text("m" .. i) }) end
            table.insert(cs, ui.Modal{ key = "a", presented = true, ui.Text("keyed") })
            return ui.Column(cs)
        end }
        """)
        var expected = (0..<11).map { "root/Modal\($0)" }
        expected.append("root/=a")
        #expect(pane.tree.modals.map(\.id) == expected)
    }

    @Test func raiseOnADroppedNodeReturnsFalse() throws {
        let pane = load("""
        return {
            init = function(p) return p or { shown = true } end,
            update = function(state, msg) state.shown = false end,
            view = function(state)
                if state.shown then return ui.Column{ ui.Button{ child = ui.Text("Hide"), action = { type = "hide" } } } end
                return ui.Column{}
            end,
        }
        """)
        let button = try #require(pane.tree.node("root/Button0"))
        #expect(pane.tree.raise(button, "action"))
        #expect(pane.tree.node("root/Button0") == nil)
        #expect(!pane.tree.raise(button, "action"))
    }

    @Test func aChangedTableRowIsTheOnlyRowWritten() throws {
        let pane = load("""
        return {
            init = function(p) return p or { n = 0 } end,
            update = function(state, msg) state.n += 1 end,
            view = function(state)
                local rows = {}
                for i = 1, 50 do
                    local text = if i == 7 then "seven " .. state.n else "row " .. i
                    table.insert(rows, { id = "r" .. i, cells = { { key = "t", text = text } } })
                end
                return ui.Column{
                    ui.Button{ child = ui.Text("Bump"), action = { type = "bump" } },
                    ui.Table{ columns = { { key = "t", label = "T" } }, sections = { { rows = rows } } },
                }
            end,
        }
        """)
        let row1 = try #require(pane.tree.node("root/Table0/r1"))
        #expect(pane.press(component: "root/Button0"))
        #expect(pane.perf.nodes == 1)
        #expect(pane.tree.node("root/Table0/r1") === row1)
        #expect(pane.tree.node("root/Table0/r7")?.props["cells"]?.arrayValue?.first?.dictionaryValue?["text"] == .string("seven 1"))
    }

    @Test func aCellsViewIsItsRowsEmbeddedNode() throws {
        let pane = load("""
        return {
            init = function(p) return p or { n = 0 } end,
            update = function(state, msg) state.n += 1 end,
            view = function(state)
                local rows = {}
                for i = 1, 3 do
                    local label = if i == 2 then "two " .. state.n else "row " .. i
                    table.insert(rows, { id = "r" .. i, cells = { { key = "t", text = "row " .. i, view = ui.Text(label) } } })
                end
                return ui.Column{
                    ui.Button{ child = ui.Text("Bump"), action = { type = "bump" } },
                    ui.Table{ columns = { { key = "t", label = "T" } }, sections = { { rows = rows } } },
                }
            end,
        }
        """)
        let table = try #require(pane.tree.node("root/Table0"))
        #expect(table.embedded.isEmpty)
        let row2 = try #require(pane.tree.node("root/Table0/r2"))
        let cell = try #require(row2.embedded["cells.0.view"])
        #expect(cell.id == "root/Table0/r2@cells.0.view")
        #expect(cell.props["text"] == .string("two 0"))
        #expect(pane.press(component: "root/Button0"))
        // Only the cell's own Text changed: the row node and the table keep their props.
        #expect(pane.perf.nodes == 1)
        #expect(row2.embedded["cells.0.view"] === cell)
        #expect(cell.props["text"] == .string("two 1"))
    }

    @Test func setWritesOnePropOnlyWhenItDiffers() throws {
        let pane = load(Self.search)
        let heading = try #require(pane.tree.node("root/Text0"))
        pane.tree.set(node: heading, prop: "text", value: .string("Fruit"))
        #expect(pane.tree.written == 0)
        pane.tree.set(node: heading, prop: "text", value: .string("Veg"))
        #expect(pane.tree.written == 1)
        #expect(heading.props["text"] == .string("Veg"))
    }

    #if os(macOS)
    /// Drawn in a window: a keystroke that changes one Text re-evaluates that Text's body, not the pane's.
    @Test func aKeystrokeEvaluatesOnlyTheChangedBodies() async throws {
        let pane = load("""
        return {
            init = function(p) return p or { q = "" } end,
            update = function(state, msg) state.q = msg.value end,
            view = function(state)
                local rows = {}
                for i = 1, 40 do table.insert(rows, ui.Text{ key = "t" .. i, text = "row " .. i }) end
                table.insert(rows, ui.Text{ key = "echo", text = "q=" .. state.q })
                return ui.Column{ ui.TextField{ label = "Search", value = state.q, on_change = { type = "q" } }, ui.Column(rows) }
            end,
        }
        """)
        let host = NSHostingView(rootView: PluginPaneView(pane: pane))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 400, height: 1200), styleMask: [.titled], backing: .buffered, defer: false)
        window.contentView = host
        func settle() async throws {
            for _ in 0..<5 { host.layoutSubtreeIfNeeded(); try await Task.sleep(nanoseconds: 20_000_000) }
        }
        try await settle()
        let before = ViewPerf.bodies
        #expect(pane.set(component: "root/TextField0", value: "w"))
        try await settle()
        // The echo Text's body, and the field's own draft: at most a few bodies of the 43 drawn.
        let spent = ViewPerf.bodies - before
        #expect(spent >= 1 && spent <= 3, "bodies evaluated: \(spent)")
    }
    #endif
}
