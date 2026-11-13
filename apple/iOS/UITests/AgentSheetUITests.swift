import XCTest

/// Drives the eye button and its agent sheet with real taps, against a wheeld that
/// `.claude/skills/project-verify/tools/ios-agents-sheet.sh` set up: space wheel with a grid agent
/// that has replied, a grid agent working, a popped-out agent and a backgrounded agent, and space
/// alpha with one agent. The script passes `WHEEL_PAIR_JSON`, `WHEEL_SYNC_DIR`, `WHEEL_SHOTS` and
/// `WHEEL_AGENTS` (`replied,working,popped,backgrounded,alpha`: each pane's agent name) as
/// TEST_RUNNER_ variables; at each step the test writes `<sync>/step-<name>` and waits for the
/// script's `<sync>/ack-<name>`, so the script changes and checks wheeld between steps.
@MainActor
final class AgentSheetUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
    }

    func testAgentSheet() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        let names = try XCTUnwrap(env["WHEEL_AGENTS"], "WHEEL_AGENTS").split(separator: ",").map(String.init)
        XCTAssertEqual(names.count, 5)
        let (replied, working, popped, backgrounded, alpha) = (names[0], names[1], names[2], names[3], names[4])
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["title"].waitForExistence(timeout: 40), "the title shows once paired")

        // Case 1: no badge while nobody waits. The eye has no text or number.
        let eye = app.buttons["agents"]
        XCTAssertTrue(eye.waitForExistence(timeout: 10), "the eye button shows")
        XCTAssertTrue(waitUntil { (eye.value as? String ?? "") == "" }, "no badge, have '\(eye.value as? String ?? "nil")'")
        XCTAssertEqual(eye.label, "Agents", "the eye carries no number in its label")
        shot("eye-quiet")
        sync("quiet")

        // The script made the backgrounded agent ask: the badge shows.
        XCTAssertTrue(waitUntil { (eye.value as? String) == "needs you" }, "the badge shows, have '\(eye.value as? String ?? "nil")'")
        shot("eye-badged")

        // Case 2: the sheet lists every agent of the session, flat, needs you, then working, then idle.
        eye.tap()
        XCTAssertTrue(app.scrollViews["agents list"].waitForExistence(timeout: 5), "the eye opens the sheet")
        let tiles = app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'agent tile '"))
        XCTAssertTrue(waitUntil { tiles.count == 5 }, "five tiles, have \(tiles.count)")
        let order = tiles.allElementsBoundByIndex.map { String($0.identifier.dropFirst("agent tile ".count)) }
        XCTAssertEqual(Array(order.prefix(2)), [backgrounded, working], "needs you first, then working: \(order)")
        XCTAssertEqual(Set(order.suffix(3)), Set([replied, popped, alpha]), "the idle three last: \(order)")
        // Case 3: the working tile shows the system spinner, the others none.
        XCTAssertTrue(tile(working).descendants(matching: .any)["working spinner"].exists, "the working tile spins")
        for name in [replied, popped, alpha, backgrounded] {
            XCTAssertFalse(tile(name).descendants(matching: .any)["working spinner"].exists, "\(name) does not spin")
        }
        // Case 7: no status dot (a StatusDot is an element labelled with its status).
        let statusWords = NSPredicate(format: "label BEGINSWITH 'Needs you' OR label IN {'Stopped', 'Working', 'Done', 'Idle'}")
        XCTAssertEqual(app.scrollViews["agents list"].descendants(matching: .any).matching(statusWords).count, 0, "no status dot in the sheet")
        // The needs-you tile carries the command and Deny, Allow and an open control.
        let waiting = tile(backgrounded)
        XCTAssertTrue(waiting.descendants(matching: .any)["tile command"].exists, "the request's command shows")
        XCTAssertEqual(waiting.buttons["tile deny"].label, "Deny")
        XCTAssertEqual(waiting.buttons["tile allow"].label, "Allow")
        XCTAssertTrue(waiting.buttons["tile open"].exists, "the open control shows")
        // Case 4: the replied agent's last message, cut to two lines (its reply is longer).
        let message = tile(replied).staticTexts["last message"]
        XCTAssertTrue(message.exists, "the replied agent's last message shows")
        lines("replied", message)
        shot("sheet-mixed")
        sync("sheet")

        // Case 5: Allow answers the request from the sheet; the tile leaves needs you.
        waiting.buttons["tile allow"].tap()
        XCTAssertTrue(waitUntil { !self.tile(backgrounded).buttons["tile allow"].exists }, "the tile leaves needs you")
        XCTAssertTrue(waitUntil { (eye.value as? String ?? "") == "" }, "the badge goes")
        sync("allowed")

        // The script made the replied agent ask; Deny denies it.
        XCTAssertTrue(waitUntil { self.tile(replied).buttons["tile deny"].exists }, "the replied agent waits")
        let first = tiles.allElementsBoundByIndex.first.map { String($0.identifier.dropFirst("agent tile ".count)) }
        XCTAssertEqual(first, replied, "the waiting tile moved first")
        if tile(replied).staticTexts["last message"].exists { lines("needs-you", tile(replied).staticTexts["last message"]) }
        shot("tile-needs-you")
        tile(replied).buttons["tile deny"].tap()
        XCTAssertTrue(waitUntil { !self.tile(replied).buttons["tile deny"].exists }, "Deny answered it")
        sync("denied")

        // Case 6: a backgrounded agent's tile opens the last page on that pane.
        tile(backgrounded).buttons["agentrow \(backgrounded)"].tap()
        XCTAssertTrue(waitUntil { !self.app.scrollViews["agents list"].exists }, "the sheet closes")
        XCTAssertTrue(app.buttons["tray back"].waitForExistence(timeout: 5), "the last page shows the pane in place")
        expectDots(endingWith: "[tray]")
        shot("open-backgrounded")
        sync("open-backgrounded")

        // A popped-out agent opens on its page.
        eye.tap()
        XCTAssertTrue(app.scrollViews["agents list"].waitForExistence(timeout: 5))
        tile(popped).buttons["agentrow \(popped)"].tap()
        XCTAssertTrue(waitUntil { !self.app.scrollViews["agents list"].exists }, "the sheet closes")
        expectDots(endingWith: "[popped] tray")
        sync("open-popped")

        // An agent in another space: the phone switches to that space and its pane.
        eye.tap()
        XCTAssertTrue(app.scrollViews["agents list"].waitForExistence(timeout: 5))
        tile(alpha).buttons["agentrow \(alpha)"].tap()
        XCTAssertTrue(waitUntil { self.app.buttons["title"].label.hasPrefix("alpha") }, "the title shows alpha, have '\(app.buttons["title"].label)'")
        sync("open-alpha")

        // The script stopped the working turn: every tile idle, no spinner.
        eye.tap()
        XCTAssertTrue(app.scrollViews["agents list"].waitForExistence(timeout: 5))
        XCTAssertTrue(waitUntil { !self.app.descendants(matching: .any)["working spinner"].exists }, "no tile spins")
        shot("sheet-all-idle")
        sync("all-idle")

        // The script turned every agent pane into a terminal: the sheet says so.
        XCTAssertTrue(app.descendants(matching: .any)["no agents"].waitForExistence(timeout: 15), "the empty sheet says there are no agents")
        XCTAssertEqual(tiles.count, 0)
        shot("sheet-empty")
        sync("empty")
    }

    private func tile(_ agent: String) -> XCUIElement {
        app.descendants(matching: .any)["agent tile \(agent)"]
    }

    /// The text's height in lines of its own font, written to the log for the script: a
    /// subheadline line is about 20 pt on an iPhone at the default size.
    private func lines(_ name: String, _ text: XCUIElement) {
        let n = Int((text.frame.height / 20).rounded())
        print("tile lines \(name)=\(n) height=\(text.frame.height)")
        sync("lines-\(name)-\(n)", wait: false)
    }

    private func expectDots(endingWith suffix: String) {
        let dots = app.descendants(matching: .any)["page dots"]
        XCTAssertTrue(dots.waitForExistence(timeout: 10), "the page dots show")
        XCTAssertTrue(waitUntil { ((dots.value as? String) ?? "").hasSuffix(suffix) }, "dots end '\(suffix)', have '\(dots.value as? String ?? "nil")'")
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

    /// Saves the screen as `<shots>/agents-<name>-<light|dark>.png`.
    private func shot(_ name: String) {
        guard let dir = env["WHEEL_SHOTS"] else { return }
        let was = XCUIDevice.shared.appearance
        for (theme, look) in [("light", XCUIDevice.Appearance.light), ("dark", .dark)] {
            XCUIDevice.shared.appearance = look
            Thread.sleep(forTimeInterval: 2.0)
            let png = XCUIScreen.main.screenshot().pngRepresentation
            try? png.write(to: URL(fileURLWithPath: "\(dir)/agents-\(name)-\(theme).png"))
        }
        XCUIDevice.shared.appearance = was
    }

    private func sync(_ step: String, wait: Bool = true) {
        guard let dir = env["WHEEL_SYNC_DIR"] else { return }
        FileManager.default.createFile(atPath: "\(dir)/step-\(step)", contents: nil)
        guard wait else { return }
        let ack = "\(dir)/ack-\(step)"
        let deadline = Date().addingTimeInterval(90)
        while !FileManager.default.fileExists(atPath: ack) && Date() < deadline { Thread.sleep(forTimeInterval: 0.2) }
        XCTAssertTrue(FileManager.default.fileExists(atPath: ack), "the script acknowledged \(step)")
    }

    private func allowNotifications() {
        let allow = XCUIApplication(bundleIdentifier: "com.apple.springboard").alerts.buttons["Allow"]
        if allow.waitForExistence(timeout: 10) { allow.tap() }
    }
}
