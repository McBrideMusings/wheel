import XCTest

/// Drives the Thumb navigation with real taps and swipes, against a wheeld that
/// `.claude/skills/project-verify/tools/ios-thumb.sh` set up. The script passes `WHEEL_PAIR_JSON`,
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

        // Stories 1-3: the sheet lists the session's spaces flat, in session order, over the
        // search field, the Spaces | Agents control and the session bar.
        title.tap()
        let rows = ["wheel", "alpha", "beta"].map { app.buttons["space \($0)"] }
        XCTAssertTrue(rows[0].waitForExistence(timeout: 5))
        XCTAssertTrue(rows[0].frame.minY < rows[1].frame.minY && rows[1].frame.minY < rows[2].frame.minY,
                      "spaces are listed in session order (wheel, alpha, beta), though alpha has agents waiting")
        for header in ["Needs you", "Recent", "A to Z"] {
            XCTAssertFalse(app.staticTexts[header].exists, "no \(header) section")
        }
        XCTAssertLessThan(abs(rows[2].frame.maxY - app.buttons["new space"].frame.minY), 40, "the list ends at the bottom, New space last")
        let controls = ["space search", "tab spaces", "tab agents", "session Main", "session Mobile", "session Games"]
        func rect(_ id: String) -> CGRect { (app.descendants(matching: .any).matching(identifier: id).firstMatch).frame }
        let mainRects = controls.map(rect)
        XCTAssertFalse(mainRects.contains(.zero), "every control of the sheet is on screen in Main")
        shot("spaces")

        // A tap on Mobile reloads the list in place; the controls keep their rects.
        app.buttons["session Mobile"].tap()
        XCTAssertTrue(app.buttons["space app"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["space wheel"].exists, "Main's spaces leave the list")
        XCTAssertEqual(controls.map(rect), mainRects, "the controls do not move in Mobile")
        let bar = app.descendants(matching: .any).matching(identifier: "session bar").firstMatch
        // A swipe along the bar goes on to Games, and back.
        bar.swipeLeft()
        XCTAssertEqual(app.buttons["session Games"].value as? String, "shown", "a swipe along the bar shows Games")
        XCTAssertEqual(controls.map(rect), mainRects, "the controls do not move in Games")
        bar.swipeRight()
        bar.swipeRight()
        XCTAssertEqual(app.buttons["session Main"].value as? String, "shown", "swipes along the bar return to Main")
        XCTAssertEqual(controls.map(rect), mainRects, "the controls do not move back in Main")

        // Search filters the shown session.
        let search = app.textFields["space search"]
        search.tap()
        search.typeText("alp")
        XCTAssertTrue(app.buttons["space alpha"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["space wheel"].exists, "the search hides wheel")
        XCTAssertFalse(app.buttons["space beta"].exists, "the search hides beta")
        shot("search")
        search.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 3))
        XCTAssertTrue(app.buttons["space wheel"].waitForExistence(timeout: 5))
        app.buttons["space beta"].tap()
        expectName("beta")
        sync("sheet")

        // Stories 4-5: the Agents tab lists every agent of the session, answers a request inline,
        // and a tap on a row takes the phone to that agent's space.
        title.tap()
        app.buttons["tab agents"].tap()
        let agentRows = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'agentrow '"))
        XCTAssertTrue(waitUntil { agentRows.count == 9 }, "All lists the session's nine agents, has \(agentRows.count)")
        XCTAssertTrue(app.buttons["tab agents"].label.contains("3"), "the tab control shows 3 waiting, reads '\(app.buttons["tab agents"].label)'")
        let needs = app.staticTexts["NEEDS YOU"].frame.minY
        let working = app.staticTexts["WORKING"].frame.minY
        let idle = app.staticTexts["IDLE"].frame.minY
        XCTAssertTrue(needs < working && working < idle, "Needs you first, then Working, then Idle: \(needs) \(working) \(idle)")
        let list = app.scrollViews["agents list"]
        let allows = list.buttons.matching(identifier: "Allow")
        XCTAssertEqual(allows.count, 3, "each waiting agent has Allow inline")
        XCTAssertEqual(list.buttons.matching(identifier: "Deny").count, 3, "each waiting agent has Deny inline")
        shot("agents")
        XCTAssertEqual(["tab spaces", "tab agents", "session Main", "session Mobile", "session Games"].map(rect), Array(mainRects[1...]),
                       "the tab control and the session bar keep their rects in the Agents tab")
        XCTAssertEqual(rect("agent scope").minY, mainRects[0].minY, accuracy: 4, "the filter takes the search field's slot")

        app.buttons["This space"].tap()
        XCTAssertTrue(waitUntil { agentRows.count == 2 }, "This space lists beta's two agents, has \(agentRows.count)")
        app.buttons["All"].tap()
        XCTAssertTrue(waitUntil { agentRows.count == 9 })

        allows.element(boundBy: 0).tap()
        XCTAssertTrue(waitUntil { allows.count == 2 }, "an answered request leaves the tab: \(allows.count) Allow buttons")
        XCTAssertTrue(waitUntil { app.buttons["tab agents"].label.contains("2") }, "the tab control drops to 2 waiting")
        XCTAssertTrue(waitUntil { count.label == "2" }, "the top-left count drops to 2, reads '\(count.label)'")
        sync("allowed")

        app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'agentrow ' AND label CONTAINS 'alpha'")).firstMatch.tap()
        expectName("alpha")
        XCTAssertTrue(app.buttons["tab agents"].waitForNonExistence(timeout: 5), "a tap on a row closes the sheet")
        sync("opened")

        // A swipe up from the page handle opens the sheet on the tab last used, a
        // long one on Agents, with the keyboard down and up.
        let handle = app.descendants(matching: .any).matching(identifier: "page handle").firstMatch
        XCTAssertTrue(handle.waitForExistence(timeout: 5))
        func swipeUp(_ points: CGFloat) {
            let from = handle.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            from.press(forDuration: 0.05, thenDragTo: from.withOffset(CGVector(dx: 0, dy: -points)))
        }
        func closeSheet() {
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.08)).tap()
            XCTAssertTrue(app.buttons["tab spaces"].waitForNonExistence(timeout: 5), "tapping above the sheet closes it")
        }
        swipeUp(80)
        XCTAssertTrue(app.buttons["tab spaces"].waitForExistence(timeout: 5), "a short swipe up from the handle opens the sheet")
        XCTAssertTrue(app.scrollViews["agents list"].exists, "on Agents, the tab last used")
        app.buttons["tab spaces"].tap()
        XCTAssertTrue(app.buttons["new space"].waitForExistence(timeout: 5))
        closeSheet()
        swipeUp(80)
        XCTAssertTrue(app.buttons["new space"].waitForExistence(timeout: 5), "a short swipe reopens on Spaces, the tab last used")
        closeSheet()
        swipeUp(220)
        XCTAssertTrue(app.scrollViews["agents list"].waitForExistence(timeout: 5), "a long swipe up opens Agents")
        shot("handle-agents")
        closeSheet()
        let composer = app.textViews.firstMatch
        XCTAssertTrue(composer.waitForExistence(timeout: 5))
        composer.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5), "the keyboard is up")
        swipeUp(80)
        XCTAssertTrue(app.buttons["tab spaces"].waitForExistence(timeout: 5), "a swipe up opens the sheet with the keyboard up")
        shot("handle-keyboard")
        app.buttons["tab spaces"].tap()
        app.buttons["space alpha"].tap()
        sync("handle")
    }

    private func expectName(_ space: String) {
        let shown = app.buttons.matching(NSPredicate(format: "identifier == 'title' AND label BEGINSWITH %@", "\(space), ")).firstMatch
        XCTAssertTrue(shown.waitForExistence(timeout: 10), "the title shows \(space)")
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
