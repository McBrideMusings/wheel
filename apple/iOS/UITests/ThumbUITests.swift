import XCTest

/// Drives the Thumb navigation with real taps and swipes, against a wheeld that
/// `.claude/skills/project/tools/ios-thumb.sh` set up. The script passes `WHEEL_PAIR_JSON`,
/// `WHEEL_SYNC_DIR` and `WHEEL_SHOTS` as TEST_RUNNER_ variables; at each step the test writes
/// `<sync>/step-<name>` and waits for the script's `<sync>/ack-<name>`.
final class ThumbUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
    }

    func testThumb() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launch()
        allowNotifications()
        let title = app.buttons["title"]
        XCTAssertTrue(title.waitForExistence(timeout: 40), "the title shows once paired")

        // Story 6: the top-left shows the space and how many agents wait; the name never moves.
        let name = app.staticTexts["wheel"]
        XCTAssertTrue(name.waitForExistence(timeout: 5))
        let count = app.staticTexts["needs you count"]
        XCTAssertEqual(count.label, "", "nobody waits: the count slot is blank")
        let quietName = name.frame
        XCTAssertLessThan(quietName.minX, 60, "the space name sits at the bar's left")
        shot("chat-0")
        sync("quiet")
        XCTAssertTrue(waitUntil { count.label == "3" }, "three waiting: the count reads 3, has '\(count.label)'")
        XCTAssertEqual(name.frame.minX, quietName.minX, "the name does not move when the count appears")
        XCTAssertEqual(name.frame.minY, quietName.minY, accuracy: 0.5)
        shot("chat-3")
        sync("waiting")
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

    private func shot(_ name: String) {
        guard let dir = env["WHEEL_SHOTS"] else { return }
        let was = XCUIDevice.shared.appearance
        for (theme, look) in [("light", XCUIDevice.Appearance.light), ("dark", .dark)] {
            XCUIDevice.shared.appearance = look
            Thread.sleep(forTimeInterval: 2.0)
            let png = XCUIScreen.main.screenshot().pngRepresentation
            try? png.write(to: URL(fileURLWithPath: "\(dir)/thumb-\(name)-\(theme).png"))
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
