import CoreGraphics
import Testing
@testable import WheelKit

@Suite struct PaneDropTests {
    private func pane(_ id: String) -> Node { .pane(id: id, kind: .empty) }
    private func split(_ axis: Axis, _ a: Node, _ b: Node) -> Node { .split(id: "x", axis: axis, ratio: 0.5, a: a, b: b) }
    private func shape(_ node: Node) -> String {
        switch node {
        case let .pane(id, _, _, _): id
        case let .split(_, axis, _, a, b): "(\(shape(a))\(axis == .horizontal ? "|" : "/")\(shape(b)))"
        }
    }
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

    @Test func thePreviewMirrorsTheDaemonsSwapAndMove() {
        let tree = split(.horizontal, pane("p1"), split(.vertical, pane("p3"), pane("p4")))
        #expect(shape(tree.applying(PaneDrop(target: "p4", zone: .center), dragging: "p1")) == "(p4|(p3/p1))")
        #expect(shape(tree.applying(PaneDrop(target: "p1", zone: .edge(.left)), dragging: "p4")) == "((p4|p1)|p3)")
        #expect(shape(tree.applying(PaneDrop(target: "p1", zone: .edge(.down)), dragging: "p4")) == "((p1/p4)|p3)")
        #expect(shape(tree.applying(PaneDrop(target: "p3", zone: .edge(.up)), dragging: "p4")) == "(p1|(p4/p3))")
        #expect(tree.applying(PaneDrop(target: "p1", zone: .center), dragging: "p1") == tree)
        #expect(tree.applying(PaneDrop(target: "p9", zone: .edge(.left)), dragging: "p4") == tree)
    }
}
