import Testing
@testable import WheelKit

struct PaneStackTests {
    private func space(panes: Int) -> SpaceInfo {
        var node = Node.pane(id: "p0", kind: .terminal(term: "t0"))
        for i in 1..<max(panes, 1) {
            node = .split(id: "s\(i)", axis: .horizontal, ratio: 0.5, a: node, b: .pane(id: "p\(i)", kind: .empty))
        }
        return SpaceInfo(id: "s", name: "s", cwd: "/", root: node, focus: "p0")
    }

    @Test func oneTwoThreeAndFivePanesDrawOneTwoThreeAndFourLayers() {
        #expect([1, 2, 3, 5].map { space(panes: $0).stackLayers } == [1, 2, 3, 4])
    }

    @Test func theFrontLayerShowsTheFirstPanesKind() {
        #expect(space(panes: 3).firstPaneKind == .terminal(term: "t0"))
    }
}
