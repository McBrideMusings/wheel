import Testing
@testable import WheelKit

struct NodeShapeTests {
    private func tree(url: String) -> Node {
        let browser = Node.pane(id: "p2", kind: .browser(url: url))
        let term = Node.pane(id: "p1", kind: .terminal(term: "t1", agent: TermAgent(name: "claude", session: "s", context: ContextUse(used: UInt64(url.count), size: 100))))
        return .split(id: "x1", axis: .horizontal, ratio: 0.5, a: term, b: browser)
    }

    @Test func aStatusChangeKeepsTheShape() {
        #expect(tree(url: "https://a").sameShape(as: tree(url: "https://bb")))
        #expect(tree(url: "https://a") != tree(url: "https://bb"))
    }

    @Test func aClosedPaneChangesTheShape() {
        let one = Node.pane(id: "p1", kind: .empty)
        #expect(!tree(url: "https://a").sameShape(as: one))
    }

    @Test func aMovedDividerChangesTheShape() {
        let a = Node.split(id: "x1", axis: .vertical, ratio: 0.5, a: .pane(id: "p1", kind: .empty), b: .pane(id: "p2", kind: .empty))
        let b = Node.split(id: "x1", axis: .vertical, ratio: 0.7, a: .pane(id: "p1", kind: .empty), b: .pane(id: "p2", kind: .empty))
        #expect(!a.sameShape(as: b))
    }
}
