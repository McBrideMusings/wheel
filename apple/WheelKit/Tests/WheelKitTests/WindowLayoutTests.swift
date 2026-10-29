import Foundation
import Testing
@testable import WheelKit

struct WindowLayoutTests {
    private func layout(_ a: [String], _ b: [String]) -> (WindowLayout, String, String) {
        var layout = WindowLayout()
        layout.reconcile(a + b)
        let first = layout.windows[0].id
        let second = layout.newWindow(spaces: b)
        return (layout, first, second)
    }

    /// With nothing saved, every space opens in one window, the first showing.
    @Test func firstLaunchPutsEverySpaceInOneWindow() {
        var layout = WindowLayout()
        layout.reconcile(["s1", "s2", "s3"])
        #expect(layout.windows.count == 1)
        #expect(layout.windows[0].spaces == ["s1", "s2", "s3"])
        #expect(layout.windows[0].active == "s1")
        #expect(layout.background.isEmpty)
    }

    /// Closing a window leaves the other open and lists the closed one's spaces as background.
    @Test func closingAWindowSendsItsSpacesToTheBackground() {
        var (layout, first, second) = layout(["s1", "s2", "s3"], ["s4", "s5", "s6"])
        layout.close(window: second)
        #expect(layout.windows.map(\.id) == [first])
        #expect(layout.background == ["s4", "s5", "s6"])
        layout.close(window: first)
        #expect(layout.windows.isEmpty)
        #expect(layout.background == ["s4", "s5", "s6", "s1", "s2", "s3"])
        #expect(layout.focused == nil)
    }

    /// A drop in the background lands at its index; a reorder inside it counts the slot it leaves.
    @Test func placingInTheBackgroundHonoursThePosition() {
        var (layout, first, _) = layout(["s1", "s2"], ["s3", "s4"])
        layout.close(window: layout.windows[1].id)
        #expect(layout.background == ["s3", "s4"])
        layout.place("s1", in: nil, at: 1)
        #expect(layout.background == ["s3", "s1", "s4"])
        layout.place("s3", in: nil, at: 3)
        #expect(layout.background == ["s1", "s4", "s3"])
        layout.place("s2", in: nil)
        #expect(layout.background == ["s1", "s4", "s3", "s2"])
        _ = first
    }

    /// Saved windows come back the same, and a space deleted meanwhile is in none of them.
    @Test func aSavedLayoutRoundTripsAndForgetsDeletedSpaces() throws {
        var (saved, _, second) = layout(["s1", "s2"], ["s3", "s4"])
        saved.show("s3", in: second)
        var back = try JSONDecoder().decode(WindowLayout.self, from: JSONEncoder().encode(saved))
        #expect(back == saved)
        #expect(back.window(second)?.active == "s3")
        back.reconcile(["s1", "s3", "s4"])
        #expect(back.window(holding: "s2") == nil)
        #expect(back.windows.flatMap(\.spaces) == ["s1", "s3", "s4"])
    }

    /// A space nothing has placed joins the focused window as a tab and is not shown; with no window
    /// it goes to the background.
    @Test func aNewSpaceOpensInTheFocusedWindowOrTheBackground() {
        var (layout, first, second) = layout(["s1"], ["s2"])
        layout.focused = second
        let appeared = layout.reconcile(["s1", "s2", "s9"])
        #expect(appeared == ["s9"])
        #expect(layout.window(second)?.spaces == ["s2", "s9"])
        #expect(layout.window(second)?.active == "s2")
        #expect(layout.window(first)?.spaces == ["s1"])
        layout.close(window: first)
        layout.close(window: second)
        layout.reconcile(["s1", "s2", "s9", "s10"])
        #expect(layout.background.contains("s10"))
        #expect(layout.windows.isEmpty)
    }

    /// A background space whose last pane closed is emptied; a new one, one in a window, and one whose
    /// pane only became empty are not.
    @Test func onlyABackgroundSpaceWhoseLastPaneClosedIsEmptied() {
        var (layout, _, _) = layout(["s1"], [])
        layout.reconcile(["s1", "s2", "s3", "s4"], background: ["s2", "s3", "s4"])
        func space(_ id: String, _ pane: String, _ kind: PaneKind) -> SpaceInfo {
            SpaceInfo(id: id, name: id, cwd: "/tmp", root: .pane(id: pane, kind: kind), focus: pane)
        }
        let old = [space("s1", "p1", .terminal(term: "t1")), space("s2", "p2", .terminal(term: "t2")),
                   space("s3", "p3", .terminal(term: "t3"))]
        let new = [space("s1", "p11", .empty), space("s2", "p12", .empty),
                   space("s3", "p3", .empty), space("s4", "p4", .empty)]
        #expect(layout.emptied(from: old, to: new) == ["s2"])
    }

    /// Launching with restore off puts every space, background ones included, in one window.
    @Test func collapseHoldsEverySpaceInOneWindow() {
        var (layout, _, second) = layout(["s1", "s2"], ["s3"])
        layout.close(window: second)
        layout.collapse(["s1", "s2", "s3"])
        #expect(layout.windows.count == 1)
        #expect(layout.windows[0].spaces == ["s1", "s2", "s3"])
        #expect(layout.background.isEmpty)
    }

    /// A space dragged above the first leaves 3,1,2; moving to another window removes it from the old.
    @Test func placeReordersWithinAWindowAndMovesBetweenWindows() {
        var layout = WindowLayout()
        layout.reconcile(["s1", "s2", "s3"])
        let first = layout.windows[0].id
        layout.place("s3", in: first, at: 0)
        #expect(layout.windows[0].spaces == ["s3", "s1", "s2"])
        layout.place("s3", in: first, at: 3)
        #expect(layout.windows[0].spaces == ["s1", "s2", "s3"])
        let other = layout.newWindow()
        let moved = layout.place("s1", in: other, at: 0)
        #expect(moved)
        #expect(layout.window(first)?.spaces == ["s2", "s3"])
        #expect(layout.window(other)?.spaces == ["s1"])
        #expect(layout.window(first)?.active == "s3")
        layout.place("s2", in: nil)
        #expect(layout.background == ["s2"])
        let refused = layout.place("s2", in: "nope")
        #expect(!refused)
    }

    /// A main window closes on its own when its last space leaves it, whichever way it goes; one made
    /// empty on purpose stays until it is filled.
    @Test func aWindowClosesWhenItsLastSpaceLeaves() {
        var (layout, first, second) = layout(["s1", "s2"], ["s3"])
        layout.place("s3", in: first)
        #expect(layout.windows.map(\.id) == [first])
        #expect(layout.window(first)?.spaces == ["s1", "s2", "s3"])
        _ = second
        var (toBackground, _, other) = self.layout(["s1"], ["s2"])
        toBackground.place("s2", in: nil)
        #expect(toBackground.window(other) == nil)
        #expect(toBackground.background == ["s2"])
        var (alone, _, third) = self.layout(["s1"], ["s2"])
        let torn = alone.tearOff("s2")
        #expect(alone.window(third) == nil)
        #expect(alone.window(torn)?.spaces == ["s2"])
        var (filled, firstFilled, _) = self.layout(["s1"], ["s2"])
        let empty = filled.newWindow()
        #expect(filled.window(empty)?.spaces == [])
        filled.place("s1", in: empty)
        #expect(filled.window(empty)?.spaces == ["s1"])
        #expect(filled.window(firstFilled) == nil)
    }

    /// Reordering a window's only space leaves the window open.
    @Test func reorderingTheOnlySpaceKeepsItsWindow() {
        var (layout, _, second) = layout(["s1"], ["s2"])
        layout.place("s2", in: second, at: 0)
        #expect(layout.window(second)?.spaces == ["s2"])
    }

    /// Stopping a window's processes takes its spaces and their little windows out of the layout
    /// without sending them to the background.
    @Test func closingAWindowToStopItsProcessesRemovesItsSpacesAndLittleWindows() throws {
        var (layout, first, second) = layout(["s1", "s2"], ["s3"])
        let opened = layout.openLittle("s2")
        let little = try #require(opened)
        let stopped = layout.close(window: first, stopping: true)
        #expect(stopped == ["s1", "s2"])
        #expect(layout.windows.map(\.id) == [second])
        #expect(layout.little(little) == nil)
        #expect(layout.background.isEmpty)
    }

    @Test func tearOffOpensAWindowHoldingOnlyThatSpace() {
        var layout = WindowLayout()
        layout.reconcile(["s1", "s2"])
        let torn = layout.tearOff("s2")
        #expect(layout.window(torn)?.spaces == ["s2"])
        #expect(layout.windows[0].spaces == ["s1"])
        #expect(layout.focused == torn)
    }

    /// A little window belongs to the main window that holds its space, which keeps the tab; closing it
    /// leaves the space there, and the layout survives a save.
    @Test func aLittleWindowKeepsItsSpaceInTheOwnersTabs() throws {
        var (layout, first, second) = layout(["s1", "s2"], ["s3"])
        let opened = layout.openLittle("s2")
        let little = try #require(opened)
        #expect(layout.little(little)?.owner == first)
        #expect(layout.window(first)?.spaces == ["s1", "s2"])
        #expect(layout.openLittle("s2") == little)
        #expect(layout.openLittle("s99") == nil)
        let back = try JSONDecoder().decode(WindowLayout.self, from: JSONEncoder().encode(layout))
        #expect(back == layout)
        layout.closeLittle(little)
        #expect(layout.littles.isEmpty)
        #expect(layout.window(holding: "s2")?.id == first)
        #expect(layout.window(second)?.spaces == ["s3"])
    }

    /// Reordering inside the owner keeps the little window; moving the space away, or the space
    /// going, closes it; closing the owner leaves it open, owned by none.
    @Test func aLittleWindowEndsWhenItsSpaceLeavesItsOwner() throws {
        var (layout, first, second) = layout(["s1", "s2"], ["s3"])
        let opened = layout.openLittle("s2")
        let little = try #require(opened)
        layout.place("s2", in: first, at: 0)
        #expect(layout.little(little)?.owner == first)
        layout.close(window: first)
        #expect(layout.little(little)?.owner == nil)
        #expect(layout.little(little)?.space == "s2")
        var moved = layout
        moved.place("s2", in: second)
        #expect(moved.littles.isEmpty)
        layout.reconcile(["s1", "s3"])
        #expect(layout.littles.isEmpty)
    }

    /// Sending the space to the background, or tearing it off into a window, closes its little window.
    @Test func aLittleWindowEndsWhenItsSpaceLeavesForTheBackgroundOrAWindowOfItsOwn() {
        var (layout, _, _) = layout(["s1", "s2"], ["s3"])
        layout.openLittle("s1")
        layout.place("s1", in: nil)
        #expect(layout.littles.isEmpty)
        layout.openLittle("s2")
        layout.tearOff("s2")
        #expect(layout.littles.isEmpty)
    }

    /// A background space opens in a little window that belongs to none and leaves the background;
    /// closing it, or sending the space to the background, returns the space there once.
    @Test func aBackgroundSpaceOpensInAnOwnerlessLittleWindow() throws {
        var (layout, first, _) = layout(["s1", "s2"], ["s3"])
        layout.place("s2", in: nil)
        let opened = layout.openLittle("s2")
        let little = try #require(opened)
        #expect(layout.little(little)?.owner == nil)
        #expect(layout.background.isEmpty)
        #expect(layout.window(first)?.spaces == ["s1"])
        #expect(layout.placed.contains("s2"))
        layout.closeLittle(little)
        #expect(layout.background == ["s2"])
        #expect(layout.littles.isEmpty)
        layout.openLittle("s2")
        layout.place("s2", in: nil)
        #expect(layout.background == ["s2"])
        #expect(layout.littles.isEmpty)
    }

    /// Closing the owner leaves its little window open with its space out of the background; the
    /// little window's close then returns the space there.
    @Test func aLittleWindowWhoseOwnerClosedKeepsItsSpaceOutOfTheBackground() throws {
        var (layout, first, _) = layout(["s1", "s2"], ["s3"])
        let opened = layout.openLittle("s2")
        let little = try #require(opened)
        layout.close(window: first)
        #expect(layout.background == ["s1"])
        layout.closeLittle(little)
        #expect(layout.background == ["s1", "s2"])
    }

    /// A layout saved before little windows existed still opens.
    @Test func aLayoutSavedWithoutLittleWindowsDecodes() throws {
        let saved = #"{"windows":[],"background":["s1"]}"#
        let layout = try JSONDecoder().decode(WindowLayout.self, from: Data(saved.utf8))
        #expect(layout.background == ["s1"])
        #expect(layout.littles.isEmpty)
    }

    @Test func aWindowIsTitledByItsNameOrItsActiveSpace() {
        var layout = WindowLayout()
        layout.reconcile(["s1"])
        let id = layout.windows[0].id
        let names = ["s1": "plex-db"]
        #expect(layout.title(of: layout.windows[0]) { names[$0] } == "plex-db")
        layout.rename(window: id, to: "  Reviews ")
        #expect(layout.title(of: layout.windows[0]) { names[$0] } == "Reviews")
        layout.rename(window: id, to: "   ")
        #expect(layout.title(of: layout.windows[0]) { names[$0] } == "plex-db")
    }

    /// A pin on a window or a little window is saved with the layout; a layout saved before pins existed reads as unpinned.
    @Test func pinsRoundTripAndDefaultOff() throws {
        var (saved, first, _) = layout(["s1", "s2"], ["s3"])
        let opened = saved.openLittle("s1")
        let little = try #require(opened)
        saved.setPinned(true, window: first)
        saved.setPinned(true, little: little)
        let back = try JSONDecoder().decode(WindowLayout.self, from: JSONEncoder().encode(saved))
        #expect(back.window(first)?.pinned == true)
        #expect(back.little(little)?.pinned == true)
        #expect(back.windows.filter(\.pinned).count == 1)
        saved.setPinned(false, window: first)
        #expect(saved.window(first)?.pinned == false)
        let old = #"{"windows":[{"id":"w","spaces":["a"],"sidebar":true}],"background":[],"littles":[{"id":"l","space":"a"}]}"#
        let legacy = try JSONDecoder().decode(WindowLayout.self, from: Data(old.utf8))
        #expect(legacy.windows[0].pinned == false)
        #expect(legacy.littles[0].pinned == false)
    }
}
