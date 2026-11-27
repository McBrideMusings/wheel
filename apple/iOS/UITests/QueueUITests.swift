import XCTest

/// Drives Orchestrator mode's queue tray and sheet with real taps against a wheeld that
/// `.claude/skills/project-verify/tools/ios-queue.sh` set up: a space on the orchestrator folder holding an agent pane
/// and a backgrounded Queue pane (`wheelhouse`'s `queue`), and a queue of a P0 bug, a P2 question and a done item, their
/// ids in `<sync>/ids`. The test writes `<sync>/step-<name>` after each step and waits for `<sync>/ack-<name>`; shots go
/// to `<shots>/queue-<state>-<theme>.png`.
final class QueueUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() { continueAfterFailure = false }

    func testQueueTray() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        let syncDir = try XCTUnwrap(env["WHEEL_SYNC_DIR"], "WHEEL_SYNC_DIR")
        let ids = try String(contentsOfFile: "\(syncDir)/ids", encoding: .utf8).split(separator: " ").map(String.init)
        XCTAssertEqual(ids.count, 3, "the script wrote three item ids")
        let (bug, question, done) = (ids[0], ids[1], ids[2])
        app = .paired(pair)
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["mode"].waitForExistence(timeout: 40))

        // Spaces mode has the panes button and no tray.
        XCTAssertTrue(app.buttons["panes"].waitForExistence(timeout: 20))
        XCTAssertFalse(app.buttons["queue"].exists, "Spaces mode has no queue tray")
        pick("Orchestrator")

        // The tray sits between the title and the panes button: two wait on the person, the most urgent a P0, so the badge is red.
        let tray = app.buttons["queue"]
        XCTAssertTrue(tray.waitForExistence(timeout: 30), "Orchestrator mode shows the queue tray")
        XCTAssertTrue(waitFor { tray.value as? String == "2 red" }, "the badge reads 2 in red: \(String(describing: tray.value))")
        XCTAssertTrue(app.buttons["panes"].exists, "the panes button stays beside the tray")
        XCTAssertFalse(app.buttons["agents"].exists || app.buttons["search"].exists, "Orchestrator mode has no eye and no search")
        XCTAssertLessThan(app.buttons["panes"].frame.maxX, app.buttons["Pane menu"].frame.minX + 1, "the panes button sits left of the pane menu")
        XCTAssertLessThan(tray.frame.maxX, app.buttons["panes"].frame.minX + 1, "the tray sits left of the panes button")
        shot("tray")
        sync("tray")

        // The sheet lists each item under its group: the bug before the question in Needs you, the done item in Finished.
        tray.tap()
        XCTAssertTrue(any("queue sheet").waitForExistence(timeout: 10), "the tray opens the queue sheet")
        let needs = any("queue group needs"), review = any("queue group review"), finished = any("queue group finished")
        XCTAssertTrue(needs.waitForExistence(timeout: 10), "Needs you is listed")
        XCTAssertEqual(needs.label, "Needs you · 2")
        XCTAssertEqual(review.label, "Ready for review · 0")
        XCTAssertEqual(finished.label, "Finished · 1")
        let b = any("queue item \(bug)"), q = any("queue item \(question)"), d = any("queue item \(done)")
        XCTAssertTrue(b.exists && q.exists && d.exists, "every item is listed")
        XCTAssertLessThan(needs.frame.minY, b.frame.minY, "the bug sits under Needs you")
        XCTAssertLessThan(b.frame.minY, q.frame.minY, "the P0 bug sits above the P2 question")
        XCTAssertLessThan(q.frame.minY, review.frame.minY, "the question sits above Ready for review")
        XCTAssertLessThan(finished.frame.minY, d.frame.minY, "the done item sits under Finished")
        shot("sheet")
        sync("sheet")

        // A tap sends the item's message to the orchestrator's chat and closes the sheet.
        q.tap()
        XCTAssertTrue(waitFor { !self.any("queue sheet").exists }, "the sheet closes")
        sync("asked")

        // The script closed the bug: the badge follows the feed to one P2, gray.
        XCTAssertTrue(waitFor(30) { tray.value as? String == "1 gray" }, "the badge reads 1 in gray: \(String(describing: tray.value))")
        shot("closed")
        sync("closed")
    }

    private func any(_ id: String) -> XCUIElement { app.descendants(matching: .any)[id].firstMatch }

    /// Toggle to `title` (Spaces or Orchestrator) with a tap on the mode button, and wait for it to show it.
    private func pick(_ title: String) {
        let shown = app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == %@", title)).firstMatch
        if !shown.exists { app.buttons["mode"].tap() }
        XCTAssertTrue(shown.waitForExistence(timeout: 5), "the button shows \(title)")
    }

    private func waitFor(_ seconds: TimeInterval = 10, _ condition: @escaping () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if condition() { return true }
            Thread.sleep(forTimeInterval: 0.2)
        }
        return condition()
    }

    private func shot(_ name: String) {
        guard let dir = env["WHEEL_SHOTS"] else { return }
        let was = XCUIDevice.shared.appearance
        for (theme, look) in [("light", XCUIDevice.Appearance.light), ("dark", .dark)] {
            XCUIDevice.shared.appearance = look
            Thread.sleep(forTimeInterval: 2.0)
            try? XCUIScreen.main.screenshot().pngRepresentation.write(to: URL(fileURLWithPath: "\(dir)/queue-\(name)-\(theme).png"))
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
