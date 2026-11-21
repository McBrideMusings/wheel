import XCTest

/// Drives the iPhone pin sheet with real taps and drags against a wheeld that
/// `.claude/skills/project-verify/tools/ios-pin.sh` set up: one space holding an agent, a terminal and a web pane,
/// whose ids come in `WHEEL_PIN_PANES` in carousel order. The test writes `<sync>/step-<name>` after each step and
/// waits for `<sync>/ack-<name>`; shots of every sheet position go to `<shots>/pin-<state>-<theme>.png`.
final class PinUITests: XCTestCase {
    private let env = ProcessInfo.processInfo.environment
    private var app: XCUIApplication!

    override func setUp() { continueAfterFailure = false }

    func testPinSheet() throws {
        let pair = try XCTUnwrap(env["WHEEL_PAIR_JSON"], "WHEEL_PAIR_JSON")
        let ids = try XCTUnwrap(env["WHEEL_PIN_PANES"], "WHEEL_PIN_PANES").split(separator: ",").map(String.init)
        XCTAssertEqual(ids.count, 3)
        app = XCUIApplication()
        app.launchEnvironment["WHEEL_PAIR_JSON"] = pair
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 40))
        XCTAssertTrue(app.otherElements.matching(NSPredicate(format: "identifier == 'page dots' AND value == '[grid] grid grid'")).firstMatch.waitForExistence(timeout: 20))
        XCTAssertFalse(sheet.exists, "no pins, no sheet")

        // 1, 2. Pinning the agent takes it out of the carousel and shows it as the sheet at low.
        pin()
        XCTAssertTrue(at("low"), "a pinned pane shows at low")
        XCTAssertTrue(dots("[grid] grid"), "the pinned pane left the carousel")
        let agent = try XCTUnwrap(menuValue, "the sheet names the agent")
        shot("low")
        sync("pinned")

        // 3, 4. A drag on the header moves it through full, high, low and hidden; full covers everything under the bar.
        let title = app.buttons["title"].frame
        drag(toY: title.maxY + 4)
        XCTAssertTrue(at("full"))
        // At full the sheet fills the area under the bar, so its frame is the area the other positions divide.
        area = sheet.frame
        let header = app.otherElements["pin sheet header"].frame
        XCTAssertGreaterThanOrEqual(header.minY, title.maxY, "the top bar stays uncovered: \(header) under \(title)")
        XCTAssertLessThanOrEqual(header.minY, title.maxY + 20, "at full the sheet reaches the top bar: \(header) under \(title)")
        shot("full")
        drag(toFraction: 0.7)
        XCTAssertTrue(at("high"))
        shot("high")
        drag(toFraction: 0.38)
        XCTAssertTrue(at("low"))
        drag(toFraction: 0)
        XCTAssertTrue(at("hidden"))
        // 5. At hidden a round button shows; a tap brings the sheet back at low.
        let show = app.buttons["sheet show"]
        XCTAssertTrue(show.waitForExistence(timeout: 5), "the hidden sheet leaves a button")
        XCTAssertFalse(app.otherElements["pin sheet header"].exists, "the hidden sheet is below the bottom edge")
        shot("hidden")
        sync("positions")
        show.tap()
        XCTAssertTrue(at("low"))
        XCTAssertFalse(show.exists)
        sync("shown")

        // 2. A terminal and a web pane pin too; with every pane pinned the carousel says so.
        pin()
        XCTAssertTrue(dots(""), "one page left: no dots")
        let terminal = try XCTUnwrap(menuValue, "the sheet names the terminal")
        sync("terminal")
        // Hidden, the sheet's terminal stops streaming; brought back, it streams again.
        drag(toFraction: 0)
        XCTAssertTrue(at("hidden"))
        sync("terminal-hidden")
        app.buttons["sheet show"].tap()
        XCTAssertTrue(at("low"))
        sync("terminal-shown")
        pin()
        XCTAssertTrue(app.descendants(matching: .any)["no panes"].waitForExistence(timeout: 5), "every pane is pinned")
        XCTAssertEqual(menuValue, "site", "the sheet shows the web pane pinned last")
        shot("web")
        sync("web")

        // 6. With several pinned panes the header's menu switches the sheet among them.
        openMenu()
        // A menu row keeps its title and drops its identifier, so rows are found by the names the header showed.
        let first = menuRow(agent)
        XCTAssertTrue(first.waitForExistence(timeout: 5), "the menu lists the agent")
        XCTAssertTrue(menuRow(terminal).exists && menuRow("site").exists, "the menu lists every pinned pane")
        shot("menu")
        first.tap()
        XCTAssertTrue(waitFor { self.menuValue == agent }, "the sheet switched to the agent")
        sync("switched")

        // Unpinning gives the pane back to the carousel; the sheet shows the next pinned one.
        openMenu()
        let unpin = menuRow("Unpin")
        XCTAssertTrue(unpin.waitForExistence(timeout: 5))
        unpin.tap()
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.descendants(matching: .any)["no panes"].exists, "the agent is a page again")
        XCTAssertTrue(at("low"))
        sync("unpinned")

        // 8. Pins survive a relaunch; the pairing code is spent, so the relaunch uses the saved credential.
        app.terminate()
        app.launchEnvironment.removeValue(forKey: "WHEEL_PAIR_JSON")
        app.launch()
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 40))
        XCTAssertTrue(at("low"), "the pins came back with the sheet")
        // The script closes the web pane here: a pinned pane that closes leaves the pins.
        sync("relaunch")
        XCTAssertTrue(waitFor { self.menuValue == terminal }, "the sheet shows the terminal once the web pane is gone")
        openMenu()
        XCTAssertTrue(menuRow("Unpin").waitForExistence(timeout: 5))
        XCTAssertFalse(menuRow("site").exists, "the closed pane left the menu")
        menuRow("Unpin").tap()
        XCTAssertTrue(waitFor { !self.sheet.exists }, "no pins, no sheet")
        XCTAssertTrue(dots("[grid] grid") || dots("grid [grid]"), "both panes are pages again")
        sync("none")
    }

    /// A row of the open header menu by its title; the header's own button carries the same name as its value only.
    private func menuRow(_ title: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label == %@ AND identifier != 'pin sheet menu'", title)).firstMatch
    }

    /// The header's name opens its menu; the element named `pin sheet menu` spans the header, so the tap goes to the name inside it.
    private func openMenu() {
        app.buttons["pin sheet menu"].buttons.firstMatch.tap()
    }

    private var sheet: XCUIElement { app.otherElements["pin sheet"] }
    /// The area under the top bar the sheet's positions divide, read at full.
    private var area = CGRect.zero
    private var menuValue: String? { app.buttons["pin sheet menu"].value as? String }

    private func at(_ position: String) -> Bool {
        app.otherElements.matching(NSPredicate(format: "identifier == 'pin sheet' AND value == %@", position)).firstMatch.waitForExistence(timeout: 5)
    }

    private func dots(_ value: String) -> Bool {
        if value.isEmpty { return waitFor { !self.app.otherElements["page dots"].exists } }
        return app.otherElements.matching(NSPredicate(format: "identifier == 'page dots' AND value == %@", value)).firstMatch.waitForExistence(timeout: 5)
    }

    /// The ⋯ on the page shown, then Pin.
    private func pin() {
        app.buttons["Pane menu"].tap()
        let pin = app.buttons["Pin"]
        XCTAssertTrue(pin.waitForExistence(timeout: 5), "the pane menu offers Pin")
        pin.tap()
    }

    /// Drag the header so the sheet's top ends `fraction` of the area up from its bottom.
    private func drag(toFraction fraction: CGFloat) {
        drag(toY: min(area.maxY - fraction * area.height, area.maxY - 2))
    }

    /// Drag the header so its top ends at `y` on the screen.
    private func drag(toY y: CGFloat) {
        let header = app.otherElements["pin sheet header"]
        XCTAssertTrue(header.waitForExistence(timeout: 5))
        let frame = header.frame
        let start = header.coordinate(withNormalizedOffset: CGVector(dx: 0.7, dy: 0.5))
        let end = app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: frame.minX + frame.width * 0.7, dy: y + frame.height / 2))
        start.press(forDuration: 0.2, thenDragTo: end, withVelocity: .slow, thenHoldForDuration: 0.3)
    }

    private func waitFor(_ condition: @escaping () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(10)
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
            try? XCUIScreen.main.screenshot().pngRepresentation.write(to: URL(fileURLWithPath: "\(dir)/pin-\(name)-\(theme).png"))
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
