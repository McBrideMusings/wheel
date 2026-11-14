import Foundation
import Testing
import WheelKit
@testable import WheelUI

@MainActor @Suite struct PluginActionTests {
    static let script = """
    return {
        init = function(previous) return previous or { shown = "none" } end,
        update = function(state, msg)
            if msg.type == "go" then
                wheel.action("clock.echo", { n = 3, s = "hi" })
            elseif msg.action ~= nil then
                if msg.error ~= nil then
                    state.shown = "err " .. msg.action .. " " .. msg.error
                else
                    state.shown = "ok " .. msg.action .. " " .. tostring(msg.result.n)
                end
            end
        end,
        view = function(state, data)
            return ui.Column({
                ui.Text(state.shown),
                ui.Button{ child = ui.Text("Go"), action = { type = "go" } },
            })
        end,
    }
    """

    final class Calls { var actions: [(String, JSONValue)] = [] }

    func make(_ calls: Calls) -> PluginPane {
        let pane = PluginPane()
        pane.onAction = { name, args in calls.actions.append((name, args)) }
        pane.load(source: Self.script, chunk: "t.luau")
        return pane
    }

    func shown(_ pane: PluginPane) -> String? { pane.componentTexts.compactMap { $0["text"] }.first }

    func go(_ pane: PluginPane) {
        let button = pane.surface.components.values.first { $0.type == "Button" }!.id
        #expect(pane.press(component: button))
    }

    @Test func aScriptActionFromUpdateReachesTheHost() {
        let calls = Calls()
        let pane = make(calls)
        #expect(pane.error == nil)
        go(pane)
        #expect(calls.actions.count == 1)
        #expect(calls.actions.first?.0 == "clock.echo")
        #expect(calls.actions.first?.1 == .object(["n": .number(3), "s": .string("hi")]))
        #expect(pane.error == nil)
    }

    @Test func aResultReachesUpdateAndTheViewShowsIt() {
        let pane = make(Calls())
        #expect(shown(pane) == "none")
        pane.actionResult(action: "clock.echo", result: ["n": 3])
        #expect(shown(pane) == "ok clock.echo 3")
        #expect(pane.error == nil)
    }

    @Test func aFailureReachesUpdateAndTheViewShowsIt() {
        let pane = make(Calls())
        pane.actionFailed(action: "clock.fail", message: "boom")
        #expect(shown(pane) == "err clock.fail boom")
        #expect(pane.error == nil)
    }
}
