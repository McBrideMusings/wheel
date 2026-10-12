import Testing
@testable import WheelKit

struct SurfacePlanTests {
    private func space(_ id: String, focus: String, _ root: Node) -> SpaceInfo {
        SpaceInfo(id: id, name: id, cwd: "/tmp", root: root, focus: focus)
    }

    /// s1 shows p1 (t1) | p2 (empty); s2 shows p3 (t3).
    private var spaces: [SpaceInfo] {
        [space("s1", focus: "p1", .split(id: "x1", axis: .horizontal, ratio: 0.5,
                                         a: .pane(id: "p1", kind: .terminal(term: "t1")),
                                         b: .pane(id: "p2", kind: .empty))),
         space("s2", focus: "p3", .pane(id: "p3", kind: .terminal(term: "t3")))]
    }

    @Test func makesASurfaceForEveryNewTerminalAndDrawsOnlyTheActiveSpace() {
        let plan = SurfacePlan(spaces, active: "s1", existing: [], focusKey: nil)
        #expect(plan.added == ["t1", "t3"])
        #expect(plan.removed == [])
        #expect(plan.visible == ["t1": true, "t3": false])
        #expect(plan.focus == "t1")
        #expect(plan.focusKey == "p1/t1")
    }

    @Test func detachesTerminalsWhosePaneLeft() {
        let plan = SurfacePlan([spaces[0]], active: "s1", existing: ["t1", "t3", "t9"], focusKey: "p1/t1")
        #expect(plan.added == [])
        #expect(plan.removed == ["t3", "t9"])
        #expect(plan.visible == ["t1": true])
    }

    @Test func movesTheKeyboardOnlyWhenTheDaemonMovesFocus() {
        let same = SurfacePlan(spaces, active: "s1", existing: ["t1", "t3"], focusKey: "p1/t1")
        #expect(same.focus == nil, "an unchanged focus leaves the keyboard where the person put it")
        #expect(same.focusKey == "p1/t1")

        let switched = SurfacePlan(spaces, active: "s2", existing: ["t1", "t3"], focusKey: "p1/t1")
        #expect(switched.focus == "t3")
        #expect(switched.visible == ["t1": false, "t3": true])

        let toEmpty = SurfacePlan([space("s1", focus: "p2", spaces[0].root)], active: "s1", existing: ["t1"], focusKey: "p1/t1")
        #expect(toEmpty.focus == nil, "an empty pane has no surface to focus")
        #expect(toEmpty.focusKey == "p2/")
    }

    @Test func aPaneThatBecomesATerminalTakesTheKeyboard() {
        let before = space("s1", focus: "p2", .pane(id: "p2", kind: .empty))
        let after = space("s1", focus: "p2", .pane(id: "p2", kind: .terminal(term: "t5")))
        let first = SurfacePlan([before], active: "s1", existing: [], focusKey: nil)
        let plan = SurfacePlan([after], active: "s1", existing: [], focusKey: first.focusKey)
        #expect(plan.added == ["t5"])
        #expect(plan.focus == "t5")
    }

    @Test func noActiveSpaceKeepsTheLastFocusKey() {
        let plan = SurfacePlan([], active: nil, existing: ["t1"], focusKey: "p1/t1")
        #expect(plan.removed == ["t1"])
        #expect(plan.focus == nil)
        #expect(plan.focusKey == "p1/t1")

        for active in [nil, "s9"] {
            let held = SurfacePlan(spaces, active: active, existing: ["t1", "t3"], focusKey: "p3/t3")
            #expect(held.focus == nil)
            #expect(held.focusKey == "p3/t3")
            #expect(held.visible == ["t1": false, "t3": false])
        }
    }

    @Test func aKeyNamesThePaneAndTheTermItShows() {
        #expect(SurfacePlan.key(pane: "p1", in: spaces) == "p1/t1")
        #expect(SurfacePlan.key(pane: "p2", in: spaces) == "p2/")
    }
}
