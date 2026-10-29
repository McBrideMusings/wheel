import CoreGraphics
import Testing
@testable import WheelKit

@Suite struct PaneDropTests {
    private func pane(_ id: String) -> Node { .pane(id: id, kind: .empty) }
    private func split(_ axis: Axis, _ a: Node, _ b: Node) -> Node { .split(id: "x", axis: axis, ratio: 0.5, a: a, b: b) }
    private let frame = CGRect(x: 100, y: 0, width: 400, height: 200)

    @Test func theMiddleIsACentreDropAndEachEdgeNamesItsSide() {
        #expect(PaneDragging.zone(at: CGPoint(x: 300, y: 100), in: frame) == .center)
        #expect(PaneDragging.zone(at: CGPoint(x: 110, y: 100), in: frame) == .edge(.left))
        #expect(PaneDragging.zone(at: CGPoint(x: 490, y: 100), in: frame) == .edge(.right))
        #expect(PaneDragging.zone(at: CGPoint(x: 300, y: 10), in: frame) == .edge(.up))
        #expect(PaneDragging.zone(at: CGPoint(x: 300, y: 190), in: frame) == .edge(.down))
        // 12 from the left (3%) against 40 from the top (20%): the nearer edge wins.
        #expect(PaneDragging.zone(at: CGPoint(x: 112, y: 40), in: frame) == .edge(.left))
    }

    @Test func aDropIsNilOverTheDraggedPaneAndOutsideEveryPane() {
        let grid = GridLayout(split(.horizontal, pane("p1"), pane("p2")), in: CGRect(x: 0, y: 0, width: 408, height: 100))
        #expect(PaneDragging.drop(at: CGPoint(x: 300, y: 50), among: grid.panes, dragging: "p1") == PaneDrop(target: "p2", zone: .center))
        #expect(PaneDragging.drop(at: CGPoint(x: 100, y: 50), among: grid.panes, dragging: "p1") == nil)
        #expect(PaneDragging.drop(at: CGPoint(x: 900, y: 50), among: grid.panes, dragging: "p1") == nil)
    }

    @Test func aPaneFromAnotherSpaceAlwaysTakesTheNearestSide() {
        let grid = GridLayout(split(.horizontal, pane("p1"), pane("p2")), in: CGRect(x: 0, y: 0, width: 408, height: 100))
        // p2 spans x 204..408: x 300 is 96 from its left edge and 108 from its right, so it docks left.
        let middle = PaneDragging.away(at: CGPoint(x: 300, y: 50), among: grid.panes)
        #expect(middle?.pane == "p2" && middle?.dir == .left)
        #expect(PaneDragging.away(at: CGPoint(x: 100, y: 10), among: grid.panes)?.dir == .up)
        #expect(PaneDragging.away(at: CGPoint(x: 900, y: 50), among: grid.panes) == nil)
    }

    @Test func eachAwayTargetSaysWhatReleasingDoes() {
        let name: (String) -> String? = { $0 == "s2" ? "docs" : nil }
        let title: (String) -> String = { _ in "Main" }
        #expect(PaneAway.end(space: "s2").hint(pane: "p1", spaceName: name, windowTitle: title) == "Move “p1” to the end of “docs”")
        #expect(PaneAway.newSpace(window: "w").hint(pane: "p1", spaceName: name, windowTitle: title) == "Move “p1” into a new space in “Main”")
        #expect(PaneAway.beside(pane: "p9", dir: .left, space: "s2").hint(pane: "p1", spaceName: name, windowTitle: title) == "Dock “p1” on the left of p9 in “docs”")
    }

    @Test func theHighlightIsTheHalfOfTheTargetThePaneWouldTake() {
        #expect(PaneDragging.highlight(.center, in: frame) == frame)
        #expect(PaneDragging.highlight(.edge(.left), in: frame) == CGRect(x: 100, y: 0, width: 200, height: 200))
        #expect(PaneDragging.highlight(.edge(.right), in: frame) == CGRect(x: 300, y: 0, width: 200, height: 200))
        #expect(PaneDragging.highlight(.edge(.up), in: frame) == CGRect(x: 100, y: 0, width: 400, height: 100))
        #expect(PaneDragging.highlight(.edge(.down), in: frame) == CGRect(x: 100, y: 100, width: 400, height: 100))
    }
}
