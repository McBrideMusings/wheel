import XCTest

/// Drives the carousel's pages with real swipes and taps, against a wheeld that
/// `.claude/skills/project-verify/tools/ios-carousel.sh` set up: one space with two grid panes, one
/// popped-out window of two panes and two backgrounded panes. The script passes `WHEEL_PAIR_JSON`,
/// `WHEEL_SYNC_DIR`, `WHEEL_SHOTS` and `WHEEL_TRAY` (the backgrounded pane ids, comma-separated, in
/// tray order) as TEST_RUNNER_ variables; at each step the test writes `<sync>/step-<name>` and waits
/// for the script's `<sync>/ack-<name>`, so the script checks wheeld between steps.
@MainActor
final class CarouselUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
    }

    func testCarousel() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        let tray = try XCTUnwrap(env["WHEEL_TRAY"], "WHEEL_TRAY").split(separator: ",").map(String.init)
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["title"].waitForExistence(timeout: 40), "the title shows once paired")

        // Grid panes in reading order, then the popped window's: two grid pages, two popped pages with rings, then the tray page.
        expectDots("[grid] grid popped popped tray")
        shot("grid-page")
        sync("page-1")
        for (n, want) in ["grid [grid] popped popped tray", "grid grid [popped] popped tray", "grid grid popped [popped] tray"].enumerated() {
            swipeLeft()
            expectDots(want)
            if n == 1 { shot("popped-page") }
            sync("page-\(n + 2)")
        }

        // The last page lists the backgrounded panes and nothing else.
        swipeLeft()
        expectDots("grid grid popped popped [tray]")
        let list = app.descendants(matching: .any)["tray list"]
        XCTAssertTrue(list.waitForExistence(timeout: 5), "the last page is the list")
        let rows = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'tray row '"))
        XCTAssertEqual(rows.allElementsBoundByIndex.map { $0.identifier }, tray.map { "tray row \($0)" }, "the rows are the backgrounded panes, in order")
        // A row holds its icon and its title, no status dot: a StatusDot is an element
        // labelled with the status ("Needs you…", "Working", "Idle"…). The ring's look is in the
        // screenshots, which a person reads.
        let statusWords = NSPredicate(format: "label BEGINSWITH 'Needs you' OR label IN {'Stopped', 'Working', 'Done', 'Idle'}")
        for row in rows.allElementsBoundByIndex {
            XCTAssertEqual(row.descendants(matching: .any).matching(statusWords).count, 0, "\(row.identifier) has no status dot")
            XCTAssertFalse(row.label.contains("Needs you") || row.label.contains("Working") || row.label.contains("Idle"), "\(row.identifier) reads '\(row.label)'")
        }
        shot("last-page-list")
        sync("tray-list")

        // A tap opens the pane in place; Back returns to the list.
        app.buttons["tray row \(tray[0])"].tap()
        XCTAssertTrue(app.buttons["tray back"].waitForExistence(timeout: 5), "the open pane has a back control")
        XCTAssertFalse(list.exists, "the list gives way to the pane")
        expectDots("grid grid popped popped [tray]")
        shot("last-page-pane-open")
        sync("tray-open")
        app.buttons["tray back"].tap()
        XCTAssertTrue(list.waitForExistence(timeout: 5), "Back shows the list again")
        sync("tray-back")

        // The tray's mark carries the needs-you badge while a backgrounded pane needs the person.
        expectDots("grid grid popped popped [tray!]")
        shot("last-page-needs-you")
        sync("needs-you")

        // The open pane leaving the tray (the script returns it to the grid) gives way to the list.
        app.buttons["tray row \(tray[0])"].tap()
        XCTAssertTrue(app.buttons["tray back"].waitForExistence(timeout: 5))
        sync("reopened")
        XCTAssertTrue(list.waitForExistence(timeout: 10), "the open pane left the tray: the list shows")
        XCTAssertEqual(rows.allElementsBoundByIndex.map { $0.identifier }, ["tray row \(tray[1])"])
        // The returned pane is a third grid page, and the phone stays on the last page.
        expectDots("grid grid grid popped popped [tray]")
        sync("left-tray")

        // With no backgrounded pane there is no last page (the script returns the last one).
        let dots = app.descendants(matching: .any)["page dots"]
        XCTAssertTrue(waitUntil { !((dots.value as? String) ?? "tray").contains("tray") }, "no tray mark, have '\(dots.value as? String ?? "nil")'")
        XCTAssertFalse(list.exists, "the last page is gone")
        shot("no-backgrounded")
        sync("no-tray")
    }

    private func expectDots(_ value: String) {
        let dots = app.descendants(matching: .any)["page dots"]
        XCTAssertTrue(dots.waitForExistence(timeout: 10), "the page dots show")
        XCTAssertTrue(waitUntil { (dots.value as? String) == value }, "dots read '\(value)', have '\(dots.value as? String ?? "nil")'")
    }

    /// A finger across the middle of the pager, away from the composer and the title.
    private func swipe(from: CGFloat, to: CGFloat) {
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: from, dy: 0.4))
        let end = app.coordinate(withNormalizedOffset: CGVector(dx: to, dy: 0.4))
        start.press(forDuration: 0.05, thenDragTo: end, withVelocity: .fast, thenHoldForDuration: 0)
    }

    private func swipeLeft() { swipe(from: 0.85, to: 0.15) }

    /// Polls up to 15 s for `condition`.
    private func waitUntil(_ condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(15)
        while Date() < deadline {
            if condition() { return true }
            Thread.sleep(forTimeInterval: 0.25)
        }
        return condition()
    }

    /// Saves the screen as `<shots>/carousel-<name>-<light|dark>.png`.
    private func shot(_ name: String) {
        guard let dir = env["WHEEL_SHOTS"] else { return }
        let was = XCUIDevice.shared.appearance
        for (theme, look) in [("light", XCUIDevice.Appearance.light), ("dark", .dark)] {
            XCUIDevice.shared.appearance = look
            Thread.sleep(forTimeInterval: 2.0)
            let png = XCUIScreen.main.screenshot().pngRepresentation
            try? png.write(to: URL(fileURLWithPath: "\(dir)/carousel-\(name)-\(theme).png"))
        }
        XCUIDevice.shared.appearance = was
    }

    private func sync(_ step: String) {
        guard let dir = env["WHEEL_SYNC_DIR"] else { return }
        FileManager.default.createFile(atPath: "\(dir)/step-\(step)", contents: nil)
        let ack = "\(dir)/ack-\(step)"
        let deadline = Date().addingTimeInterval(60)
        while !FileManager.default.fileExists(atPath: ack) && Date() < deadline { Thread.sleep(forTimeInterval: 0.2) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: ack), "the script acknowledged \(step)")
    }

    private func allowNotifications() {
        let allow = XCUIApplication(bundleIdentifier: "com.apple.springboard").alerts.buttons["Allow"]
        if allow.waitForExistence(timeout: 10) { allow.tap() }
    }
}
