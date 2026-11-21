import Foundation
import Testing
@testable import WheelKit

struct PinsTests {
    @Test func pinsKeepTheirOrderAndPinOnce() {
        var pins = Pins()
        pins.pin("p2", in: "s1")
        pins.pin("p1", in: "s1")
        pins.pin("p2", in: "s1")
        #expect(pins.pinned(in: "s1") == ["p2", "p1"])
        pins.unpin("p2", in: "s1")
        pins.unpin("p1", in: "s1")
        #expect(pins.bySpace.isEmpty, "a space with no pins leaves the map")
    }

    @Test func aClosedPaneLeavesThePinsAndAnUnlistedSpaceKeepsThem() throws {
        let json = #"[{"id":"s1","name":"w","cwd":"/w","root":{"node":"pane","id":"p1","kind":"terminal","term":"t1"},"backgrounded":[{"node":"pane","id":"p3","kind":"empty"}]}]"#
        let spaces = try JSONDecoder().decode([SpaceInfo].self, from: Data(json.utf8))
        let pins = Pins(["s1": ["p1", "p2", "p3"], "s9": ["p9"]])
        #expect(pins.kept(in: spaces) == Pins(["s1": ["p1", "p3"], "s9": ["p9"]]))
        #expect(Pins(["s1": ["p2"]]).kept(in: spaces).bySpace.isEmpty)
    }

    @Test func aDragEndsAtTheNearestPosition() {
        #expect(SheetPosition.nearest(to: 600, in: 640) == .full)
        #expect(SheetPosition.nearest(to: 420, in: 640) == .high)
        #expect(SheetPosition.nearest(to: 250, in: 640) == .low)
        #expect(SheetPosition.nearest(to: 40, in: 640) == .header)
        #expect(SheetPosition.nearest(to: -90, in: 640) == .header, "a drag past the bottom edge ends at the header")
    }
}
