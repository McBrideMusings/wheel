import XCTest

/// Drives the phone's Orchestrator mode with real taps against a wheeld that
/// `.claude/skills/project-verify/tools/ios-orchestrator.sh` set up: one space holding one agent pane, and no
/// Orchestrator space until the mode opens. The test writes `<sync>/step-<name>` after each step and waits for
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
        XCTAssertTrue(app.buttons["title"].waitForExistence(timeout: 40))
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 20))

        // Opening the mode with no global orchestrator starts one with no tap (wheel-eg4e.19.23); there is no Start.
        sync("before")
        pick("Orchestrator")
        XCTAssertFalse(app.buttons["orchestrator start"].exists, "no Start button")

        // 2. The start makes the Orchestrator space; the global orchestrator's chat is its first page, with no pin
        // sheet, no page dots and no agent header row (wheel-eg4e.19.33).
        XCTAssertTrue(chat.waitForExistence(timeout: 30), "the orchestrator's chat is the first page")
        XCTAssertFalse(any("no orchestrator").exists)
        XCTAssertFalse(any("pin sheet").exists, "no pin sheet in Orchestrator mode")
        XCTAssertFalse(any("page dots").exists, "no page dots in Orchestrator mode")
        XCTAssertFalse(chat.buttons["agent-header"].exists, "the chat has no agent header row")
        XCTAssertEqual(title.label, "Global orchestrator", "the title names the orchestrator")
        XCTAssertTrue(chat.staticTexts["Message"].exists, "the composer says Message")
        shot("chat")
        sync("chat")

        // One global orchestrator: the title is a label with no menu, there is no eye, and the handle's swipe up opens
        // nothing. Search and the ⋯ hold the places they have in Spaces mode.
        XCTAssertFalse(app.buttons["title"].exists, "the title is no button in Orchestrator mode")
        XCTAssertFalse(app.buttons["agents"].exists, "Orchestrator mode has no eye")
        XCTAssertFalse(app.buttons["search"].exists, "Orchestrator mode has no search (wheel-eg4e.19.27)")
        let here = (app.buttons["mode"].frame, app.buttons["Pane menu"].frame)
        title.tap()
        Thread.sleep(forTimeInterval: 1)
        XCTAssertFalse(app.buttons["space wheel"].exists, "a tap on the title opens no menu")
        let handle = app.otherElements["page handle"]
        handle.press(forDuration: 0.05, thenDragTo: app.buttons["mode"])
        Thread.sleep(forTimeInterval: 1)
        XCTAssertFalse(app.buttons["space wheel"].exists, "a swipe up on the handle opens no sheet")
        sync("chrome")

        // The eye's slot holds Tasks (wheel-eg4e.20.3): its sheet lists the task file the check wrote, and Done
        // writes the person's done.
        let tasks = app.buttons["tasks"]
        XCTAssertTrue(tasks.waitForExistence(timeout: 10), "Orchestrator mode has a Tasks button")
        tasks.tap()
        XCTAssertTrue(any("task tc1").waitForExistence(timeout: 10), "the Tasks sheet lists the task file")
        XCTAssertTrue(app.staticTexts["Check the Tasks sheet"].exists, "the task names what was asked")
        XCTAssertEqual(any("task status tc1").label, "Ready for review")
        shot("tasks")
        app.buttons["task done tc1"].tap()
        XCTAssertTrue(waitFor { !self.app.buttons["task done tc1"].exists }, "Done goes once the task is done")
        sync("tasks")
        app.swipeDown(velocity: .fast)
        XCTAssertTrue(waitFor { !self.any("tasks list").exists }, "the sheet closes")

        // The composer's keyboard button (wheel-eg4e.19.25): it shows while the keyboard is up and hides it.
        XCTAssertFalse(app.buttons["hide keyboard"].exists, "no keyboard button while the keyboard is down")
        // The .19.31 placeholders (wheel-eg4e.19.41): a mic, and voice mode in Send's place while the field is empty.
        XCTAssertTrue(chat.buttons["dictate"].exists, "the composer has the dictation button")
        XCTAssertTrue(chat.buttons["voice mode"].exists, "an empty field shows voice mode")
        XCTAssertFalse(chat.buttons["Send"].exists, "an empty field shows no Send")
        let chatDown = chat.frame.height
        chat.textViews.firstMatch.tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5), "the composer raised the keyboard")
        let hide = app.buttons["hide keyboard"]
        XCTAssertTrue(hide.waitForExistence(timeout: 5), "the composer shows the keyboard button")
        // The composer sits on the keyboard: no page handle between them. XCUITest's keyboard frame starts under the
        // suggestion bar, so the script compares the chat's bottom with the top the app's keyboard notification gave.
        Thread.sleep(forTimeInterval: 1)
        XCTAssertFalse(app.otherElements["page handle"].exists, "no page handle while the keyboard is up")
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
        chat.buttons["Send"].tap()
        XCTAssertTrue(waitFor { !self.chat.staticTexts["Message the global orchestrator."].exists }, "the message is in the transcript")
        // With the keyboard up, the + menu draws above it, and the composer lets go of it, so a pick never raises it
        // again (wheel-eg4e.19.40).
        if !app.keyboards.firstMatch.exists { chat.textViews.firstMatch.tap() }
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5), "the keyboard is up before the + menu opens")
        let keyboardTop = app.keyboards.firstMatch.frame.minY
        chat.buttons["Attach"].tap()
        let modelMenu = app.buttons["model menu"]
        XCTAssertTrue(modelMenu.waitForExistence(timeout: 5), "the + menu has Model")
        XCTAssertTrue(app.buttons["Clear chat"].isHittable, "Clear chat is on top, not behind the keyboard")
        XCTAssertLessThanOrEqual(app.buttons["Clear chat"].frame.maxY, keyboardTop, "the menu ends above the keyboard")
        XCTAssertFalse(app.buttons["hide keyboard"].exists, "the composer let go of the keyboard")
        XCTAssertTrue(app.buttons["Clear chat"].exists, "the + menu has Clear chat")
        shot("plus-menu")
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
        XCTAssertFalse(chat.staticTexts["Message the global orchestrator."].exists, "cancelling kept the transcript")
        chat.buttons["Attach"].tap()
        XCTAssertTrue(app.buttons["Clear chat"].waitForExistence(timeout: 5), "the + menu opens a third time")
        app.buttons["Clear chat"].tap()
        XCTAssertTrue(confirm.waitForExistence(timeout: 5), "Clear chat asks again")
        confirm.tap()
        XCTAssertTrue(chat.staticTexts["Message the global orchestrator."].waitForExistence(timeout: 10), "Clear chat emptied the transcript")
        sync("model")

        // 1. Full screen under the top bar, no modal: the chat starts under the title and the handle reaches the
        // bottom edge.
        let window = app.windows.firstMatch.frame
        XCTAssertLessThan(title.frame.maxY, chat.frame.minY + 1, "the chat lies under the top bar")
        XCTAssertGreaterThan(handle.frame.maxY, window.maxY - 60, "the handle reaches the bottom edge: \(handle.frame) in \(window)")
        XCTAssertFalse(app.otherElements["PopoverDismissRegion"].exists, "no modal is presented")
        sync("fullscreen")

        // The orchestrator opens a pane in its space: it is a page after the chat, a swipe shows it under its own name,
        // and still no dots show.
        sync("spawn")
        var swipes = 0
        while title.label != "notes" && swipes < 4 {
            handle.swipeLeft()
            Thread.sleep(forTimeInterval: 1)
            swipes += 1
        }
        XCTAssertEqual(title.label, "notes", "a swipe shows the pane the orchestrator opened, named in the title")
        XCTAssertFalse(any("page dots").exists, "no page dots with two pages")
        XCTAssertFalse(chat.isHittable, "the chat is off screen")
        // The floating button (wheel-eg4e.19.34): on every page but the chat, its value the orchestrator's state. A
        // drag to the left edge snaps it to the left; a tap shows the chat.
        XCTAssertTrue(float.waitForExistence(timeout: 5), "the floating button shows on a page that is not the chat")
        XCTAssertTrue(["idle", "working", "needs you"].contains(float.value as? String ?? ""), "the button carries the state: \(String(describing: float.value))")
        let win = app.windows.firstMatch
        XCTAssertGreaterThan(float.frame.midX, win.frame.midX, "the button starts at the right")
        XCTAssertLessThan(float.frame.maxY, handle.frame.minY, "the button sits above the bottom bar: \(float.frame) over \(handle.frame)")
        shot("page")
        float.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            .press(forDuration: 0.2, thenDragTo: win.coordinate(withNormalizedOffset: CGVector(dx: 0.08, dy: 0.45)))
        XCTAssertTrue(waitFor { self.float.frame.midX < win.frame.midX }, "the drag snapped the button to the left: \(float.frame)")
        XCTAssertGreaterThan(float.frame.minX, 0, "the button stays on screen: \(float.frame)")
        shot("float-left")
        float.tap()
        XCTAssertTrue(waitFor { self.title.label == "Global orchestrator" }, "a tap on the button shows the chat: \(title.label)")
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
        XCTAssertTrue(waitFor { self.title.label == "notes" }, "the link slid to the notes page: \(title.label)")
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Orchestrator'")).firstMatch.exists, "still in Orchestrator mode")
        float.tap()
        XCTAssertTrue(waitFor { self.title.label == "Global orchestrator" }, "back on the chat: \(title.label)")
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

        // 4. The Orchestrator space is in no Spaces mode list: the title menu lists the other space and session only.
        pick("Spaces")
        XCTAssertTrue(app.buttons["Pane menu"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["agents"].exists, "Spaces mode keeps the eye")
        XCTAssertEqual(app.buttons["mode"].frame, here.0, "the mode button holds its place across the modes")
        XCTAssertEqual(app.buttons["Pane menu"].frame, here.1, "the ⋯ holds its place across the modes")
        app.buttons["title"].tap()
        XCTAssertTrue(app.buttons["space wheel"].waitForExistence(timeout: 10), "the title menu lists the Spaces mode space")
        XCTAssertFalse(app.buttons["space Orchestrator"].exists, "the title menu lists no Orchestrator space")
        XCTAssertFalse(app.buttons["session Orchestrator"].exists, "the session bar lists no Orchestrator session")
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
        while title.label != "notes" && out < 4 {
            handle.swipeLeft()
            Thread.sleep(forTimeInterval: 1)
            out += 1
        }
        XCTAssertEqual(title.label, "notes", "the notes page shows before the relaunch")
        app.terminate()
        // The pairing code is spent; the relaunch connects with the credential the first launch saved.
        app.launchEnvironment.removeValue(forKey: "WHEEL_PAIR_JSON")
        app.launch()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "identifier == 'mode' AND value == 'Orchestrator'")).firstMatch.waitForExistence(timeout: 40), "Orchestrator mode restored")
        XCTAssertTrue(waitFor { self.title.label == "notes" }, "the relaunch shows the notes page: \(title.label)")
        XCTAssertTrue(float.waitForExistence(timeout: 5), "the floating button is over the notes page")
        XCTAssertLessThan(float.frame.midX, app.windows.firstMatch.frame.midX, "the button is still at the left after a relaunch")
        XCTAssertFalse(any("pin sheet").exists, "no pin sheet after a relaunch")
        shot("relaunch")
        sync("relaunch")
        pick("Spaces")
    }

    private var chat: XCUIElement { app.otherElements["orchestrator chat"] }
    private var float: XCUIElement { app.buttons["orchestrator float"] }
    private var title: XCUIElement { app.descendants(matching: .any)["title"].firstMatch }

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
