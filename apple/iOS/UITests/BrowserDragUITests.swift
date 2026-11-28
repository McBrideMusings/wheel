import XCTest

/// Drives a browser pane with real drags against a wheeld that
/// `.claude/skills/project-verify/tools/ios-browser-drag.sh` set up: one space whose first grid
/// pane is a browser on a page that records how far a finger moved, and a second grid pane that is
/// an agent. Sideways drags on the browser page reach the page and leave the pane where it is; a
/// drag along the top bar, and one on the agent page, still change pane. At each step the test
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
        app = .paired(pair)
        app.launchEnvironment["WHEEL_NOTIFICATIONS"] = "off"
        app.launch()
        XCTAssertTrue(app.buttons["mode"].waitForExistence(timeout: 40), "the title shows once paired")
        expectPages("[grid] grid")
        XCTAssertTrue(app.buttons["browser-home"].waitForExistence(timeout: 20), "the browser row shows")
        sync("loaded")

        // Right to left across the middle of the page: the page takes it, the pane stays.
        drag(from: 0.85, to: 0.15, dy: 0.5)
        holdPages("[grid] grid")
        sync("dragged-left")

        // Left to right from the screen's edge: no pane change and no going back.
        drag(from: 0.01, to: 0.85, dy: 0.5)
        holdPages("[grid] grid")
        sync("dragged-from-edge")

        // A sideways drag on the top bar, between the title and the panes button, still changes pane: the bar is the
        // strip a page cannot take.
        let panes = app.buttons["panes"], mode = app.buttons["mode"]
        XCTAssertTrue(panes.exists, "the panes button shows")
        let row = mode.frame.midY
        let startX = panes.frame.minX - 12
        XCTAssertGreaterThan(startX, mode.frame.maxX, "the drag starts right of the title: \(startX) vs \(mode.frame)")
        let origin = app.coordinate(withNormalizedOffset: .zero)
        origin.withOffset(CGVector(dx: startX, dy: row))
            .press(forDuration: 0.05, thenDragTo: origin.withOffset(CGVector(dx: startX - 140, dy: row)),
                   withVelocity: .fast, thenHoldForDuration: 0)
        expectPages("grid [grid]")
        sync("top-bar")

        // A page that is not a browser still swipes.
        drag(from: 0.15, to: 0.85, dy: 0.4)
        expectPages("[grid] grid")
        sync("agent-swipe")
    }

    /// A finger across the screen at height `dy` (0 is the top), from `from` to `to` across it.
    private func drag(from: CGFloat, to: CGFloat, dy: CGFloat) {
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: from, dy: dy))
        let end = app.coordinate(withNormalizedOffset: CGVector(dx: to, dy: dy))
        start.press(forDuration: 0.05, thenDragTo: end, withVelocity: .fast, thenHoldForDuration: 0)
    }

    private func pagesValue() -> String {
        app.descendants(matching: .any)["pages"].value as? String ?? "nil"
    }

    private func expectPages(_ value: String) {
        let pages = app.descendants(matching: .any)["pages"]
        XCTAssertTrue(pages.waitForExistence(timeout: 10), "the pages element is behind the top bar")
        XCTAssertTrue(waitUntil { pagesValue() == value }, "pages read '\(value)', have '\(pagesValue())'")
    }

    /// The pages read `value` now and still do after the pager would have settled on another page.
    private func holdPages(_ value: String) {
        let deadline = Date().addingTimeInterval(2)
        while Date() < deadline {
            XCTAssertEqual(pagesValue(), value, "the pane did not change")
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
