import CoreGraphics
import Testing
@testable import WheelKit

struct SpaceDroppingTests {
    // Screen coordinates, origin bottom left: window A on the left, window B on the right.
    private let a = DropWindow(
        id: "A", frame: CGRect(x: 0, y: 0, width: 600, height: 500),
        sidebar: CGRect(x: 0, y: 0, width: 200, height: 500),
        rows: [CGRect(x: 0, y: 460, width: 200, height: 30), CGRect(x: 0, y: 430, width: 200, height: 30), CGRect(x: 0, y: 400, width: 200, height: 30)]
    )
    private let b = DropWindow(id: "B", frame: CGRect(x: 700, y: 0, width: 600, height: 500))

    /// The gap opens above the first row whose middle is below the pointer.
    @Test func aPointerInASidebarPicksTheGapUnderIt() {
        #expect(SpaceDropping.target(at: CGPoint(x: 50, y: 485), windows: [a, b]) == .sidebar(window: "A", index: 0))
        #expect(SpaceDropping.target(at: CGPoint(x: 50, y: 450), windows: [a, b]) == .sidebar(window: "A", index: 1))
        #expect(SpaceDropping.target(at: CGPoint(x: 50, y: 100), windows: [a, b]) == .sidebar(window: "A", index: 3))
    }

    @Test func aPointerInATabStripPicksTheGapBesideATab() {
        let tabs = DropWindow(
            id: "T", frame: CGRect(x: 0, y: 0, width: 600, height: 500),
            tabStrip: CGRect(x: 0, y: 470, width: 600, height: 26),
            tabs: [CGRect(x: 0, y: 470, width: 100, height: 26), CGRect(x: 104, y: 470, width: 100, height: 26), CGRect(x: 208, y: 470, width: 100, height: 26)]
        )
        #expect(SpaceDropping.target(at: CGPoint(x: 10, y: 480), windows: [tabs]) == .tabs(window: "T", index: 0))
        #expect(SpaceDropping.target(at: CGPoint(x: 160, y: 480), windows: [tabs]) == .tabs(window: "T", index: 2))
        #expect(SpaceDropping.target(at: CGPoint(x: 550, y: 480), windows: [tabs]) == .tabs(window: "T", index: 3))
        #expect(SpaceDropping.target(at: CGPoint(x: 300, y: 200), windows: [tabs]) == .window("T"))
    }

    @Test func noBarShowsBesideTheDraggedItem() {
        let held = ["a", "b", "c"]
        #expect(SpaceDropping.barIndex(0, held: held, dragged: "b") == 0)
        #expect(SpaceDropping.barIndex(1, held: held, dragged: "b") == nil)
        #expect(SpaceDropping.barIndex(2, held: held, dragged: "b") == nil)
        #expect(SpaceDropping.barIndex(3, held: held, dragged: "b") == 3)
        #expect(SpaceDropping.barIndex(3, held: held, dragged: "c") == nil)
        // A space dragged in from another window has no place here: every gap shows.
        #expect(SpaceDropping.barIndex(1, held: held, dragged: "z") == 1)
    }

    @Test func aWindowBodyTakesTheSpaceAtTheEndAndTheDesktopOpensAWindow() {
        #expect(SpaceDropping.target(at: CGPoint(x: 900, y: 250), windows: [a, b]) == .window("B"))
        #expect(SpaceDropping.target(at: CGPoint(x: 400, y: 250), windows: [a, b]) == .window("A"))
        #expect(SpaceDropping.target(at: CGPoint(x: 650, y: 250), windows: [a, b]) == .desktop)
    }

    @Test func theFrontWindowWinsWhereWindowsOverlap() {
        let over = DropWindow(id: "C", frame: CGRect(x: 100, y: 100, width: 300, height: 300))
        #expect(SpaceDropping.target(at: CGPoint(x: 150, y: 150), windows: [over, a]) == .window("C"))
        #expect(SpaceDropping.target(at: CGPoint(x: 150, y: 150), windows: [a, over]) == .sidebar(window: "A", index: 3))
    }

    @Test func thePopoverIsInFrontAndItsCardsAreTheTargets() {
        let popover = DropPopover(frame: CGRect(x: 0, y: 0, width: 300, height: 400), cards: [(window: "A", frame: CGRect(x: 10, y: 250, width: 280, height: 100)), (window: nil, frame: CGRect(x: 10, y: 100, width: 280, height: 100))])
        #expect(SpaceDropping.target(at: CGPoint(x: 100, y: 300), windows: [a], popover: popover) == .card(window: "A"))
        #expect(SpaceDropping.target(at: CGPoint(x: 100, y: 150), windows: [a], popover: popover) == .card(window: nil))
        // Between the cards: the nearer one.
        #expect(SpaceDropping.target(at: CGPoint(x: 100, y: 215), windows: [a], popover: popover) == .card(window: nil))
    }

    @Test func theHintNamesWhatReleasingDoes() {
        let title: (String) -> String = { _ in "Work" }
        #expect(SpaceDropping.hint(.desktop, space: "wheel", windowTitle: title) == "Release to open “wheel” in a little window")
        #expect(SpaceDropping.hint(.card(window: nil), space: "wheel", windowTitle: title) == "Run “wheel” in the background, with no window")
        #expect(SpaceDropping.hint(.sidebar(window: "A", index: 0), space: "wheel", windowTitle: title) == "Place “wheel” here in “Work”")
        #expect(SpaceDropping.hint(.window("A"), space: "wheel", windowTitle: title) == "Move “wheel” to “Work”")
    }
}
