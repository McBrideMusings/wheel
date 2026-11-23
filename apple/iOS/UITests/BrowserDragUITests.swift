import XCTest

/// Drives a browser pane with real drags against a wheeld that
/// `.claude/skills/project-verify/tools/ios-browser-drag.sh` set up: one space whose first grid
/// pane is a browser on a page that records how far a finger moved, and a second grid pane that is
/// an agent. Sideways drags on the browser page reach the page and leave the pane where it is; a
/// drag along the dots bar, and one on the agent page, still change pane. At each step the test
/// writes `<sync>/step-<name>` and waits for the script's `<sync>/ack-<name>`.
@MainActor
final class BrowserDragUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
    }

    func testBrowserKeepsSidewaysDrags() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launchEnvironment["WHEEL_NOTIFICATIONS"] = "off"
        app.launch()
        XCTAssertTrue(app.buttons["mode"].waitForExistence(timeout: 40), "the title shows once paired")
        expectDots("[grid] grid")
        XCTAssertTrue(app.buttons["browser-home"].waitForExistence(timeout: 20), "the browser row shows")
        sync("loaded")

        // Right to left across the middle of the page: the page takes it, the pane stays.
        drag(from: 0.85, to: 0.15, dy: 0.5)
        holdDots("[grid] grid")
        sync("dragged-left")

        // Left to right from the screen's edge: no pane change and no going back.
        drag(from: 0.01, to: 0.85, dy: 0.5)
        holdDots("[grid] grid")
        sync("dragged-from-edge")

        // The dots bar still changes pane.
        let handle = app.descendants(matching: .any)["page handle"]
        XCTAssertTrue(handle.exists, "the dots bar shows")
        let start = handle.coordinate(withNormalizedOffset: CGVector(dx: 0.85, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: handle.coordinate(withNormalizedOffset: CGVector(dx: 0.15, dy: 0.5)),
                    withVelocity: .fast, thenHoldForDuration: 0)
        expectDots("grid [grid]")
        sync("handle")

        // A page that is not a browser still swipes.
        drag(from: 0.15, to: 0.85, dy: 0.4)
        expectDots("[grid] grid")
        sync("agent-swipe")
    }

    /// A finger across the screen at height `dy` (0 is the top), from `from` to `to` across it.
    private func drag(from: CGFloat, to: CGFloat, dy: CGFloat) {
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: from, dy: dy))
        let end = app.coordinate(withNormalizedOffset: CGVector(dx: to, dy: dy))
        start.press(forDuration: 0.05, thenDragTo: end, withVelocity: .fast, thenHoldForDuration: 0)
    }

    private func dotsValue() -> String {
        app.descendants(matching: .any)["page dots"].value as? String ?? "nil"
    }

    private func expectDots(_ value: String) {
        let dots = app.descendants(matching: .any)["page dots"]
        XCTAssertTrue(dots.waitForExistence(timeout: 10), "the page dots show")
        XCTAssertTrue(waitUntil { dotsValue() == value }, "dots read '\(value)', have '\(dotsValue())'")
    }

    /// The dots read `value` now and still do after the pager would have settled on another page.
    private func holdDots(_ value: String) {
        let deadline = Date().addingTimeInterval(2)
        while Date() < deadline {
            XCTAssertEqual(dotsValue(), value, "the pane did not change")
            Thread.sleep(forTimeInterval: 0.2)
        }
    }

    /// Polls up to 15 s for `condition`.
    private func waitUntil(_ condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(15)
        while Date() < deadline {
            if condition() { return true }
            Thread.sleep(forTimeInterval: 0.25)
        }
        return condition()
    }

    private func sync(_ step: String) {
        guard let dir = env["WHEEL_SYNC_DIR"] else { return }
        FileManager.default.createFile(atPath: "\(dir)/step-\(step)", contents: nil)
        let ack = "\(dir)/ack-\(step)"
        let deadline = Date().addingTimeInterval(60)
        while !FileManager.default.fileExists(atPath: ack) && Date() < deadline { Thread.sleep(forTimeInterval: 0.2) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: ack), "the script acknowledged \(step)")
    }
}
