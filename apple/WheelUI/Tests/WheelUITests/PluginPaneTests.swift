import Foundation
import Testing
import WheelKit
@testable import WheelUI

/// A plugin pane against the sample plugin's script (`daemon/plugins/sample/main.luau`).
@MainActor @Suite struct PluginPaneTests {
    static let sample = """
    local function init(previous)
        return previous or { count = 0, name = "" }
    end
    local function update(state, msg)
        if msg.type == "add" then
            state.count += 1
        elseif msg.type == "name" then
            state.name = msg.value
        end
    end
    local function view(state, data)
        return ui.Column{
            modifiers = { { padding = 16 } },
            ui.Text{ text = "Count " .. state.count, variant = "h2" },
            ui.Button{ child = ui.Text("Add one"), action = { type = "add" } },
            ui.TextField{ label = "Name", value = state.name, on_change = { type = "name" } },
            ui.Text(if state.name == "" then "Type a name" else "Hello, " .. state.name),
        }
    end
    return { init = init, update = update, view = view }
    """

    func texts(_ pane: PluginPane) -> [String] { pane.componentTexts.compactMap { $0["text"] } }
    func button(_ pane: PluginPane) -> String? {
        pane.surface.components.values.first { $0.type == "Button" }?.id
    }

    @Test func aLoadDrawsTheScriptsView() {
        let pane = PluginPane()
        pane.load(source: Self.sample, chunk: "sample/main.luau")
        #expect(pane.error == nil)
        #expect(texts(pane) == ["Count 0", "Add one", "Type a name"])
        #expect(pane.componentTexts.contains { $0["type"] == "TextField" && $0["label"] == "Name" })
    }

    @Test func pressingTheButtonIncrementsTheCount() throws {
        let pane = PluginPane()
        pane.load(source: Self.sample, chunk: "sample/main.luau")
        let id = try #require(button(pane))
        #expect(pane.press(component: id))
        #expect(pane.press(component: id))
        #expect(pane.stateJSON == #"{"count":2,"name":""}"#)
        #expect(texts(pane).first == "Count 2")
    }

    @Test func aTextFieldChangeUpdatesTheState() throws {
        let pane = PluginPane()
        pane.load(source: Self.sample, chunk: "sample/main.luau")
        let field = try #require(pane.surface.components.values.first { $0.type == "TextField" })
        pane.set(path: "/" + field.id, value: "Ada")
        #expect(pane.stateJSON == #"{"count":0,"name":"Ada"}"#)
        #expect(texts(pane).last == "Hello, Ada")
    }

    @Test func aReloadKeepsTheState() throws {
        let pane = PluginPane()
        pane.load(source: Self.sample, chunk: "sample/main.luau")
        let id = try #require(button(pane))
        pane.press(component: id)
        pane.load(source: Self.sample.replacingOccurrences(of: "Add one", with: "Add 1"), chunk: "sample/main.luau")
        #expect(pane.reloads == 1)
        #expect(texts(pane) == ["Count 1", "Add 1", "Type a name"])
        #expect(pane.stateJSON == #"{"count":1,"name":""}"#)
    }

    @Test func aTableActionNamingEventStillReachesUpdate() throws {
        let pane = PluginPane()
        pane.load(source: """
        return {
            init = function(p) return p or { on = false } end,
            update = function(state, msg) if msg.event == "toggle" then state.on = not state.on end end,
            view = function(state)
                return ui.Button{ child = ui.Text("Toggle"), action = { event = "toggle" } }
            end,
        }
        """, chunk: "toggle.luau")
        #expect(pane.error == nil)
        #expect(pane.press(component: try #require(button(pane))))
        #expect(pane.stateJSON == #"{"on":true}"#)
    }

    static let controls = """
    return {
        init = function(p) return p or { log = {} } end,
        update = function(state, msg)
            local keys = {}
            for k, v in msg do table.insert(keys, k .. "=" .. (if type(v) == "table" then "[" .. #v .. "]" else tostring(v))) end
            table.sort(keys)
            table.insert(state.log, table.concat(keys, ","))
        end,
        view = function(state, data)
            return ui.Column{
                ui.Text("pane " .. tostring(data.pane) .. " cwd " .. tostring(data.cwd) .. " " .. tostring(data.plugin) .. "/" .. tostring(data.plugin_pane)),
                ui.Table{
                    columns = { { key = "t", label = "T" } },
                    sections = { { rows = { { id = "a", cells = { { key = "t", text = "A" } }, menu = { { label = "Drop", action = { type = "drop", id = "a" } } } } } } },
                    rowAction = { type = "open" }, sortAction = { type = "sort" }, foldAction = { type = "fold" },
                },
                ui.Canvas{ ops = {}, hits = { { id = "n1", rect = { 0, 0, 10, 10 }, action = { type = "hit" }, doubleAction = { type = "open", id = "n1" } } } },
                ui.CheckBox{ label = "Done", value = false, on_change = { type = "done" } },
                ui.ChoicePicker{ options = { { label = "A", value = "a" } }, value = {}, on_change = { type = "pick" } },
                ui.Modal{ dismissAction = { type = "dismiss" }, ui.Text("modal") },
            }
        end,
    }
    """

    func component(_ pane: PluginPane, _ type: String) throws -> String {
        try #require(pane.surface.components.values.first { $0.type == type }?.id)
    }

    @Test func viewGetsThePanesContext() {
        let pane = PluginPane()
        pane.context = ["pane": .string("p7"), "cwd": .string("/tmp/proj"), "plugin": .string("wh"), "plugin_pane": .string("beads")]
        pane.load(source: Self.controls, chunk: "controls.luau")
        #expect(pane.error == nil)
        #expect(texts(pane).first == "pane p7 cwd /tmp/proj wh/beads")
    }

    @Test func everyActionKeyReachesUpdateWithTheEventsFields() throws {
        let pane = PluginPane()
        pane.load(source: Self.controls, chunk: "controls.luau")
        let table = try component(pane, "Table"), canvas = try component(pane, "Canvas"), modal = try component(pane, "Modal")
        #expect(pane.raise(component: table, key: "rowAction", context: ["id": "a"]))
        #expect(pane.raise(component: table, key: "sortAction", context: ["key": "t", "asc": false]))
        #expect(pane.raise(component: table, key: "foldAction", context: ["path": "a"]))
        #expect(pane.raise(component: table, key: "sections.0.rows.0.menu.0.action", context: [:]))
        #expect(pane.raise(component: canvas, key: "hits.0.action", context: ["id": "n1"]))
        #expect(pane.raise(component: canvas, key: "hits.0.doubleAction", context: ["id": "other"]))
        #expect(pane.raise(component: modal, key: "dismissAction", context: nil))
        #expect(!pane.raise(component: table, key: "selectAction", context: nil))
        #expect(pane.stateJSON == #"{"log":["id=a,type=open","asc=false,key=t,type=sort","path=a,type=fold","id=a,type=drop","id=n1,type=hit","id=n1,type=open","type=dismiss"]}"#)
        // The Table draws the names, never the tables.
        let props = try #require(pane.surface.components[table]?.properties)
        #expect(props["rowAction"]?.stringValue == "plugin:\(table)/rowAction")
    }

    @Test func everyBoundControlSendsItsOnChange() throws {
        let pane = PluginPane()
        pane.load(source: Self.controls, chunk: "controls.luau")
        pane.set(path: "/" + (try component(pane, "CheckBox")), value: true)
        pane.set(path: "/" + (try component(pane, "ChoicePicker")), value: ["a"])
        #expect(pane.stateJSON == #"{"log":["type=done,value=true","type=pick,value=[1]"]}"#)
    }

    @Test func aControlTheScriptDoesNotEchoKeepsTheEdit() throws {
        let pane = PluginPane()
        pane.load(source: """
        return {
            init = function(p) return p or { picks = 0, reset = false } end,
            update = function(state, msg) if msg.type == "pick" then state.picks += 1 elseif msg.type == "reset" then state.reset = true end end,
            view = function(state)
                return ui.Column{
                    ui.ChoicePicker{ options = { { label = "A", value = "a" } }, on_change = { type = "pick" } },
                    ui.TextField{ label = "Name", value = if state.reset then "fresh" else "", on_change = { type = "name" } },
                    ui.Button{ child = ui.Text("Reset"), action = { type = "reset" } },
                }
            end,
        }
        """, chunk: "echo.luau")
        let picker = "/" + (try component(pane, "ChoicePicker")), field = "/" + (try component(pane, "TextField"))
        pane.set(path: picker, value: ["a"])
        pane.set(path: field, value: "Ada")
        #expect(pane.surface.get(picker) == .array([.string("a")]))
        #expect(pane.surface.get(field) == .string("Ada"))
        // A value the script changes is written over the edit.
        #expect(pane.press(component: try component(pane, "Button")))
        #expect(pane.surface.get(field) == .string("fresh"))
    }

    @Test func dataUnderAnActionKeyAndACopyStayAsWritten() throws {
        let pane = PluginPane()
        pane.load(source: """
        return {
            view = function()
                return ui.Column{
                    ui.Canvas{ ops = {}, hits = { { id = "n", rect = { 0, 0, 1, 1 }, action = { type = "hit" }, context = { action = { type = "data" } } } } },
                    ui.Button{ child = ui.Text("Copy"), action = { copy = { text = "wheel-1" } } },
                }
            end,
        }
        """, chunk: "data.luau")
        let canvas = try #require(pane.surface.components[try component(pane, "Canvas")]?.properties)
        let hit = try #require(canvas["hits"]?.arrayValue?.first?.dictionaryValue)
        #expect(hit["action"]?.stringValue != nil)
        #expect(hit["context"]?.dictionaryValue?["action"]?.dictionaryValue?["type"]?.stringValue == "data")
        let button = try #require(pane.surface.components[try component(pane, "Button")]?.properties)
        #expect(button["action"]?.dictionaryValue?["copy"]?.dictionaryValue?["text"]?.stringValue == "wheel-1")
    }

    @Test func aSyntaxErrorKeepsTheLastViewAndFixingClearsTheError() {
        let pane = PluginPane()
        pane.load(source: Self.sample, chunk: "sample/main.luau")
        pane.load(source: Self.sample + "\nlocal = =", chunk: "sample/main.luau")
        #expect(pane.error?.contains("sample/main.luau") == true)
        #expect(pane.error?.contains("\n") == false)
        #expect(texts(pane) == ["Count 0", "Add one", "Type a name"])
        #expect(pane.reloads == 0)
        // A message the old script handles does not clear the error: the file is still broken.
        pane.press(component: button(pane) ?? "")
        #expect(pane.error != nil)
        #expect(texts(pane).first == "Count 1")
        pane.load(source: Self.sample, chunk: "sample/main.luau")
        #expect(pane.error == nil)
        #expect(pane.reloads == 1)
    }
}
