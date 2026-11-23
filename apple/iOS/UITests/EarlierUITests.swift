import XCTest

/// Drives an agent pane's earlier conversations with real taps against a wheeld that
/// `.claude/skills/project-verify/tools/ios-earlier.sh` set up: one space whose agent pane visited C0 (not in the
/// store), C1, C2 and now C3, their ids in `<sync>/ids`. The test writes `<sync>/step-<name>` after each step and
/// waits for `<sync>/ack-<name>`; shots go to `<shots>/earlier-<state>-<theme>.png`.
final class EarlierUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() { continueAfterFailure = false }

    func testEarlierConversations() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        let syncDir = try XCTUnwrap(env["WHEEL_SYNC_DIR"], "WHEEL_SYNC_DIR")
        let ids = try String(contentsOfFile: "\(syncDir)/ids", encoding: .utf8).split(separator: " ").map(String.init)
        XCTAssertEqual(ids.count, 4, "the script wrote four conversation ids")
        let (c0, c1, c2, c3) = (ids[0], ids[1], ids[2], ids[3])
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["mode"].waitForExistence(timeout: 40))

        // The three earlier conversations, oldest first, above the current one's empty transcript.
        let first = any("earlier \(c0)"), one = any("earlier \(c1)"), two = any("earlier \(c2)")
        XCTAssertTrue(two.waitForExistence(timeout: 30), "C2's divider is drawn")
        XCTAssertTrue(one.exists, "C1's divider is drawn")
        XCTAssertTrue(first.exists, "the first conversation's divider is drawn")
        XCTAssertFalse(any("earlier \(c3)").exists, "the current conversation has no divider")
        XCTAssertLessThan(first.frame.minY, one.frame.minY, "the first conversation sits above C1")
        XCTAssertLessThan(one.frame.minY, two.frame.minY, "C1 sits above C2")
        // Each divider is titled by its conversation's first prompt once read; one the store lacks says so.
        XCTAssertTrue(app.staticTexts["fix the flaky restore test"].waitForExistence(timeout: 15), "C1's divider carries its first prompt")
        XCTAssertTrue(app.staticTexts["add resume to the agent pane"].waitForExistence(timeout: 15), "C2's divider carries its first prompt")
        XCTAssertTrue(app.staticTexts["The restore test waited on a timer; it now waits on the save."].exists, "C1's reply is drawn under its divider")
        XCTAssertTrue(app.staticTexts["Conversation no longer in the agent's store"].waitForExistence(timeout: 15), "the first conversation is missing from the store")
        XCTAssertFalse(app.buttons["resume \(c0)"].isEnabled, "a conversation the store lacks cannot be resumed")
        XCTAssertTrue(app.buttons["resume \(c1)"].isEnabled, "C1 can be resumed")
        shot("dividers")
        sync("dividers")

        // Resume puts the agent back in C1: its divider goes, and C3, which the pane left, gets one.
        app.buttons["resume \(c1)"].tap()
        XCTAssertTrue(any("earlier \(c3)").waitForExistence(timeout: 30), "C3 is now an earlier conversation")
        XCTAssertTrue(waitFor { !self.any("earlier \(c1)").exists }, "C1 is current again and has no divider")
        XCTAssertLessThan(any("earlier \(c2)").frame.minY, any("earlier \(c3)").frame.minY, "C2 sits above C3")
        shot("resumed")
        sync("resumed")
    }

    private func any(_ id: String) -> XCUIElement { app.descendants(matching: .any)[id].firstMatch }

    private func waitFor(_ condition: @escaping () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(10)
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
            try? XCUIScreen.main.screenshot().pngRepresentation.write(to: URL(fileURLWithPath: "\(dir)/earlier-\(name)-\(theme).png"))
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

    /// The system asks to send notifications on the first launch; its alert sits over the app until answered.
    private func allowNotifications() {
        let allow = XCUIApplication(bundleIdentifier: "com.apple.springboard").alerts.buttons["Allow"]
        if allow.waitForExistence(timeout: 10) { allow.tap() }
    }
}
