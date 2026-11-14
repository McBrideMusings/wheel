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
