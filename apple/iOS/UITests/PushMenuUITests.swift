import XCTest

/// Drives the pane menu's Notifications item with real taps against a wheeld that
/// `.claude/skills/project-verify/tools/ios-pane-push.sh` set up: one space holding one pane. The test writes
/// `<sync>/step-<name>` after each step and waits for `<sync>/ack-<name>`; shots of the open menu go to
/// `<shots>/push-<state>.png`.
final class PushMenuUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() { continueAfterFailure = false }

    func testPushToggle() throws {
        app = .paired(try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON"))
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 40))

        // A pane that sends pushes offers to turn them off; choosing it makes the pane quiet in wheeld.
        choose("Turn Notifications Off", shot: "on")
        sync("off")
        // The layout wheeld sends back flips the item.
        choose("Turn Notifications On", shot: "off")
        sync("on")
        app.buttons["Pane menu"].tap()
        XCTAssertTrue(app.buttons["Turn Notifications Off"].waitForExistence(timeout: 5), "the pane pushes again")
    }

    /// Open the ⋯, shoot the open menu, then tap `title`.
    private func choose(_ title: String, shot name: String) {
        app.buttons["Pane menu"].tap()
        let item = app.buttons[title]
        XCTAssertTrue(item.waitForExistence(timeout: 10), "the pane menu offers \(title)")
        shot(name)
        item.tap()
    }

    private func shot(_ name: String) {
        guard let dir = env["WHEEL_SHOTS"] else { return }
        try? XCUIScreen.main.screenshot().pngRepresentation.write(to: URL(fileURLWithPath: "\(dir)/push-\(name).png"))
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
