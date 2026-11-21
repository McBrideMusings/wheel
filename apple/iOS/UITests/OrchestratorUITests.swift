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

        // Opening the mode with no global orchestrator starts one with no tap (wheel-eg4e.19.23); there is no Start.
        sync("before")
        pick("Orchestrator")
        XCTAssertFalse(app.buttons["orchestrator start"].exists, "no Start button")

        // 2. The start makes the Orchestrator space; its first open pins the global orchestrator as the sheet.
        XCTAssertTrue(sheet.waitForExistence(timeout: 30), "the first open pins the orchestrator as the sheet")
        XCTAssertFalse(any("no orchestrator").exists)
        XCTAssertEqual(title.label.contains("Orchestrator"), true, "the title names the Orchestrator space: \(title.label)")
        shot("pinned")
        sync("pinned")

        // One global orchestrator: the title is a label with no menu, there is no eye, and the handle's swipe up opens
        // nothing. Search and the ⋯ hold the places they have in Spaces mode.
        XCTAssertFalse(app.buttons["title"].exists, "the title is no button in Orchestrator mode")
        XCTAssertFalse(app.buttons["agents"].exists, "Orchestrator mode has no eye")
        XCTAssertFalse(app.buttons["search"].exists, "Orchestrator mode has no search (wheel-eg4e.19.27)")
        let here = (app.buttons["mode"].frame, app.buttons["Pane menu"].frame)
        title.tap()
        Thread.sleep(forTimeInterval: 1)
        XCTAssertFalse(app.buttons["space wheel"].exists, "a tap on the title opens no menu")
        let handle = app.otherElements["page handle"]
        handle.press(forDuration: 0.05, thenDragTo: app.buttons["mode"])
        Thread.sleep(forTimeInterval: 1)
        XCTAssertFalse(app.buttons["space wheel"].exists, "a swipe up on the handle opens no sheet")
        shot("chrome")
        sync("chrome")

        // The composer's keyboard button (wheel-eg4e.19.25): it shows while the keyboard is up and hides it.
        XCTAssertFalse(app.buttons["hide keyboard"].exists, "no keyboard button while the keyboard is down")
        sheet.textViews.firstMatch.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5), "the composer raised the keyboard")
        let hide = app.buttons["hide keyboard"]
        XCTAssertTrue(hide.waitForExistence(timeout: 5), "the composer shows the keyboard button")
        // While the keyboard is up the sheet sits full, so the chat shows above the composer (wheel-eg4e.19.30).
        let sheetHeader = app.otherElements["pin sheet header"]
        XCTAssertTrue(waitFor { sheetHeader.frame.minY < self.title.frame.maxY + 24 }, "the sheet opens full while typing: \(sheetHeader.frame)")
        shot("keyboard")
        hide.tap()
        XCTAssertTrue(waitFor { !self.app.keyboards.firstMatch.exists }, "the button hid the keyboard")
        XCTAssertTrue(waitFor { sheetHeader.frame.minY > self.title.frame.maxY + 100 }, "the sheet goes back down with the keyboard: \(sheetHeader.frame)")
        XCTAssertFalse(app.buttons["hide keyboard"].exists, "the button goes with the keyboard")

        // 1. Full screen under the top bar, no modal: the bar's title sits above the body, the body reaches the
        // bottom edge, and no presented sheet covers it.
        let window = app.windows.firstMatch.frame
        let bar = title.frame
        let handleFrame = handle.frame
        XCTAssertLessThan(bar.maxY, sheet.frame.minY, "the pin sheet lies under the top bar")
        // The pinned sheet reaches the bottom edge, and the pages and their handle end above its header.
        XCTAssertGreaterThan(sheet.frame.maxY, window.maxY - 60, "the body reaches the bottom edge: \(sheet.frame) in \(window)")
        XCTAssertLessThanOrEqual(handleFrame.maxY, sheet.frame.maxY, "the handle ends inside the body: \(handleFrame)")
        XCTAssertFalse(app.otherElements["PopoverDismissRegion"].exists, "no modal is presented")
        sync("fullscreen")

        // 4. The Orchestrator space is in no Spaces mode list: the title menu lists the other space and session only.
        pick("Spaces")
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["agents"].exists, "Spaces mode keeps the eye")
        XCTAssertEqual(app.buttons["mode"].frame, here.0, "the mode button holds its place across the modes")
        XCTAssertEqual(app.buttons["Pane menu"].frame, here.1, "the ⋯ holds its place across the modes")
        app.buttons["title"].tap()
        XCTAssertTrue(app.buttons["space wheel"].waitForExistence(timeout: 10), "the title menu lists the Spaces mode space")
        XCTAssertFalse(app.buttons["space Orchestrator"].exists, "the title menu lists no Orchestrator space")
        XCTAssertFalse(app.buttons["session Orchestrator"].exists, "the session bar lists no Orchestrator session")
        shot("lists")
        app.swipeDown(velocity: .fast)
        XCTAssertTrue(waitFor { !self.app.buttons["space wheel"].isHittable })
        sync("lists")

        // 3 (wheel-eg4e.19.27). The person only talks to the orchestrator: it stays pinned, its header has no picker or
        // Unpin, and the ⋯ holds Settings alone.
        pick("Orchestrator")
        XCTAssertTrue(sheet.waitForExistence(timeout: 5), "the orchestrator is pinned on the second open")
        XCTAssertFalse(app.buttons["pin sheet menu"].exists, "no picker or Unpin over the orchestrator")
        XCTAssertTrue(any("pin sheet title").exists, "the header names the orchestrator")
        app.buttons["Pane menu"].tap()
        XCTAssertTrue(app.buttons["Settings"].waitForExistence(timeout: 5), "the ⋯ holds Settings")
        for row in ["Split Right", "Split Down", "Pin", "Close", "Pane info"] {
            XCTAssertFalse(app.buttons[row].exists, "the ⋯ offers no \(row) in Orchestrator mode")
        }
        shot("menu")
        app.buttons["Settings"].tap()
        XCTAssertTrue(app.buttons["Done"].waitForExistence(timeout: 5), "Settings opened")
        app.buttons["Done"].tap()
        pick("Spaces")
        pick("Orchestrator")
        XCTAssertTrue(sheet.waitForExistence(timeout: 5), "still pinned after a mode switch")
        app.terminate()
        // The pairing code is spent; the relaunch connects with the credential the first launch saved.
        app.launchEnvironment.removeValue(forKey: "WHEEL_PAIR_JSON")
        app.launch()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Orchestrator'")).firstMatch.waitForExistence(timeout: 40), "Orchestrator mode restored")
        XCTAssertTrue(sheet.waitForExistence(timeout: 20), "still pinned after a relaunch")
        shot("pinned-relaunch")
        sync("unpinned")
        pick("Spaces")
    }

    private var sheet: XCUIElement { app.otherElements["pin sheet"] }
    private var title: XCUIElement { app.descendants(matching: .any)["title"].firstMatch }

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
