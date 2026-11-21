import XCTest

/// Drives the phone's mode menu with real taps against a wheeld that
/// `.claude/skills/project-verify/tools/ios-mode.sh` set up: one space holding two agent panes.
/// The test writes `<sync>/step-<name>` after each step and waits for `<sync>/ack-<name>`, and writes the
/// top bar's control rects to `<shots>/mode-rects-<state>.txt`. With `WHEEL_MODE_BASE` set it writes the launch
/// rects and stops, so the same test measures the bar of a build from before the change.
final class ModeUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!
    /// The bar's controls whose rects must hold still: the title, the eye, search and the ⋯.
    private let controls = ["title", "agents", "search", "Pane menu"]

    override func setUp() { continueAfterFailure = false }

    func testModeMenu() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["title"].waitForExistence(timeout: 40))
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 20))
        let launch = rects("launch")
        shot("spaces")
        if env["WHEEL_MODE_BASE"] != nil { return }
        let mode = app.buttons["mode"]
        XCTAssertTrue(mode.waitForExistence(timeout: 5), "the top bar has the mode button")

        // 1. The button is 28 pt square, the leftmost of the right-hand group, and shows Spaces.
        XCTAssertEqual(mode.frame.width, 28, accuracy: 0.5, "the mode button is 28 pt wide: \(mode.frame)")
        XCTAssertEqual(mode.value as? String, "Spaces")
        XCTAssertLessThan(mode.frame.maxX, app.buttons["agents"].frame.minX, "the mode button sits left of the eye")
        XCTAssertFalse(app.otherElements["mode body"].exists)

        // 2. The menu lists the Mac's three modes; each pick shows its body and moves no control.
        for (title, id) in [("Agents", "agents"), ("Orchestrator", "orchestrator"), ("Spaces", "spaces")] {
            mode.tap()
            let row = menuRow(title)
            XCTAssertTrue(row.waitForExistence(timeout: 5), "the menu has \(title)")
            XCTAssertTrue(menuRow("Spaces").exists && menuRow("Agents").exists && menuRow("Orchestrator").exists, "the menu lists all three modes")
            if id == "agents" { shot("menu") }
            row.tap()
            XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == %@", title)).firstMatch.waitForExistence(timeout: 5), "the button shows \(title)")
            let body = app.descendants(matching: .any)["mode body"]
            if id == "spaces" {
                XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 5))
                XCTAssertFalse(body.exists, "Spaces shows the pages")
            } else {
                // With no Orchestrator space in this fixture, Orchestrator mode says so and offers Start.
                let shown = id == "orchestrator" ? app.descendants(matching: .any)["no orchestrator"] : body
                XCTAssertTrue(shown.waitForExistence(timeout: 5), "\(title) shows its body")
                shot(id)
            }
            // Orchestrator mode's title names another space, so only its width may follow that name.
            XCTAssertEqual(placed(rects(id)), placed(launch), "no control moved in \(title)")
            sync(id)
        }

        // 3. Opening a pane from the eye's sheet in Agents shows Spaces again, with the pane.
        mode.tap()
        menuRow("Agents").tap()
        XCTAssertTrue(app.descendants(matching: .any)["mode body"].waitForExistence(timeout: 5))
        app.buttons["agents"].tap()
        let tile = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'agentrow'")).firstMatch
        XCTAssertTrue(tile.waitForExistence(timeout: 10), "the eye's sheet lists an agent")
        tile.tap()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Spaces'")).firstMatch.waitForExistence(timeout: 5), "opening a pane shows Spaces")
        XCTAssertFalse(app.descendants(matching: .any)["mode body"].exists)
        sync("opened")

        // 4. The mode survives a relaunch.
        mode.tap()
        menuRow("Agents").tap()
        app.terminate()
        // The pairing code is spent; the relaunch connects with the credential the first launch saved.
        app.launchEnvironment.removeValue(forKey: "WHEEL_PAIR_JSON")
        app.launch()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Agents'")).firstMatch.waitForExistence(timeout: 40), "Agents restored after a relaunch")
        sync("relaunch")
        app.buttons["mode"].tap()
        menuRow("Spaces").tap()
    }

    /// A row of the open mode menu; the eye is also labelled Agents, so it is left out by its id.
    private func menuRow(_ title: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label == %@ AND identifier != 'agents' AND identifier != 'mode'", title)).firstMatch
    }

    /// The rect lines with the title's width left out: the title is the shown space's name, and holds its place.
    private func placed(_ lines: String) -> String {
        lines.split(separator: "\n").map { line -> String in
            var f = line.split(separator: " ").map(String.init)
            if f.first == "title", f.count == 5 { f[3] = "-" }
            return f.joined(separator: " ")
        }.joined(separator: "\n")
    }

    /// Each control's frame, one `<id> x y w h` line per control, written to the shots folder.
    private func rects(_ state: String) -> String {
        let lines = controls.map { id -> String in
            let f = app.buttons[id].firstMatch.frame
            return "\(id) \(Int(f.minX)) \(Int(f.minY)) \(Int(f.width)) \(Int(f.height))"
        }.joined(separator: "\n")
        if let dir = env["WHEEL_SHOTS"] {
            try? lines.write(toFile: "\(dir)/mode-rects-\(state).txt", atomically: true, encoding: .utf8)
        }
        return lines
    }

    private func shot(_ name: String) {
        guard let dir = env["WHEEL_SHOTS"] else { return }
        let was = XCUIDevice.shared.appearance
        for (theme, look) in [("light", XCUIDevice.Appearance.light), ("dark", .dark)] {
            XCUIDevice.shared.appearance = look
            Thread.sleep(forTimeInterval: 2.0)
            try? XCUIScreen.main.screenshot().pngRepresentation.write(to: URL(fileURLWithPath: "\(dir)/mode-\(name)-\(theme).png"))
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
