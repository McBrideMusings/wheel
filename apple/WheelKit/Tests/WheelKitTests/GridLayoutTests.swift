import CoreGraphics
import Testing
@testable import WheelKit

struct GridLayoutTests {
    /// 0.3 of a 408-wide horizontal split, less the 8pt gap, gives the left pane 120 and the right 280.
    @Test func splitsTheAreaByRatioAroundTheGap() {
        let root = Node.split(id: "x1", axis: .horizontal, ratio: 0.3,
                              a: .pane(id: "p1", kind: .empty),
                              b: .split(id: "x2", axis: .vertical, ratio: 0.5, a: .pane(id: "p2", kind: .empty), b: .pane(id: "p3", kind: .empty)))
        let layout = GridLayout(root, in: CGRect(x: 0, y: 0, width: 408, height: 208))
        #expect(layout.panes.map(\.id) == ["p1", "p2", "p3"])
        #expect(layout.panes[0].frame == CGRect(x: 0, y: 0, width: 120, height: 208))
        #expect(layout.panes[1].frame == CGRect(x: 128, y: 0, width: 280, height: 100))
        #expect(layout.panes[2].frame == CGRect(x: 128, y: 108, width: 280, height: 100))
        #expect(layout.dividers.first { $0.id == "x1" }?.frame == CGRect(x: 120, y: 0, width: 8, height: 208))

        let dragged = GridLayout(root, in: CGRect(x: 0, y: 0, width: 408, height: 208), ratios: ["x1": 0.5])
        #expect(dragged.panes[0].frame.width == 200)
    }

    @Test func aDragPositionBecomesARatioTheDaemonAccepts() throws {
        let root = Node.split(id: "x1", axis: .horizontal, ratio: 0.5, a: .pane(id: "p1", kind: .empty), b: .pane(id: "p2", kind: .empty))
        let divider = try #require(GridLayout(root, in: CGRect(x: 0, y: 0, width: 408, height: 100)).dividers.first)
        #expect(GridLayout.ratio(at: CGPoint(x: 104, y: 50), in: divider) == 0.25)
        #expect(GridLayout.ratio(at: CGPoint(x: 0, y: 50), in: divider) == 0.1)
        #expect(GridLayout.ratio(at: CGPoint(x: 400, y: 50), in: divider) == 0.9)
    }

    private let minimum = CGSize(width: 100, height: 60)

    @Test func aDividerStopsWhereASideReachesTheMinimum() throws {
        let root = Node.split(id: "x1", axis: .horizontal, ratio: 0.5, a: .pane(id: "p1", kind: .empty), b: .pane(id: "p2", kind: .empty))
        let rect = CGRect(x: 0, y: 0, width: 408, height: 200)
        let dragged = GridLayout(root, in: rect, ratios: ["x1": 0.02], minimum: minimum)
        #expect(dragged.panes[0].frame.width == 100)
        let divider = try #require(dragged.dividers.first)
        #expect(GridLayout.ratio(at: CGPoint(x: 0, y: 50), in: divider) == 0.25)
        #expect(GridLayout.ratio(at: CGPoint(x: 408, y: 50), in: divider) == 0.75)
    }

    @Test func aSplitHoldsTheMinimumOfEveryPaneInsideIt() {
        let root = Node.split(id: "x1", axis: .horizontal, ratio: 0.1, a: .pane(id: "p1", kind: .empty),
                              b: .split(id: "x2", axis: .horizontal, ratio: 0.5, a: .pane(id: "p2", kind: .empty), b: .pane(id: "p3", kind: .empty)))
        let layout = GridLayout(root, in: CGRect(x: 0, y: 0, width: 1000, height: 200), ratios: ["x1": 0.9], minimum: minimum)
        // The right side holds two panes, so it keeps 100 + 8 + 100.
        #expect(layout.panes[1].frame.width >= 100 && layout.panes[2].frame.width >= 100)
    }

    /// A row of panes whose minimums an area fits exactly leaves `needA / total` and
    /// `1 - needB / total` equal on paper but a rounding step apart in floating point; the range
    /// must never invert, whatever width the window has.
    @Test func anAreaThatFitsTheMinimumsExactlyNeverInvertsTheRange() {
        var root = Node.pane(id: "p0", kind: .empty)
        for i in 1 ..< 9 {
            root = .split(id: "x\(i)", axis: .horizontal, ratio: 0.2535, a: root, b: .pane(id: "p\(i)", kind: .empty))
        }
        for width in stride(from: 1900.0, through: 2600.0, by: 0.01) {
            let layout = GridLayout(root, in: CGRect(x: 0, y: 0, width: width, height: 600), minimum: GridLayout.minimumPane)
            #expect(layout.dividers.count == 8)
        }
    }

    @Test func tooSmallAnAreaSharesItByNeed() {
        let root = Node.split(id: "x1", axis: .vertical, ratio: 0.5, a: .pane(id: "p1", kind: .empty), b: .pane(id: "p2", kind: .empty))
        let layout = GridLayout(root, in: CGRect(x: 0, y: 0, width: 300, height: 108), minimum: minimum)
        #expect(layout.panes[0].frame.height == 50)
        #expect(layout.panes[1].frame.height == 50)
    }
}
