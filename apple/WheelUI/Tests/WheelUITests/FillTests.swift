import Foundation
import Testing
@testable import WheelUI

@MainActor
struct FillTests {
    private func pane(_ view: String) -> PluginPane {
        let pane = PluginPane()
        pane.load(source: "return { view = function() return \(view) end }", chunk: "fill.luau")
        #expect(pane.error == nil)
        return pane
    }

    @Test func aFramelessVerticalListFillsThePane() {
        #expect(pane(#"ui.Column{ ui.List{ ui.Text("a") }, ui.Text("composer") }"#).fillsPane)
    }

    @Test func aListWithAFrameOrAHorizontalListDoesNotFill() {
        #expect(!pane(#"ui.List{ ui.Text("a"), modifiers = { { frame = { height = 80 } } } }"#).fillsPane)
        #expect(!pane(#"ui.List{ ui.Text("a"), direction = "horizontal" }"#).fillsPane)
        #expect(!pane(#"ui.Text("a")"#).fillsPane)
    }

    @Test func aVideoStreamWithNoHeightFillsThePaneUnderAZStack() {
        #expect(pane(#"ui.ZStack{ ui.VideoStream{ stream = "p.s" }, ui.Button{ child = ui.Text("x") }, alignment = "topLeading" }"#).fillsPane)
        #expect(!pane(#"ui.VideoStream{ stream = "p.s", height = 200 }"#).fillsPane)
    }

    @Test func aFramelessTableFillsThePane() {
        #expect(pane(#"ui.Table{ columns = {}, sections = {} }"#).fillsPane)
    }
}
