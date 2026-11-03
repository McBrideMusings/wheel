import XCTest

/// Drives the phone's command palette with real taps against a wheeld that
/// `.claude/skills/project/tools/ios-palette.sh` set up: session "wheel" holding spaces wheel (two
/// agent panes, one working) and alpha, an artifact "dash" in wheel, and session "games" holding chess.
/// The test writes `<sync>/step-<name>` after each step and waits for `<sync>/ack-<name>`.
final class CommandPaletteUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() { continueAfterFailure = false }

    func testPalette() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["title"].waitForExistence(timeout: 40))
        expectTitle("wheel", session: "wheel")

        // 1. The search button opens the palette, listing every kind of row.
        openPalette()
        for id in ["palette space:", "palette pane:", "palette artifact:project/dash", "palette session:",
                   "palette action:rename-space", "palette action:close-space", "palette action:new-agent-pane"] {
            XCTAssertTrue(row(prefix: id).waitForExistence(timeout: 10), "a row \(id)")
        }
        shot("open")

        // 2. A typed name finds a space in another session and choosing it shows it.
        type("chess")
        XCTAssertTrue(row(prefix: "palette space:").waitForExistence(timeout: 5))
        row(prefix: "palette space:").tap()
        expectTitle("chess", session: "games")
        sync("chess")

        // 3. A session by name.
        openPalette()
        type("wheel")
        let session = row(prefix: "palette session:")
        XCTAssertTrue(session.waitForExistence(timeout: 5))
        session.tap()
        expectTitle("wheel", session: "wheel")

        // 4. A pane by its agent's state.
        openPalette()
        type("working")
        let pane = row(prefix: "palette pane:")
        XCTAssertTrue(pane.waitForExistence(timeout: 5))
        shot("pane-state")
        pane.tap()
        sync("pane")

        // 5. An artifact opens in a new pane.
        openPalette()
        type("dash")
        XCTAssertTrue(app.buttons["palette artifact:project/dash"].waitForExistence(timeout: 10))
        app.buttons["palette artifact:project/dash"].tap()
        sync("artifact")

        // 6. New agent pane.
        openPalette()
        type("new")
        XCTAssertTrue(app.buttons["palette action:new-agent-pane"].waitForExistence(timeout: 5))
        app.buttons["palette action:new-agent-pane"].tap()
        sync("new-agent")

        // 7. Rename space… opens the rename alert; Close space asks while an agent works.
        openPalette()
        type("rename")
        app.buttons["palette action:rename-space"].tap()
        let field = app.alerts.textFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 5), "Rename space… opens the rename alert")
        field.tap()
        field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 12) + "wheel2")
        app.alerts.buttons["Rename"].tap()
        sync("renamed")

        openPalette()
        type("close")
        app.buttons["palette action:close-space"].tap()
        XCTAssertTrue(app.alerts.buttons["Cancel"].waitForExistence(timeout: 5), "Close space asks while an agent works")
        shot("close-confirm")
        app.alerts.buttons["Cancel"].tap()
        sync("kept")
    }

    private func openPalette() {
        app.buttons["search"].tap()
        XCTAssertTrue(app.textFields["palette field"].waitForExistence(timeout: 5))
    }

    private func type(_ text: String) {
        let field = app.textFields["palette field"]
        field.tap()
        field.typeText(text)
    }

    private func row(prefix: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@", prefix)).firstMatch
    }

    private func expectTitle(_ space: String, session: String, timeout: TimeInterval = 10) {
        let label = "\(space), session \(session)"
        let shown = app.buttons.matching(NSPredicate(format: "identifier == 'title' AND label == %@", label)).firstMatch
        XCTAssertTrue(shown.waitForExistence(timeout: timeout), "title reads '\(label)', has '\(app.buttons["title"].label)'")
    }

    private func shot(_ name: String) {
        guard let dir = env["WHEEL_SHOTS"] else { return }
        let was = XCUIDevice.shared.appearance
        for (theme, look) in [("light", XCUIDevice.Appearance.light), ("dark", .dark)] {
            XCUIDevice.shared.appearance = look
            Thread.sleep(forTimeInterval: 2.0)
            try? XCUIScreen.main.screenshot().pngRepresentation.write(to: URL(fileURLWithPath: "\(dir)/palette-\(name)-\(theme).png"))
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
