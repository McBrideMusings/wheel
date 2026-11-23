import XCTest

/// Drives the phone's Orchestrator mode with real taps against a wheeld that
/// `.claude/skills/project-verify/tools/ios-orchestrator.sh` set up: one space holding one agent pane, and no
/// space on the orchestrator folder until the script opens one at the `before` step. The test writes `<sync>/step-<name>` after each step and waits for
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
        XCTAssertTrue(app.buttons["mode"].waitForExistence(timeout: 40))
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 20))

        // With no space on the orchestrator folder the mode names the folder and starts nothing (ADR-0054).
        pick("Orchestrator")
        XCTAssertTrue(any("no orchestrator").waitForExistence(timeout: 20), "the mode says no space is open on the folder")
        XCTAssertTrue(any("orchestrator empty").label.contains("Open a space with an agent pane on"), "the text says what to open: \(any("orchestrator empty").label)")
        XCTAssertTrue(any("orchestrator empty").label.contains("/orchestrator"), "the text names the folder: \(any("orchestrator empty").label)")
        XCTAssertFalse(app.buttons["orchestrator start"].exists, "no Start button")
        XCTAssertFalse(chat.exists, "no chat without a space")
        shot("empty")
        sync("before")

        // The script opened a space on the folder with an agent pane; its chat is the first page, with no pin
        // sheet, no page dots and no agent header row.
        XCTAssertTrue(chat.waitForExistence(timeout: 30), "the orchestrator's chat is the first page")
        XCTAssertTrue(waitFor { !self.any("no orchestrator").exists }, "the empty state gives way to the chat")
        XCTAssertFalse(any("pin sheet").exists, "no pin sheet in Orchestrator mode")
        XCTAssertFalse(any("page dots").exists, "no page dots in Orchestrator mode")
        XCTAssertFalse(chat.buttons["agent-header"].exists, "the chat has no agent header row")
        XCTAssertEqual(shownPage, "Orchestrator", "the chat is the shown page, with no page name under the title")
        XCTAssertTrue(chat.staticTexts["Message"].exists, "the composer says Message")
        XCTAssertTrue(chat.staticTexts["Message the orchestrator."].exists, "the empty transcript says Message the orchestrator.")
        shot("chat")
        sync("chat")

        // One global orchestrator: the title is the mode menu with no space switcher under it, there is no eye, no
        // search and no page handle, and the ⋯ holds its place.
        XCTAssertEqual(title.value as? String, "Orchestrator", "the title is the mode menu on Orchestrator")
        XCTAssertFalse(app.buttons["space switcher"].exists, "no space switcher in Orchestrator mode")
        XCTAssertFalse(app.buttons["agents"].exists, "Orchestrator mode has no eye")
        XCTAssertFalse(app.buttons["search"].exists, "Orchestrator mode has no search (wheel-eg4e.19.27)")
        let here = (title.frame, app.buttons["Pane menu"].frame)
        XCTAssertFalse(app.otherElements["page handle"].exists, "Orchestrator mode has no page handle")
        // The model is the title's one detail, on the chat.
        XCTAssertTrue(app.buttons["model menu"].waitForExistence(timeout: 10), "the model menu is under the title")
        XCTAssertGreaterThan(app.buttons["model menu"].frame.minY, title.frame.maxY - 1, "the model sits under the title")
        sync("chrome")

        // The composer's keyboard button (wheel-eg4e.19.25): it shows while the keyboard is up and hides it.
        XCTAssertFalse(app.buttons["hide keyboard"].exists, "no keyboard button while the keyboard is down")
        // The .19.31 placeholders (wheel-eg4e.19.41): a mic, and voice mode in Send's place while the field is empty.
        XCTAssertTrue(chat.buttons["dictate"].exists, "the composer has the dictation button")
        XCTAssertTrue(chat.buttons["voice mode"].exists, "an empty field shows voice mode")
        XCTAssertFalse(chat.buttons["Send"].exists, "an empty field shows no Send")
        measure("keyboard-down")
        let chatDown = chat.frame.height
        chat.textViews.firstMatch.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5), "the composer raised the keyboard")
        let hide = app.buttons["hide keyboard"]
        XCTAssertTrue(hide.waitForExistence(timeout: 5), "the composer shows the keyboard button")
        // The composer sits on the keyboard: no page handle between them. XCUITest's keyboard frame starts under the
        // suggestion bar, so the script compares the chat's bottom with the top the app's keyboard notification gave.
        Thread.sleep(forTimeInterval: 1)
        XCTAssertFalse(app.otherElements["page handle"].exists, "no page handle while the keyboard is up")
        measure("keyboard-up")
        try? "\(Int(chat.frame.maxY.rounded()))".write(toFile: "\(env["WHEEL_SYNC_DIR"] ?? "")/chat-bottom", atomically: true, encoding: .utf8)
        sync("keyboard")
        shot("keyboard")
        hide.tap()
        XCTAssertTrue(waitFor { !self.app.keyboards.firstMatch.exists }, "the button hid the keyboard")
        XCTAssertFalse(app.buttons["hide keyboard"].exists, "the button goes with the keyboard")

        // The composer's + menu (wheel-eg4e.19.35): Model picks another model, and the check follows what the agent
        // took; Clear chat asks first (wheel-eg4e.19.38), cancelling keeps the transcript, and confirming empties it.
        chat.textViews.firstMatch.tap()
        chat.textViews.firstMatch.typeText("hello")
        shot("typed")
        chat.buttons["Send"].tap()
        XCTAssertTrue(waitFor { !self.chat.staticTexts["Message the orchestrator."].exists }, "the message is in the transcript")
        // With the keyboard up, the + menu draws above it, and the composer lets go of it, so a pick never raises it
        // again (wheel-eg4e.19.40).
        if !app.keyboards.firstMatch.exists { chat.textViews.firstMatch.tap() }
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5), "the keyboard is up before the + menu opens")
        let keyboardTop = app.keyboards.firstMatch.frame.minY
        chat.buttons["Attach"].tap()
        XCTAssertTrue(app.buttons["Clear chat"].waitForExistence(timeout: 5), "the + menu opens")
        XCTAssertEqual(app.buttons.matching(identifier: "model menu").count, 1, "the + menu holds no Model; only the title's")
        XCTAssertTrue(app.buttons["Clear chat"].isHittable, "Clear chat is on top, not behind the keyboard")
        XCTAssertLessThanOrEqual(app.buttons["Clear chat"].frame.maxY, keyboardTop, "the menu ends above the keyboard")
        XCTAssertFalse(app.buttons["hide keyboard"].exists, "the composer let go of the keyboard")
        XCTAssertTrue(app.buttons["Clear chat"].exists, "the + menu has Clear chat")
        shot("plus-menu")
        let outsideMenu = app.otherElements["PopoverDismissRegion"]
        if outsideMenu.exists { outsideMenu.tap() } else { app.windows.firstMatch.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.35)).tap() }
        XCTAssertTrue(waitFor { !self.app.buttons["Clear chat"].exists }, "the + menu closed")
        let modelMenu = app.buttons["model menu"]
        modelMenu.tap()
        // The fake agent's models, by the names its `initialize` gives them.
        let choices = app.buttons.matching(NSPredicate(format: "label IN %@", ["Default", "Sonnet", "Haiku"]))
        XCTAssertTrue(choices.firstMatch.waitForExistence(timeout: 5), "Model lists the agent's models")
        XCTAssertGreaterThan(choices.count, 1, "Model lists more than one model")
        shot("model-menu")
        // The last choice: the fake agent starts on its first, `default`.
        let picked = choices.element(boundBy: choices.count - 1)
        let value = picked.label.lowercased()
        picked.tap()
        try? value.write(toFile: "\(env["WHEEL_SYNC_DIR"] ?? "")/picked-model", atomically: true, encoding: .utf8)
        Thread.sleep(forTimeInterval: 1)
        XCTAssertFalse(app.keyboards.firstMatch.exists, "a Model pick leaves the keyboard down")
        chat.buttons["Attach"].tap()
        XCTAssertTrue(app.buttons["Clear chat"].waitForExistence(timeout: 5), "the + menu opens again")
        app.buttons["Clear chat"].tap()
        let confirm = app.buttons["confirm clear chat"].firstMatch
        XCTAssertTrue(confirm.waitForExistence(timeout: 5), "Clear chat asks before clearing")
        shot("clear-confirm")
        // The question is a popover on iOS 26, with no Cancel button: a tap outside it cancels.
        let outside = app.otherElements["PopoverDismissRegion"]
        if outside.exists { outside.tap() } else { app.buttons["Cancel"].tap() }
        XCTAssertTrue(waitFor { !confirm.exists }, "a tap outside closes the question")
        Thread.sleep(forTimeInterval: 1)
        XCTAssertFalse(chat.staticTexts["Message the orchestrator."].exists, "cancelling kept the transcript")
        chat.buttons["Attach"].tap()
        XCTAssertTrue(app.buttons["Clear chat"].waitForExistence(timeout: 5), "the + menu opens a third time")
        app.buttons["Clear chat"].tap()
        XCTAssertTrue(confirm.waitForExistence(timeout: 5), "Clear chat asks again")
        confirm.tap()
        XCTAssertTrue(chat.staticTexts["Message the orchestrator."].waitForExistence(timeout: 10), "Clear chat emptied the transcript")
        sync("model")

        // 1. Full screen under the top bar, no modal: the chat starts under the title and its composer sits on the home
        // indicator, with no bar under it.
        let window = app.windows.firstMatch.frame
        XCTAssertLessThan(title.frame.maxY, chat.frame.minY + 1, "the chat lies under the top bar")
        XCTAssertGreaterThan(chat.buttons["Attach"].frame.maxY, window.maxY - 60, "the composer sits on the home indicator: \(chat.buttons["Attach"].frame) in \(window)")
        XCTAssertFalse(app.otherElements["PopoverDismissRegion"].exists, "no modal is presented")
        sync("fullscreen")

        // The orchestrator opens a pane in its space: it is a page after the chat, a swipe shows it under its own name,
        // and still no dots show. One left swipe from the chat shows the space's first pane (design M9).
        sync("spawn")
        swipePage()
        XCTAssertTrue(waitFor { self.shownPage != "Orchestrator" }, "a left swipe from the chat shows the first pane")
        XCTAssertFalse(chat.isHittable, "the chat is off screen after one swipe")
        var swipes = 1
        while shownPage != "notes" && swipes < 4 {
            swipePage()
            Thread.sleep(forTimeInterval: 1)
            swipes += 1
        }
        XCTAssertEqual(shownPage, "notes", "a swipe shows the pane the orchestrator opened, named under the title")
        XCTAssertEqual(title.value as? String, "Orchestrator", "the title stays the mode menu on a pane page")
        XCTAssertFalse(any("page dots").exists, "no page dots with two pages")
        XCTAssertFalse(chat.isHittable, "the chat is off screen")
        // The floating button (wheel-eg4e.19.34): on every page but the chat, its value the orchestrator's state. A
        // drag to the left edge snaps it to the left; a tap shows the chat.
        XCTAssertTrue(float.waitForExistence(timeout: 5), "the floating button shows on a page that is not the chat")
        XCTAssertTrue(["idle", "working", "needs you"].contains(float.value as? String ?? ""), "the button carries the state: \(String(describing: float.value))")
        let win = app.windows.firstMatch
        XCTAssertGreaterThan(float.frame.midX, win.frame.midX, "the button starts at the right")
        XCTAssertLessThan(float.frame.maxY, win.frame.maxY - 34, "the button sits above the home indicator: \(float.frame)")
        shot("page")
        float.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            .press(forDuration: 0.2, thenDragTo: win.coordinate(withNormalizedOffset: CGVector(dx: 0.08, dy: 0.45)))
        XCTAssertTrue(waitFor { self.float.frame.midX < win.frame.midX }, "the drag snapped the button to the left: \(float.frame)")
        XCTAssertGreaterThan(float.frame.minX, 0, "the button stays on screen: \(float.frame)")
        shot("float-left")
        float.tap()
        XCTAssertTrue(waitFor { self.shownPage == "Orchestrator" }, "a tap on the button shows the chat: \(shownPage)")
        XCTAssertFalse(float.exists, "no button over the chat")
        sync("page")

        // Pane links (wheel-eg4e.19.37): a pane in the Orchestrator space is a link that slides to its page; a pane in
        // another space is plain text.
        let ids = (try? String(contentsOfFile: "\(env["WHEEL_SYNC_DIR"] ?? "")/link-panes", encoding: .utf8))?.split(separator: " ").map(String.init) ?? []
        XCTAssertEqual(ids.count, 2, "the script named the two panes")
        let (inSpace, elsewhere) = (ids.first ?? "", ids.last ?? "")
        chat.textViews.firstMatch.tap()
        chat.textViews.firstMatch.typeText("look at \(inSpace) and \(elsewhere)")
        chat.buttons["Send"].tap()
        dismissKeyboard()
        let link = chat.links.matching(NSPredicate(format: "label BEGINSWITH %@", "\(inSpace) ")).firstMatch
        XCTAssertTrue(link.waitForExistence(timeout: 15), "the reply links \(inSpace)")
        XCTAssertFalse(chat.links.matching(NSPredicate(format: "label BEGINSWITH %@", "\(elsewhere) ")).firstMatch.exists, "\(elsewhere), in another space, is no link")
        shot("links")
        link.tap()
        XCTAssertTrue(waitFor { self.shownPage == "notes" }, "the link slid to the notes page: \(shownPage)")
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Orchestrator'")).firstMatch.exists, "still in Orchestrator mode")
        float.tap()
        XCTAssertTrue(waitFor { self.shownPage == "Orchestrator" }, "back on the chat: \(shownPage)")
        sync("link")

        // The scroll-to-bottom arrow (wheel-eg4e.19.32): it shows once the transcript leaves its end, and a tap goes back.
        chat.textViews.firstMatch.tap()
        chat.textViews.firstMatch.typeText(String(repeating: "a long line for the scroll check ", count: 50))
        chat.buttons["Send"].tap()
        dismissKeyboard()
        let arrow = app.buttons["scroll to bottom"]
        Thread.sleep(forTimeInterval: 3)
        XCTAssertFalse(arrow.exists, "no arrow at the newest message")
        chat.collectionViews.firstMatch.swipeDown(velocity: .fast)
        XCTAssertTrue(arrow.waitForExistence(timeout: 5), "the arrow shows once the transcript leaves its end")
        shot("scroll-arrow")
        arrow.tap()
        XCTAssertTrue(waitFor { !arrow.exists }, "the arrow goes once the newest message is in view")
        sync("scroll")

        // The orchestrator's space is an ordinary space: Spaces mode lists it in the title menu.
        pick("Spaces")
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["agents"].exists, "Spaces mode keeps the eye")
        XCTAssertEqual(title.frame.minX, here.0.minX, accuracy: 0.5, "the mode menu holds its place across the modes")
        XCTAssertEqual(title.frame.midY, here.0.midY, accuracy: 0.5, "the mode menu holds its row across the modes")
        XCTAssertEqual(app.buttons["Pane menu"].frame, here.1, "the ⋯ holds its place across the modes")
        app.buttons["space switcher"].tap()
        XCTAssertTrue(app.buttons["space wheel"].waitForExistence(timeout: 10), "the title menu lists the first space")
        XCTAssertTrue(app.buttons["space orchestrator"].exists, "the title menu lists the orchestrator's space")
        shot("lists")
        app.swipeDown(velocity: .fast)
        XCTAssertTrue(waitFor { !self.app.buttons["space wheel"].isHittable })
        sync("lists")

        // 3 (wheel-eg4e.19.27). The person only talks to the orchestrator: the ⋯ holds Settings alone.
        pick("Orchestrator")
        XCTAssertTrue(chat.waitForExistence(timeout: 5), "the chat shows on the second open")
        app.buttons["Pane menu"].tap()
        XCTAssertTrue(app.buttons["Settings"].waitForExistence(timeout: 5), "the ⋯ holds Settings")
        for row in ["Split Right", "Split Down", "Pin", "Close", "Pane info"] {
            XCTAssertFalse(app.buttons[row].exists, "the ⋯ offers no \(row) in Orchestrator mode")
        }
        shot("menu")
        app.buttons["Settings"].tap()
        XCTAssertTrue(app.buttons["Done"].waitForExistence(timeout: 5), "Settings opened")
        app.buttons["Done"].tap()
        // The phone reopens to the mode and page it showed last (wheel-eg4e.19.36): leave it on the notes page.
        var out = 0
        while shownPage != "notes" && out < 4 {
            swipePage()
            Thread.sleep(forTimeInterval: 1)
            out += 1
        }
        XCTAssertEqual(shownPage, "notes", "the notes page shows before the relaunch")
        app.terminate()
        // The pairing code is spent; the relaunch connects with the credential the first launch saved.
        app.launchEnvironment.removeValue(forKey: "WHEEL_PAIR_JSON")
        app.launch()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Orchestrator'")).firstMatch.waitForExistence(timeout: 40), "Orchestrator mode restored")
        XCTAssertTrue(waitFor { self.shownPage == "notes" }, "the relaunch shows the notes page: \(shownPage)")
        XCTAssertTrue(float.waitForExistence(timeout: 5), "the floating button is over the notes page")
        XCTAssertLessThan(float.frame.midX, app.windows.firstMatch.frame.midX, "the button is still at the left after a relaunch")
        XCTAssertFalse(any("pin sheet").exists, "no pin sheet after a relaunch")
        shot("relaunch")
        sync("relaunch")
        pick("Spaces")
    }

    private var chat: XCUIElement { app.otherElements["orchestrator chat"] }
    private var float: XCUIElement { app.buttons["orchestrator float"] }
    /// The bar's title: the mode menu.
    private var title: XCUIElement { app.buttons["mode"] }
    /// The shown page's name under the title, "Orchestrator" on the chat, which shows none.
    private var shownPage: String {
        let name = any("page name")
        return name.exists ? name.label : "Orchestrator"
    }

    private func any(_ id: String) -> XCUIElement { app.descendants(matching: .any)[id] }

    /// Pick `title` from the mode menu and wait for the button to show it.
    private func pick(_ title: String) {
        app.buttons["mode"].tap()
        let row = app.buttons.matching(NSPredicate(format: "label == %@ AND identifier != 'agents' AND identifier != 'mode'", title)).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 5), "the mode menu has \(title)")
        row.tap()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == %@", title)).firstMatch.waitForExistence(timeout: 5), "the button shows \(title)")
    }

    /// Close the keyboard with the composer's button and wait until it is gone.
    private func dismissKeyboard() {
        for _ in 0 ..< 3 where app.keyboards.firstMatch.exists {
            if app.buttons["hide keyboard"].exists { app.buttons["hide keyboard"].tap() } else { chat.collectionViews.firstMatch.swipeDown() }
            _ = waitFor { !self.app.keyboards.firstMatch.exists }
        }
    }

    /// One page to the right, by a drag across the middle of the pager.
    private func swipePage() {
        let win = app.windows.firstMatch
        win.coordinate(withNormalizedOffset: CGVector(dx: 0.85, dy: 0.5))
            .press(forDuration: 0.05, thenDragTo: win.coordinate(withNormalizedOffset: CGVector(dx: 0.1, dy: 0.5)))
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

    /// The frames the layout checks compare, one `<id> minX minY maxX maxY` line each, written to `<shots>/measure-<label>.txt`.
    private func measure(_ label: String) {
        guard let dir = env["WHEEL_SHOTS"] else { return }
        let items: [(String, XCUIElement)] = [
            ("window", app.windows.firstMatch), ("chat", chat), ("field", chat.textViews.firstMatch),
            ("attach", chat.buttons["Attach"]), ("mode", app.buttons["mode"]),
            ("menu", app.buttons["Pane menu"]), ("keyboard", app.keyboards.firstMatch),
        ]
        let lines = items.map { id, e -> String in
            let f = e.exists ? e.frame : .zero
            return "\(id) \(Int(f.minX)) \(Int(f.minY)) \(Int(f.maxX)) \(Int(f.maxY))"
        }
        try? lines.joined(separator: "\n").write(toFile: "\(dir)/measure-\(label).txt", atomically: true, encoding: .utf8)
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
