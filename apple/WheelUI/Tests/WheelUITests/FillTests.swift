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

    @Test func aFrameWithInfiniteMaxHeightFillsThePane() {
        #expect(pane(#"ui.Column{ ui.Text("No stream"), modifiers = { { frame = { maxWidth = "infinity", maxHeight = "infinity", alignment = "center" } } } }"#).fillsPane)
        #expect(pane(#"ui.ZStack{ ui.Column{ ui.Text("a"), modifiers = { { frame = { maxHeight = "infinity" } } } } }"#).fillsPane)
        #expect(!pane(#"ui.Column{ ui.Text("a"), modifiers = { { frame = { maxWidth = "infinity" } } } }"#).fillsPane)
    }

    @Test func aFixedHeightStopsWhatItHoldsFromFilling() {
        // The gallery's NavigationSplitView: a List asking for every height inside a surface 140 points high keeps the page scrolling.
        #expect(!pane(#"ui.Column{ ui.NavigationSplitView{ ui.List{ ui.Text("a"), modifiers = { { frame = { maxHeight = "infinity" } } } }, ui.Text("d"), modifiers = { { frame = { height = 140 } } } } }"#).fillsPane)
        #expect(!pane(#"ui.Column{ ui.List{ ui.Text("a") }, modifiers = { { frame = { maxHeight = 300 } } } }"#).fillsPane)
    }

    @Test func aFramelessTableFillsThePane() {
        #expect(pane(#"ui.Table{ columns = {}, sections = {} }"#).fillsPane)
    }
}
