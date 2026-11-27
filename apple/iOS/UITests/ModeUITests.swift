import XCTest

/// Drives the phone's mode toggle with real taps against a wheeld that
/// `.claude/skills/project-verify/tools/ios-mode.sh` set up: one space holding two agent panes.
/// The test writes `<sync>/step-<name>` after each step and waits for `<sync>/ack-<name>`, and writes the
/// top bar's control rects to `<shots>/mode-rects-<state>.txt`. With `WHEEL_MODE_BASE` set it writes the launch
/// rects and stops, so the same test measures the bar of a build from before the change.
final class ModeUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!
    /// The bar's controls whose rects must hold still: the title (the mode toggle), the panes button and the ⋯.
    private let controls = ["mode", "panes", "Pane menu"]

    override func setUp() { continueAfterFailure = false }

    func testModeToggle() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        app = .paired(pair)
        app.launch()
        allowNotifications()
        // The ⋯, not the mode toggle, so a base build from before the toggle passes this too.
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 40))
        let launch = rects("launch")
        shot("spaces")
        if env["WHEEL_MODE_BASE"] != nil { return }
        let mode = app.buttons["mode"]
        XCTAssertTrue(mode.waitForExistence(timeout: 5), "the top bar has the mode toggle")

        // 1. The title is the mode toggle at the bar's left, showing Spaces, with the space switcher under it; the
        // panes button sits left of the ⋯, and the eye, search, page handle and page dots are gone.
        XCTAssertEqual(mode.value as? String, "Spaces")
        XCTAssertLessThan(mode.frame.minX, 40, "the mode toggle is the bar's title, at its left: \(mode.frame)")
        let switcher = app.buttons["space switcher"]
        XCTAssertTrue(switcher.exists, "the space switcher is under the title")
        XCTAssertGreaterThan(switcher.frame.minY, mode.frame.maxY - 1, "the switcher sits under the title")
        XCTAssertTrue(switcher.label.hasPrefix("wheel, session "), "the switcher names the space: \(switcher.label)")
        let panes = app.buttons["panes"]
        XCTAssertTrue(panes.exists, "the panes button shows")
        XCTAssertEqual(panes.value as? String, "2 panes", "the panes button counts the pages")
        XCTAssertLessThan(panes.frame.maxX, app.buttons["Pane menu"].frame.minX + 1, "the panes button sits left of the ⋯")
        for gone in ["agents", "search"] { XCTAssertFalse(app.buttons[gone].exists, "no \(gone) button") }
        for gone in ["page handle", "page dots", "mode body"] { XCTAssertFalse(any(gone).exists, "no \(gone)") }

        // 2. A tap toggles Spaces and Orchestrator with no menu, and no control moves.
        for (title, id) in [("Orchestrator", "orchestrator"), ("Spaces", "spaces")] {
            mode.tap()
            XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == %@", title)).firstMatch.waitForExistence(timeout: 5), "the button shows \(title)")
            XCTAssertFalse(app.buttons.matching(NSPredicate(format: "label IN %@ AND identifier != 'mode'", ["Spaces", "Agents", "Orchestrator"])).firstMatch.exists, "the toggle opens no menu")
            if id == "orchestrator" {
                // With no space open on the orchestrator's folder, Orchestrator mode says where to open one.
                XCTAssertTrue(app.otherElements["no orchestrator"].waitForExistence(timeout: 5), "\(title) shows its body")
                XCTAssertFalse(switcher.exists, "Orchestrator mode has no space switcher")
                XCTAssertTrue(panes.exists, "Orchestrator mode keeps the panes button")
                XCTAssertFalse(app.buttons["agents"].exists || app.buttons["search"].exists, "Orchestrator mode has no eye and no search")
            } else {
                XCTAssertTrue(switcher.waitForExistence(timeout: 5), "Spaces shows the space switcher")
            }
            shot(id)
            XCTAssertEqual(placed(rects(id)), placed(launch), "no control moved in \(title)")
            sync(id)
        }

        // 2b. With the keyboard up, a drag on the transcript hides it, and so does a tap.
        // The pager keeps the neighbour page in the tree, so the composer and the pane are the ones on screen. The
        // transcript has no element of its own: the drag runs down the pane's middle, and the tap lands on its message.
        let shown = { (q: XCUIElementQuery) in q.allElementsBoundByIndex.first { $0.isHittable } ?? q.firstMatch }
        let pane = { shown(self.app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'agent pane '"))) }
        let drag = {
            let p = pane()
            p.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.2))
                .press(forDuration: 0.05, thenDragTo: p.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.55)))
        }
        let tap = { shown(self.app.staticTexts.matching(identifier: "Give this agent a task.")).tap() }
        for (how, act) in [("drag", drag), ("tap", tap)] {
            shown(app.textViews).tap()
            XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5), "the composer raised the keyboard before the \(how)")
            if how == "drag" { shot("keyboard") }
            act()
            XCTAssertTrue(waitGone(app.keyboards.firstMatch), "a \(how) on the transcript hid the keyboard")
        }
        sync("keyboard")

        // 3. The panes button swaps the page for the list; a row opens its pane and closes the list.
        XCTAssertFalse(any("panes list").exists, "the list is closed")
        panes.tap()
        XCTAssertTrue(any("panes list").waitForExistence(timeout: 5), "the panes button opens the list")
        XCTAssertTrue(panes.isSelected, "the button is selected while the list is open")
        XCTAssertTrue((panes.value as? String ?? "").hasSuffix(", open"), "the value says open: \(panes.value as? String ?? "nil")")
        let rows = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'pane row '"))
        XCTAssertTrue(rows.element(boundBy: 1).waitForExistence(timeout: 5), "the list has a row per pane")
        XCTAssertEqual(rows.count, 2)
        XCTAssertEqual(rows.element(boundBy: 0).value as? String, "shown", "the first page is the one shown")
        rows.element(boundBy: 1).tap()
        XCTAssertTrue(waitGone(any("panes list")), "a row tap closes the list")
        expectPages("grid [grid]")
        XCTAssertFalse(panes.isSelected, "the button is not selected once the list closes")
        sync("opened")

        // 4. Spaces -> Orchestrator -> Spaces returns to the same page.
        mode.tap()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Orchestrator'")).firstMatch.waitForExistence(timeout: 5))
        mode.tap()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Spaces'")).firstMatch.waitForExistence(timeout: 5))
        expectPages("grid [grid]")
        sync("roundtrip")

        // 5. The mode survives a relaunch.
        mode.tap()
        // The pairing code is spent; the relaunch connects with the credential the first launch saved.
        app.relaunchFromKeychain()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Orchestrator'")).firstMatch.waitForExistence(timeout: 40), "Orchestrator restored after a relaunch")
        sync("relaunch")
        app.buttons["mode"].tap()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Spaces'")).firstMatch.waitForExistence(timeout: 5))
    }

    private func any(_ id: String) -> XCUIElement { app.descendants(matching: .any)[id].firstMatch }

    private func expectPages(_ value: String) {
        let pages = any("pages")
        XCTAssertTrue(pages.waitForExistence(timeout: 10), "the pages element is behind the top bar")
        let deadline = Date().addingTimeInterval(15)
        while (pages.value as? String) != value && Date() < deadline { Thread.sleep(forTimeInterval: 0.25) }
        XCTAssertEqual(pages.value as? String, value, "pages read '\(value)'")
    }

    private func waitGone(_ element: XCUIElement) -> Bool {
        let deadline = Date().addingTimeInterval(5)
        while element.exists && Date() < deadline { Thread.sleep(forTimeInterval: 0.2) }
        return !element.exists
    }

    /// The rect lines with the title's width left out: the title is the shown mode's name, and holds its place.
    private func placed(_ lines: String, without left: [String] = []) -> String {
        lines.split(separator: "\n").filter { line in !left.contains { line.hasPrefix("\($0) ") } }.map { line -> String in
            var f = line.split(separator: " ").map(String.init)
            if f.first == "mode", f.count == 5 { f[3] = "-" }
            return f.joined(separator: " ")
        }.joined(separator: "\n")
    }

    /// Each control's frame, one `<id> x y w h` line per control, written to the shots folder.
    private func rects(_ state: String) -> String {
        let lines = controls.map { id -> String in
            let element = app.buttons[id].firstMatch
            let f = element.exists ? element.frame : .zero
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
