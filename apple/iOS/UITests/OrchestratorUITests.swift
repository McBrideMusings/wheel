import XCTest

/// Drives the phone's Orchestrator mode with real taps against a wheeld that
/// `.claude/skills/project-verify/tools/ios-orchestrator.sh` set up: one space holding one agent pane, and no
/// Orchestrator space until the test taps Start. The test writes `<sync>/step-<name>` after each step and waits for
/// `<sync>/ack-<name>`; shots go to `<shots>/orchestrator-<state>-<theme>.png`.
final class OrchestratorUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() { continueAfterFailure = false }

    func testOrchestratorMode() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["title"].waitForExistence(timeout: 40))
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 20))

        // With no Orchestrator space, the mode says so and offers Start.
        pick("Orchestrator")
        XCTAssertTrue(any("no orchestrator").waitForExistence(timeout: 5), "Orchestrator mode shows its empty state")
        let start = app.buttons["orchestrator start"]
        XCTAssertTrue(start.waitForExistence(timeout: 5))
        XCTAssertTrue(start.isEnabled, "Start is offered: the running wheeld lists orchestrator_start")
        shot("empty")
        sync("empty")

        // 2. Start makes the Orchestrator space; its first open pins the global orchestrator as the sheet.
        start.tap()
        XCTAssertTrue(sheet.waitForExistence(timeout: 30), "the first open pins the orchestrator as the sheet")
        XCTAssertFalse(any("no orchestrator").exists)
        XCTAssertEqual(app.buttons["title"].label.contains("Orchestrator"), true, "the title names the Orchestrator space: \(app.buttons["title"].label)")
        shot("pinned")
        sync("pinned")

        // 1. Full screen under the top bar, no modal: the bar's title sits above the body, the body reaches the
        // bottom edge, and no presented sheet covers it.
        let window = app.windows.firstMatch.frame
        let bar = app.buttons["title"].frame
        let handle = app.otherElements["page handle"].frame
        XCTAssertLessThan(bar.maxY, sheet.frame.minY, "the pin sheet lies under the top bar")
        XCTAssertGreaterThan(handle.maxY, window.maxY - 60, "the body reaches the bottom edge: \(handle) in \(window)")
        XCTAssertFalse(app.otherElements["PopoverDismissRegion"].exists, "no modal is presented")
        sync("fullscreen")

        // 4. The Orchestrator space is in no Spaces mode list: the title menu lists the other space and session only.
        pick("Spaces")
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 5))
        app.buttons["title"].tap()
        XCTAssertTrue(app.buttons["space wheel"].waitForExistence(timeout: 10), "the title menu lists the Spaces mode space")
        XCTAssertFalse(app.buttons["space Orchestrator"].exists, "the title menu lists no Orchestrator space")
        XCTAssertFalse(app.buttons["session Orchestrator"].exists, "the session bar lists no Orchestrator session")
        shot("lists")
        app.swipeDown(velocity: .fast)
        XCTAssertTrue(waitFor { !self.app.buttons["space wheel"].isHittable })
        sync("lists")

        // 3. Unpinning the orchestrator there keeps it unpinned on the next open, and across a relaunch.
        pick("Orchestrator")
        XCTAssertTrue(sheet.waitForExistence(timeout: 5), "the orchestrator is still pinned on the second open")
        // The element named `pin sheet menu` spans the header, so the tap goes to the name inside it; a row is found by
        // its title, since the header's own button carries the same name as its value.
        app.buttons["pin sheet menu"].buttons.firstMatch.tap()
        let unpin = app.buttons.matching(NSPredicate(format: "label == 'Unpin' AND identifier != 'pin sheet menu'")).firstMatch
        XCTAssertTrue(unpin.waitForExistence(timeout: 5))
        unpin.tap()
        XCTAssertTrue(waitFor { !self.sheet.exists }, "unpinning removes the sheet")
        pick("Spaces")
        pick("Orchestrator")
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 5))
        Thread.sleep(forTimeInterval: 1)
        XCTAssertFalse(sheet.exists, "the orchestrator stays unpinned on the next open")
        shot("unpinned")
        app.terminate()
        // The pairing code is spent; the relaunch connects with the credential the first launch saved.
        app.launchEnvironment.removeValue(forKey: "WHEEL_PAIR_JSON")
        app.launch()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Orchestrator'")).firstMatch.waitForExistence(timeout: 40), "Orchestrator mode restored")
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 20))
        Thread.sleep(forTimeInterval: 2)
        XCTAssertFalse(sheet.exists, "the orchestrator stays unpinned after a relaunch")
        sync("unpinned")
        pick("Spaces")
    }

    private var sheet: XCUIElement { app.otherElements["pin sheet"] }

    private func any(_ id: String) -> XCUIElement { app.descendants(matching: .any)[id] }

    /// Pick `title` from the mode menu and wait for the button to show it.
    private func pick(_ title: String) {
        app.buttons["mode"].tap()
        let row = app.buttons.matching(NSPredicate(format: "label == %@ AND identifier != 'agents' AND identifier != 'mode'", title)).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 5), "the mode menu has \(title)")
        row.tap()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == %@", title)).firstMatch.waitForExistence(timeout: 5), "the button shows \(title)")
    }

    private func waitFor(_ condition: @escaping () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(5)
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
            try? XCUIScreen.main.screenshot().pngRepresentation.write(to: URL(fileURLWithPath: "\(dir)/orchestrator-\(name)-\(theme).png"))
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
