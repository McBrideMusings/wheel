import XCTest

/// Drives Orchestrator mode's topic line and notes sheet with real taps against a wheeld that
/// `.claude/skills/project-verify/tools/ios-topic.sh` set up: a space on the orchestrator folder holding an agent pane,
/// at first with no `threads/current`. After the `none` step the script writes `threads/current` (wheel-abc) and
/// `threads/wheel-abc.md`, then runs a turn. The test writes `<sync>/step-<name>` after each step and waits for
/// `<sync>/ack-<name>`; shots go to `<shots>/topic-<state>-<theme>.png`.
final class TopicUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() { continueAfterFailure = false }

    func testTopicNotes() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["mode"].waitForExistence(timeout: 40))
        pick("Orchestrator")

        // No threads/current: the title, the mode menu, stands over the model alone.
        let title = app.buttons["mode"]
        XCTAssertTrue(waitFor(20) { title.value as? String == "Orchestrator" }, "the title reads Orchestrator: \(String(describing: title.value))")
        Thread.sleep(forTimeInterval: 2)
        let line = app.buttons["topic"]
        XCTAssertFalse(line.exists, "no current topic shows no context line")
        let modeBefore = app.buttons["mode"].frame, menuBefore = app.buttons["Pane menu"].frame
        shot("none")
        sync("none")

        // The script wrote the topic and ran a turn: its end brings the title under the header.
        XCTAssertTrue(line.waitForExistence(timeout: 30), "the turn's end shows the topic line")
        XCTAssertEqual(line.label, "wheel · plugin hot reload")
        XCTAssertGreaterThan(line.frame.minY, title.frame.maxY - 1, "the topic line sits under the title")
        XCTAssertEqual(line.frame.minX, title.frame.minX, accuracy: 1, "the topic line lines up under the title")
        XCTAssertLessThan(line.frame.maxX, app.buttons["Pane menu"].frame.minX, "the topic line sits left of the header's buttons")
        XCTAssertEqual(app.buttons["mode"].frame, modeBefore, "the topic line moves no header button")
        XCTAssertEqual(app.buttons["Pane menu"].frame, menuBefore, "the topic line moves no header button")
        shot("line")
        sync("line")

        // A tap opens the notes, rendered: headings and list items as text, no front matter.
        line.tap()
        let sheet = app.descendants(matching: .any)["topic notes"].firstMatch
        XCTAssertTrue(sheet.waitForExistence(timeout: 10), "the line opens the notes sheet")
        XCTAssertEqual(app.staticTexts["topic notes title"].label, "wheel · plugin hot reload")
        XCTAssertTrue(text("Decisions").waitForExistence(timeout: 10), "the heading is rendered")
        XCTAssertTrue(text("Shared pane state lives in the backend feed").exists, "the list item is rendered")
        XCTAssertTrue(text("Snapshot on quit or on every update?").exists, "the open question is rendered")
        XCTAssertFalse(text("session: 5f0c2d4e").exists, "the front matter is not shown")
        XCTAssertFalse(text("## Decisions").exists, "the Markdown is rendered, not shown raw")
        shot("notes")
        sync("notes")
    }

    private func text(_ s: String) -> XCUIElement {
        app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", s)).firstMatch
    }

    private func pick(_ title: String) {
        app.buttons["mode"].tap()
        let row = app.buttons.matching(NSPredicate(format: "label == %@ AND identifier != 'agents' AND identifier != 'mode'", title)).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 5), "the mode menu has \(title)")
        row.tap()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == %@", title)).firstMatch.waitForExistence(timeout: 5), "the button shows \(title)")
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
            try? XCUIScreen.main.screenshot().pngRepresentation.write(to: URL(fileURLWithPath: "\(dir)/topic-\(name)-\(theme).png"))
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
