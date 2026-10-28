import AppKit
import SwiftUI
import WheelKit

/// Every built-in action by id: its title, its built-in key and what it does. The app's menus, the
/// command palette and the debug endpoint all run an action through its entry here, so an action
/// has one procedure. Which key runs it is the model's `keymap`: these keys with the `keybind`
/// setting applied over them. The keys follow Ghostty's: a Ghostty tab is a space, a split a pane.
@MainActor
enum CommandActions {
    struct Entry {
        let id: String
        let title: String
        /// The built-in key, in the spelling wheeld stores a `keybind` trigger in.
        let keys: String?
        /// Whether the action means anything now (a pane action needs a space with a focused pane).
        let enabled: @MainActor (AppModel) -> Bool
        let run: @MainActor (AppModel) -> Void

        init(id: String, title: String, keys: String? = nil,
             enabled: @escaping @MainActor (AppModel) -> Bool, run: @escaping @MainActor (AppModel) -> Void) {
            self.id = id
            self.title = title
            self.keys = keys
            self.enabled = enabled
            self.run = run
        }

        var chord: KeyChord? {
            keys.map { keys in
                guard let chord = KeyChord(keys) else { preconditionFailure("built-in key \(keys) is not in wheeld's spelling") }
                return chord
            }
        }
    }

    /// The pane an action on "the" pane means: the active space's focused one.
    private static func focusedPane(_ model: AppModel) -> String? { model.activeSpace?.focus }

    private static func focusedKind(_ model: AppModel) -> PaneKind? {
        guard let space = model.activeSpace else { return nil }
        return space.root.panes.first { $0.id == space.focus }?.kind
    }

    /// The conversation view the focused pane could switch to, or nil when it has only one.
    static func otherView(_ model: AppModel) -> ConversationView? {
        switch focusedKind(model) {
        case let .terminal(_, agent)? where agent?.isClaude == true && model.agentFound("claude"): .agent
        case let .agent(_, agent)?: agent == "claude" ? .terminal : nil
        default: nil
        }
    }

    private static func hasPane(_ model: AppModel) -> Bool {
        if case .unknown? = focusedKind(model) { return false }
        return focusedPane(model) != nil
    }

    /// The key window when it is a window of its own that is not one of Wheel's (Settings, Usage):
    /// never a sheet, such as the confirmation, or a panel.
    private static var otherWindow: NSWindow? {
        NSApp.keyWindow.flatMap { $0.isWheelWindow || $0.isSheet || $0 is NSPanel ? nil : $0 }
    }

    private static func manyPanes(_ model: AppModel) -> Bool { (model.activeSpace?.root.panes.count ?? 0) > 1 }

    private static func onPane(_ act: @escaping @MainActor (AppModel, String) -> Void) -> @MainActor (AppModel) -> Void {
        { model in focusedPane(model).map { act(model, $0) } }
    }

    private static let fixed: [Entry] = {
        var entries: [Entry] = [
            Entry(id: PaletteAction.openFolderID, title: "Open Folder…", keys: "cmd+o",
                  enabled: { _ in true }, run: { $0.closeCommandPalette(); openSpace($0) }),
            Entry(id: "new-window", title: "New Window", keys: "cmd+n",
                  enabled: { _ in true }, run: { $0.newWindow() }),
            Entry(id: "new-space", title: "New Space…", keys: "cmd+t",
                  enabled: { $0.connection == .connected }, run: { $0.openNewSpacePalette() }),
            Entry(id: "rename-window", title: "Rename Window…",
                  enabled: { $0.layout.focusedWindow != nil },
                  run: { model in model.layout.focusedWindow.map { model.beginRename(window: $0.id) } }),
            // ⌘W in a window that is not Wheel's (Settings, Usage) closes that window, as it does
            // everywhere on macOS.
            Entry(id: "close-pane", title: "Close Pane", keys: "cmd+w",
                  enabled: { focusedPane($0) != nil || otherWindow != nil },
                  run: { model in
                      if let other = otherWindow { return other.performClose(nil) }
                      focusedPane(model).map { model.askClosePane($0) }
                  }),
            Entry(id: "close-space", title: "Close Space", keys: "opt+cmd+w",
                  enabled: { $0.activeSpace != nil }, run: { model in model.active.map { model.askCloseSpace($0) } }),
            Entry(id: "close-window", title: "Close Window", keys: "shift+cmd+w",
                  enabled: { $0.layout.focusedWindow != nil },
                  run: { model in model.layout.focusedWindow.map { model.askCloseWindow($0.id) } }),
            Entry(id: "settings", title: "Settings", keys: "cmd+comma",
                  enabled: { _ in true }, run: { $0.settings.openRequest += 1 }),
            Entry(id: "usage", title: "Usage",
                  enabled: { _ in true }, run: { $0.usage.openRequest += 1 }),
            Entry(id: "command-palette", title: "Command Palette…", keys: "shift+cmd+p",
                  enabled: { _ in true }, run: { $0.openCommandPalette() }),
            Entry(id: "toggle-sidebar", title: "Toggle Sidebar", keys: "ctrl+cmd+s",
                  enabled: { _ in true }, run: { $0.setSidebar(open: !$0.sidebarOpen) }),
            Entry(id: "split-right", title: "Split Right", keys: "cmd+d",
                  enabled: hasPane, run: onPane { $0.split($1, .right) }),
            Entry(id: "split-down", title: "Split Down", keys: "shift+cmd+d",
                  enabled: hasPane, run: onPane { $0.split($1, .down) }),
            Entry(id: "focus-previous", title: "Previous Pane", keys: "cmd+bracket_left",
                  enabled: manyPanes, run: { $0.cycleFocus(by: -1) }),
            Entry(id: "focus-next", title: "Next Pane", keys: "cmd+bracket_right",
                  enabled: manyPanes, run: { $0.cycleFocus(by: 1) }),
        ]
        for (dir, arrow) in [(Dir.left, "arrow_left"), (.right, "arrow_right"), (.up, "arrow_up"), (.down, "arrow_down")] {
            entries.append(Entry(id: "focus-\(dir.rawValue)", title: "Pane \(dir.rawValue.capitalized)", keys: "opt+cmd+\(arrow)",
                                 enabled: manyPanes, run: { $0.moveFocus(toward: dir) }))
        }
        for (dir, arrow) in [(Dir.left, "arrow_left"), (.right, "arrow_right"), (.up, "arrow_up"), (.down, "arrow_down")] {
            entries.append(Entry(id: "resize-\(dir.rawValue)", title: "Move Divider \(dir.rawValue.capitalized)", keys: "ctrl+cmd+\(arrow)",
                                 enabled: manyPanes, run: { $0.resizeFocused(toward: dir) }))
        }
        entries += [
            Entry(id: "equalize-splits", title: "Equalize Panes", keys: "ctrl+cmd+equal",
                  enabled: manyPanes, run: { $0.equalizeSplits() }),
            Entry(id: "zoom-pane", title: "Zoom Pane", keys: "shift+cmd+enter",
                  enabled: { model in
                      guard let space = model.activeSpace else { return false }
                      return model.zoomedPane(in: space) != nil || space.root.panes.count > 1
                  },
                  run: onPane { $0.toggleZoom($1) }),
            Entry(id: "switch-view", title: "Switch Agent Pane Between Chat and Terminal", keys: "shift+cmd+t",
                  enabled: { otherView($0) != nil },
                  run: { model in
                      guard let pane = focusedPane(model), let view = otherView(model) else { return }
                      model.switchView(pane, to: view)
                  }),
            Entry(id: "pane-terminal", title: "Show a Terminal in This Pane",
                  enabled: hasPane, run: onPane { $0.askSetKind($1, .terminal) }),
            Entry(id: "pane-browser", title: "Show a Browser in This Pane",
                  enabled: hasPane, run: onPane { $0.askSetKind($1, .browser) }),
            Entry(id: "pane-artifact", title: "Show an Artifact in This Pane…",
                  enabled: hasPane, run: onPane { $0.openPalette($1) }),
            Entry(id: "previous-space", title: "Previous Space", keys: "shift+cmd+bracket_left",
                  enabled: { $0.windowSpaces.count > 1 }, run: { $0.cycleSpace(by: -1) }),
            Entry(id: "next-space", title: "Next Space", keys: "shift+cmd+bracket_right",
                  enabled: { $0.windowSpaces.count > 1 }, run: { $0.cycleSpace(by: 1) }),
        ]
        for n in 1 ... 8 {
            entries.append(Entry(id: "space-\(n)", title: "Space \(n)", keys: "cmd+\(n)",
                                 enabled: { $0.windowSpaces.count >= n }, run: { $0.showSpace(at: n - 1) }))
        }
        entries.append(Entry(id: "last-space", title: "Last Space", keys: "cmd+9",
                             enabled: { !$0.windowSpaces.isEmpty }, run: { $0.showSpace(at: $0.windowSpaces.count - 1) }))
        return entries
    }()

    /// One "show this agent" action per agent whose program was found, in the picker's order.
    private static func agentEntries(_ model: AppModel) -> [Entry] {
        AgentInfo.pickable(model.agents).map { agent in
            Entry(id: "pane-agent-\(agent.name)", title: "Start \(agent.title) in This Pane",
                  enabled: hasPane, run: onPane { $0.askStartAgent($1, agent.name) })
        }
    }

    /// One "Run" row per `[runner]` command of the active space. Only the palette lists these, so
    /// no key is bound to one: which runners there are changes with the space.
    private static func runnerEntries(_ model: AppModel) -> [Entry] {
        model.runners.map { runner in
            Entry(id: "run-\(runner.name)", title: "Run \(runner.name)",
                  enabled: { $0.connection == .connected && $0.active != nil },
                  run: { $0.startRunner(runner.name) })
        }
    }

    /// Every action there is now.
    static func all(_ model: AppModel) -> [Entry] { fixed + agentEntries(model) }

    static func entry(_ id: String, _ model: AppModel) -> Entry? { (all(model) + runnerEntries(model)).first { $0.id == id } }

    /// Each action with its built-in key: what the `keybind` setting applies over.
    static func defaults(_ model: AppModel) -> [(action: String, chord: KeyChord?)] {
        all(model).map { ($0.id, $0.chord) }
    }

    /// Run action `id` as the palette, a menu or the debug endpoint asks, and log it.
    static func run(_ id: String, _ model: AppModel) {
        guard let entry = entry(id, model), entry.enabled(model) else {
            Log.info("action \(id): not run (unknown or not available now)")
            return
        }
        Log.info("action \(id): run")
        entry.run(model)
    }

    /// The actions the palette lists: those that mean something now.
    static func paletteActions(_ model: AppModel) -> [PaletteAction] {
        (all(model) + runnerEntries(model)).filter { $0.enabled(model) }
            .map { PaletteAction(id: $0.id, title: $0.title, shortcut: model.keymap.chords[$0.id]?.symbol) }
    }
}

extension KeyChord {
    /// The chord as a SwiftUI menu takes it.
    var shortcut: KeyboardShortcut? {
        let equivalent: KeyEquivalent
        switch key {
        case "enter": equivalent = .return
        case "tab": equivalent = .tab
        case "space": equivalent = .space
        case "escape": equivalent = .escape
        case "backspace": equivalent = .delete
        case "delete": equivalent = .deleteForward
        case "arrow_up": equivalent = .upArrow
        case "arrow_down": equivalent = .downArrow
        case "arrow_left": equivalent = .leftArrow
        case "arrow_right": equivalent = .rightArrow
        case "home": equivalent = .home
        case "end": equivalent = .end
        case "page_up": equivalent = .pageUp
        case "page_down": equivalent = .pageDown
        default:
            if key.count > 1, key.hasPrefix("f"), let n = Int(key.dropFirst()), let scalar = UnicodeScalar(NSF1FunctionKey + n - 1) {
                equivalent = KeyEquivalent(Character(scalar))
            } else if let character = KeyChord.character(key) {
                equivalent = KeyEquivalent(character)
            } else {
                return nil
            }
        }
        var mods: EventModifiers = []
        if modifiers.contains(.ctrl) { mods.insert(.control) }
        if modifiers.contains(.opt) { mods.insert(.option) }
        if modifiers.contains(.shift) { mods.insert(.shift) }
        if modifiers.contains(.cmd) { mods.insert(.command) }
        return KeyboardShortcut(equivalent, modifiers: mods)
    }
}

/// The command palette's operations: ⌘⇧P, the palette's own keys and the debug endpoint all call these.
extension AppModel {
    func openCommandPalette(mode: CommandPalette.Mode = .root) {
        guard commandPalette == nil else { return }
        commandPalette = CommandPalette(mode: mode)
        Log.info("command palette: opened in \(mode.rawValue) mode")
        refreshCommandPalette()
        askRecentDirs()
        askRunners()
        if mode == .newSpace { askRoots() }
    }

    /// Ask the daemon for the active space's `[runner]` commands; the answer re-lists the open palette.
    func askRunners() {
        // Another space's runners never stand in for this one's while it answers.
        runners = []
        guard connection == .connected, let space = active else { return }
        let req = client.req()
        runnersReq = req
        client.send(.runnerLs(req: req, space: space))
    }

    /// Run `[runner]` command `name` of the active space in a new terminal pane, as `wheel run` does.
    func startRunner(_ name: String) {
        guard connection == .connected, let space = active else { return }
        let req = client.req()
        Log.info("runner \(name): run in space \(space)")
        sendLayoutOp(.run(req: req, name: name, space: space), req: req)
    }

    /// The `+` buttons: the palette in New Space mode, switching one already open into it.
    func openNewSpacePalette() {
        guard commandPalette != nil else { return openCommandPalette(mode: .newSpace) }
        guard commandPalette?.mode != .newSpace else { return }
        enterNewSpaceMode()
    }

    /// Switch the open palette to New Space mode and ask for the folders it lists.
    func enterNewSpaceMode() {
        commandPalette?.enterNewSpace()
        Log.info("command palette: new space mode")
        askRoots()
    }

    /// Backspace on an empty field: back to the root list.
    func commandPaletteBack() {
        guard commandPalette?.mode == .newSpace else { return }
        commandPalette?.leaveNewSpace()
        Log.info("command palette: root mode")
    }

    /// Tab: complete the selected folder's path.
    func commandPaletteTab() {
        guard var palette = commandPalette, palette.mode == .newSpace else { return }
        let before = palette.browsedParent
        palette.tab()
        commandPalette = palette
        Log.info("command palette: tab -> query of \(palette.query.count) characters")
        if palette.browsedParent != before { askDirList(palette.browsedParent) }
    }

    /// Ask the daemon for the folders New Space mode starts from.
    func askRoots() {
        guard connection == .connected else { return }
        let req = client.req()
        rootsReq = req
        client.send(.dirRoots(req: req))
    }

    /// Ask the daemon for the subfolders of `path`; the answer is kept for the palette.
    func askDirList(_ path: String?) {
        guard let path, connection == .connected else { return }
        let req = client.req()
        dirReqs[req] = path
        client.send(.dirList(req: req, path: path))
    }

    func closeCommandPalette() {
        guard commandPalette != nil else { return }
        commandPalette = nil
        recentReq = nil
        dirReqs = [:]
        rootsReq = nil
        runnersReq = nil
        Log.info("command palette: closed")
    }

    /// Ask the daemon for the recent directories; the answer re-lists the open palette.
    func askRecentDirs() {
        guard connection == .connected else { return }
        let req = client.req()
        recentReq = req
        client.send(.spaceRecent(req: req))
    }

    /// List the palette again from the layout, the recent directories and the actions as they are now.
    func refreshCommandPalette() {
        guard commandPalette != nil else { return }
        commandPalette?.list(
            spaces: spaces.map { .init(id: $0.id, name: $0.name, dir: $0.cwd) },
            recents: recentDirs,
            actions: CommandActions.paletteActions(self)
        )
    }

    func commandPaletteTyped(_ text: String) {
        guard var palette = commandPalette, palette.query != text else { return }
        let before = palette.browsedParent
        palette.type(text)
        commandPalette = palette
        if palette.browsedParent != before { askDirList(palette.browsedParent) }
        Log.info("command palette: query of \(text.count) characters -> \(palette.rows.count) rows, first \(palette.rows.first?.id ?? "-")")
    }

    func commandPaletteMoved(by: Int) {
        commandPalette?.move(by)
    }

    func commandPaletteSelect(_ index: Int) {
        commandPalette?.select(index)
    }

    /// Return: close the palette and run the selected row.
    func commandPaletteRun() {
        guard let item = commandPalette?.selectedItem else { return }
        // These two keep the palette open: one changes its mode, the other only explains.
        if case .newSpaceMode = item { return enterNewSpaceMode() }
        if case .missing = item { return }
        closeCommandPalette()
        Log.info("command palette: run \(item.kind) \(item.id)")
        switch item {
        case let .action(id, _, _): CommandActions.run(id, self)
        case .openFolder: CommandActions.run(PaletteAction.openFolderID, self)
        case let .space(id, _, _): focus(space: id)
        case let .recent(dir): createSpace(cwd: dir)
        case .newSpaceMode: break
        case let .folder(path, space):
            if let space { focus(space: space) } else { createSpace(cwd: path) }
        case .missing: break
        }
    }
}
