import XCTest

/// Drives the panes button and its list with real taps, against a wheeld that
/// `.claude/skills/project-verify/tools/ios-panes-list.sh` set up: space wheel with a grid agent
/// that has replied, a grid agent working, a popped-out agent and a backgrounded agent, and space
/// alpha with one agent. The script passes `WHEEL_PAIR_JSON`, `WHEEL_SYNC_DIR`, `WHEEL_SHOTS` and
/// `WHEEL_AGENTS` (`replied,working,popped,backgrounded,alpha`: each pane's agent name) as
/// TEST_RUNNER_ variables; at each step the test writes `<sync>/step-<name>` and waits for the
/// script's `<sync>/ack-<name>`, so the script changes and checks wheeld between steps.
@MainActor
final class PanesListUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
    }

    func testPanesList() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        let names = try XCTUnwrap(env["WHEEL_AGENTS"], "WHEEL_AGENTS").split(separator: ",").map(String.init)
        XCTAssertEqual(names.count, 5)
        let (replied, working, popped, backgrounded, alpha) = (names[0], names[1], names[2], names[3], names[4])
        app = .paired(pair)
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["mode"].waitForExistence(timeout: 40), "the title shows once paired")

        // Case 1: no orange dot while nobody waits. The button counts the pages and says it is closed.
        let panes = app.buttons["panes"]
        XCTAssertTrue(panes.waitForExistence(timeout: 10), "the panes button shows")
        XCTAssertEqual(panes.label, "Panes")
        XCTAssertTrue(waitUntil { (panes.value as? String) == "3 panes" }, "three pages and no badge, have '\(panes.value as? String ?? "nil")'")
        XCTAssertFalse(panes.isSelected, "the button is not selected while the list is closed")
        XCTAssertFalse(any("panes list").exists, "the list is closed")
        shot("quiet")
        sync("quiet")

        // The script made the backgrounded agent ask: the orange dot shows, in the button's value.
        XCTAssertTrue(waitUntil { (panes.value as? String) == "3 panes, needs you" }, "the badge shows, have '\(panes.value as? String ?? "nil")'")
        shot("badged")

        // Case 2: the button swaps the page for the list: the space's panes, then its agents, needs you, then working,
        // then idle, with the backgrounded agent hidden behind a button.
        panes.tap()
        XCTAssertTrue(any("panes list").waitForExistence(timeout: 5), "the panes button opens the list")
        XCTAssertTrue(panes.isSelected, "the button is selected while the list is open")
        XCTAssertEqual(panes.value as? String, "3 panes, needs you, open")
        let rows = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'pane row '"))
        XCTAssertTrue(waitUntil { rows.count == 3 }, "a row for each of the three panes, have \(rows.count)")
        XCTAssertEqual(rows.element(boundBy: 0).value as? String, "shown", "the first pane is the one shown")
        XCTAssertEqual(rows.element(boundBy: 1).value as? String ?? "", "", "another pane is not shown")
        let tiles = app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'agent tile '"))
        XCTAssertTrue(waitUntil { tiles.count == 3 }, "three tiles with the backgrounded one hidden, have \(tiles.count)")
        XCTAssertFalse(tile(backgrounded).exists, "the backgrounded agent is hidden")
        XCTAssertFalse(any("no agents").exists)
        let order = tileOrder(tiles)
        XCTAssertEqual(order.first, working, "working first: \(order)")
        XCTAssertEqual(Set(order.suffix(2)), Set([replied, popped]), "the idle two last: \(order)")
        // Case 3: the working tile shows the system spinner, the others none; each tile names its state.
        XCTAssertTrue(tile(working).descendants(matching: .any)["working spinner"].exists, "the working tile spins")
        for name in [replied, popped] {
            XCTAssertFalse(tile(name).descendants(matching: .any)["working spinner"].exists, "\(name) does not spin")
        }
        XCTAssertEqual(tile(working).staticTexts["tile state"].label, "Working")
        XCTAssertEqual(tile(replied).staticTexts["tile state"].label, "Idle")
        // Case 4: the replied agent's last message, cut to two lines (its reply is longer).
        let message = tile(replied).staticTexts["last message"]
        XCTAssertTrue(message.exists, "the replied agent's last message shows")
        lines("replied", message)
        shot("list-hidden")

        // The toggle shows the backgrounded agent: needs you first, tagged backgrounded, with the command, Deny, Allow
        // and an open control.
        let toggle = app.buttons["backgrounded toggle"]
        XCTAssertEqual(toggle.label, "Show 1 backgrounded")
        toggle.tap()
        XCTAssertTrue(waitUntil { tiles.count == 4 }, "four tiles with the backgrounded one shown, have \(tiles.count)")
        XCTAssertEqual(toggle.label, "Hide 1 backgrounded")
        XCTAssertEqual(tileOrder(tiles).first, backgrounded, "needs you first: \(tileOrder(tiles))")
        let waiting = tile(backgrounded)
        XCTAssertTrue(waiting.staticTexts["tile backgrounded"].exists, "the tile is tagged backgrounded")
        XCTAssertEqual(waiting.staticTexts["tile state"].label, "Needs you")
        XCTAssertTrue(waiting.descendants(matching: .any)["tile command"].exists, "the request's command shows")
        XCTAssertEqual(waiting.buttons["tile deny"].label, "Deny")
        XCTAssertEqual(waiting.buttons["tile allow"].label, "Allow")
        XCTAssertTrue(waiting.buttons["tile open"].exists, "the open control shows")
        shot("list-mixed")
        sync("list")

        // Case 5: Allow answers the request from the list; the tile leaves needs you and the dot goes.
        waiting.buttons["tile allow"].tap()
        XCTAssertTrue(waitUntil { !self.tile(backgrounded).buttons["tile allow"].exists }, "the tile leaves needs you")
        XCTAssertTrue(waitUntil { (panes.value as? String) == "3 panes, open" }, "the dot goes, have '\(panes.value as? String ?? "nil")'")
        sync("allowed")

        // The script made the replied agent ask; its tile moves first, its row says needs you, and Deny denies it.
        XCTAssertTrue(waitUntil { self.tile(replied).buttons["tile deny"].exists }, "the replied agent waits")
        XCTAssertEqual(tileOrder(tiles).first, replied, "the waiting tile moved first: \(tileOrder(tiles))")
        XCTAssertEqual(rows.element(boundBy: 0).value as? String, "shown, needs you", "the pane's row says it needs you")
        if tile(replied).staticTexts["last message"].exists { lines("needs-you", tile(replied).staticTexts["last message"]) }
        shot("tile-needs-you")
        tile(replied).buttons["tile deny"].tap()
        XCTAssertTrue(waitUntil { !self.tile(replied).buttons["tile deny"].exists }, "Deny answered it")
        sync("denied")

        // Case 6: a backgrounded agent's tile opens the last page on that pane and closes the list.
        tile(backgrounded).buttons["agentrow \(backgrounded)"].tap()
        XCTAssertTrue(waitUntil { !self.any("panes list").exists }, "the list closes")
        XCTAssertTrue(app.buttons["tray back"].waitForExistence(timeout: 5), "the last page shows the pane in place")
        expectPages(endingWith: "[tray]")
        shot("open-backgrounded")
        sync("open-backgrounded")

        // A row opens its pane and closes the list: the popped-out one is on its page.
        panes.tap()
        XCTAssertTrue(any("panes list").waitForExistence(timeout: 5))
        app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'pane row '")).element(boundBy: 2).tap()
        XCTAssertTrue(waitUntil { !self.any("panes list").exists }, "a row tap closes the list")
        expectPages(endingWith: "[popped] tray")
        sync("open-popped")

        // A tile opens the pane too: the grid agent that replied.
        panes.tap()
        XCTAssertTrue(any("panes list").waitForExistence(timeout: 5))
        tile(replied).buttons["agentrow \(replied)"].tap()
        XCTAssertTrue(waitUntil { !self.any("panes list").exists }, "a tile tap closes the list")
        expectPages(endingWith: "popped tray")
        XCTAssertTrue(waitUntil { (self.any("pages").value as? String ?? "").hasPrefix("[grid]") }, "the replied agent's page shows")
        sync("open-tile")

        // A second tap on the button closes the list.
        panes.tap()
        XCTAssertTrue(any("panes list").waitForExistence(timeout: 5))
        panes.tap()
        XCTAssertTrue(waitUntil { !self.any("panes list").exists }, "a second tap on the button closes the list")
        XCTAssertFalse(panes.isSelected)
        sync("closed")

        // An agent in another space is one tap from the space switcher's Agents tab: the phone switches to that
        // space and its pane.
        app.buttons["space switcher"].tap()
        app.buttons["tab agents"].tap()
        XCTAssertTrue(tile(alpha).waitForExistence(timeout: 5), "the Agents tab lists the other space's agent")
        tile(alpha).buttons["agentrow \(alpha)"].tap()
        XCTAssertTrue(waitUntil { self.app.buttons["space switcher"].label.hasPrefix("alpha") }, "the title shows alpha, have '\(app.buttons["space switcher"].label)'")
        sync("open-alpha")

        // The script stopped the working turn: alpha's list has its one agent idle, no spinner.
        panes.tap()
        XCTAssertTrue(any("panes list").waitForExistence(timeout: 5))
        XCTAssertTrue(waitUntil { tiles.count == 1 }, "alpha's list has one tile, have \(tiles.count)")
        XCTAssertTrue(waitUntil { !self.app.descendants(matching: .any)["working spinner"].exists }, "no tile spins")
        shot("list-all-idle")
        sync("all-idle")

        // The script turned every agent pane into a terminal: the list says so.
        XCTAssertTrue(any("no agents").waitForExistence(timeout: 15), "the empty list says there are no agents")
        XCTAssertEqual(tiles.count, 0)
        XCTAssertEqual(rows.count, 1, "the pane still has its row")
        shot("list-empty")
        sync("empty")
    }

    private func any(_ id: String) -> XCUIElement { app.descendants(matching: .any)[id].firstMatch }

    private func tile(_ agent: String) -> XCUIElement {
        app.descendants(matching: .any)["agent tile \(agent)"]
    }

    private func tileOrder(_ tiles: XCUIElementQuery) -> [String] {
        tiles.allElementsBoundByIndex.map { String($0.identifier.dropFirst("agent tile ".count)) }
    }

    /// The text's height in lines of its own font, written to the log for the script: a
    /// subheadline line is about 20 pt on an iPhone at the default size.
    private func lines(_ name: String, _ text: XCUIElement) {
        let n = Int((text.frame.height / 20).rounded())
        print("tile lines \(name)=\(n) height=\(text.frame.height)")
        sync("lines-\(name)-\(n)", wait: false)
    }

    private func expectPages(endingWith suffix: String) {
        let pages = any("pages")
        XCTAssertTrue(pages.waitForExistence(timeout: 10), "the pages element is behind the top bar")
        XCTAssertTrue(waitUntil { ((pages.value as? String) ?? "").hasSuffix(suffix) }, "pages end '\(suffix)', have '\(pages.value as? String ?? "nil")'")
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

    /// Saves the screen as `<shots>/panes-<name>-<light|dark>.png`.
    private func shot(_ name: String) {
        guard let dir = env["WHEEL_SHOTS"] else { return }
        let was = XCUIDevice.shared.appearance
        for (theme, look) in [("light", XCUIDevice.Appearance.light), ("dark", .dark)] {
            XCUIDevice.shared.appearance = look
            Thread.sleep(forTimeInterval: 2.0)
            let png = XCUIScreen.main.screenshot().pngRepresentation
            try? png.write(to: URL(fileURLWithPath: "\(dir)/panes-\(name)-\(theme).png"))
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
