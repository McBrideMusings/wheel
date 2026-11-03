import XCTest

/// Drives the title menu with real taps and long presses, against a wheeld that
/// `.claude/skills/project/tools/ios-title-menu.sh` set up: session "wheel" holding spaces wheel
/// (two agents working, two panes), alpha and beta; session "games" holding chess.
///
/// The script passes `WHEEL_PAIR_JSON` (a one-time pairing code), `WHEEL_SYNC_DIR` and `WHEEL_SHOTS`
/// as TEST_RUNNER_ variables. At each step the test writes `<sync>/step-<name>` and waits for the
/// script's `<sync>/ack-<name>`, so the script checks wheeld between steps.
final class TitleMenuUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
    }

    func testTitleMenu() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launch()
        let title = app.buttons["title"]
        XCTAssertTrue(title.waitForExistence(timeout: 40), "the title shows once paired")
        expectTitle("wheel", session: "wheel")
        shot("chat")

        // 1. Only the shown session's spaces are listed.
        title.tap()
        XCTAssertTrue(row("space wheel").waitForExistence(timeout: 5))
        XCTAssertTrue(row("space alpha").exists)
        XCTAssertTrue(row("space beta").exists)
        XCTAssertFalse(row("space chess").exists, "a space of another session is not listed")
        shot("space-menu")

        // 2. A switch draws with wheeld stopped: the script holds it with SIGSTOP between these steps.
        sync("stall")
        row("space alpha").tap()
        expectTitle("alpha", session: "wheel", timeout: 3)
        sync("switched")

        // 3. Switch session…, then back: the session's last-shown space returns.
        title.tap()
        app.buttons["switch session"].tap()
        XCTAssertTrue(row("session games").waitForExistence(timeout: 5))
        XCTAssertTrue(row("session wheel").exists)
        shot("session-menu")
        row("session games").tap()
        expectTitle("chess", session: "games")
        title.tap()
        app.buttons["switch session"].tap()
        row("session wheel").tap()
        expectTitle("alpha", session: "wheel")

        // 4. Relaunch: the same session, space and pane come back. Show wheel's second pane first.
        title.tap()
        row("space wheel").tap()
        expectTitle("wheel", session: "wheel")
        app.swipeLeft()
        sync("second-pane")
        app.terminate()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = nil
        app.launch()
        XCTAssertTrue(title.waitForExistence(timeout: 40))
        expectTitle("wheel", session: "wheel")
        sync("relaunched")

        // 5. Rename beta from its long-press menu.
        title.tap()
        row("space beta").press(forDuration: 1.0)
        XCTAssertTrue(app.buttons["Rename"].waitForExistence(timeout: 5))
        shot("long-press")
        app.buttons["Rename"].tap()
        let field = app.alerts.textFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 12) + "beta2")
        app.alerts.buttons["Rename"].tap()
        sync("renamed")

        // 6. Move beta2 to games.
        title.tap()
        XCTAssertTrue(row("space beta2").waitForExistence(timeout: 5))
        row("space beta2").press(forDuration: 1.0)
        app.buttons["Move to session…"].tap()
        XCTAssertTrue(app.buttons["games"].waitForExistence(timeout: 5))
        app.buttons["games"].tap()
        sync("moved")
        title.tap()
        XCTAssertTrue(row("space alpha").waitForExistence(timeout: 5))
        XCTAssertFalse(row("space beta2").exists, "a moved space leaves this session's list")

        // 7. New pane in wheel.
        row("space wheel").press(forDuration: 1.0)
        app.buttons["New pane"].tap()
        sync("new-pane")

        // 8. Close alpha: no agent works there, so it closes at once.
        title.tap()
        row("space alpha").press(forDuration: 1.0)
        app.buttons["Close space"].tap()
        sync("closed-alpha")

        // 9. Close wheel: two agents work there, so it asks first; Cancel keeps it, then close it.
        title.tap()
        row("space wheel").press(forDuration: 1.0)
        app.buttons["Close space"].tap()
        let question = "Close wheel? 2 agents are working. Closing ends them and every terminal in it."
        XCTAssertTrue(app.alerts.staticTexts[question].waitForExistence(timeout: 5), "the close question names the working agents")
        shot("close-confirm")
        app.alerts.buttons["Cancel"].tap()
        XCTAssertTrue(app.alerts.firstMatch.waitForNonExistence(timeout: 5), "Cancel closes the question")
        sync("kept-wheel")
        title.tap()
        row("space wheel").press(forDuration: 1.0)
        app.buttons["Close space"].tap()
        XCTAssertTrue(app.alerts.staticTexts[question].waitForExistence(timeout: 5))
        app.alerts.buttons["Close space"].tap()
        sync("closed-wheel")
        // The session is left empty: it stays shown, never another session's space.
        expectTitle("No space", session: "wheel")
        shot("empty-session")
    }

    private func row(_ id: String) -> XCUIElement { app.buttons[id] }

    private func expectTitle(_ space: String, session: String, timeout: TimeInterval = 10) {
        let label = "\(space), session \(session)"
        let shown = app.buttons.matching(NSPredicate(format: "identifier == 'title' AND label == %@", label)).firstMatch
        XCTAssertTrue(shown.waitForExistence(timeout: timeout), "title reads '\(label)', has '\(app.buttons["title"].label)'")
    }

    /// Saves the screen as `<shots>/title-menu-<name>-<light|dark>.png`.
    private func shot(_ name: String) {
        guard let dir = env["WHEEL_SHOTS"] else { return }
        let was = XCUIDevice.shared.appearance
        for (theme, look) in [("light", XCUIDevice.Appearance.light), ("dark", .dark)] {
            XCUIDevice.shared.appearance = look
            Thread.sleep(forTimeInterval: 2.0)
            let png = XCUIScreen.main.screenshot().pngRepresentation
            try? png.write(to: URL(fileURLWithPath: "\(dir)/title-menu-\(name)-\(theme).png"))
        }
        XCUIDevice.shared.appearance = was
    }

    /// Tells the script this step is done and waits up to 60 s for its go-ahead.
    private func sync(_ step: String) {
        guard let dir = env["WHEEL_SYNC_DIR"] else { return }
        FileManager.default.createFile(atPath: "\(dir)/step-\(step)", contents: nil)
        let ack = "\(dir)/ack-\(step)"
        let deadline = Date().addingTimeInterval(60)
        while !FileManager.default.fileExists(atPath: ack) && Date() < deadline { Thread.sleep(forTimeInterval: 0.2) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: ack), "the script acknowledged \(step)")
    }
}
