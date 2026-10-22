import SwiftUI
import WheelKit

/// Every built-in action by id: its title, its shortcut and what it does. The app's menus, the
/// command palette and the debug endpoint all run an action through its entry here, so an action
/// has one procedure.
@MainActor
enum CommandActions {
    struct Entry {
        let id: String
        let title: String
        let key: KeyEquivalent?
        let modifiers: EventModifiers
        /// Whether the action means anything now (a pane action needs a space with a focused pane).
        let enabled: @MainActor (AppModel) -> Bool
        let run: @MainActor (AppModel) -> Void

        /// The shortcut as a menu prints it, e.g. `⌃⌘S`.
        var shortcutText: String? {
            guard let key else { return nil }
            var out = ""
            if modifiers.contains(.control) { out += "⌃" }
            if modifiers.contains(.option) { out += "⌥" }
            if modifiers.contains(.shift) { out += "⇧" }
            if modifiers.contains(.command) { out += "⌘" }
            return out + key.character.uppercased()
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

    static let all: [Entry] = [
        Entry(id: PaletteAction.openFolderID, title: "Open Folder…", key: "o", modifiers: .command,
              enabled: { _ in true }, run: { $0.closeCommandPalette(); openSpace($0) }),
        Entry(id: "new-window", title: "New Window", key: "n", modifiers: .command,
              enabled: { _ in true }, run: { $0.newWindow() }),
        Entry(id: "rename-window", title: "Rename Window…", key: nil, modifiers: [],
              enabled: { $0.layout.focusedWindow != nil },
              run: { model in model.layout.focusedWindow.map { model.beginRename(window: $0.id) } }),
        Entry(id: "settings", title: "Settings", key: ",", modifiers: .command,
              enabled: { _ in true }, run: { $0.settings.openRequest += 1 }),
        Entry(id: "usage", title: "Usage", key: nil, modifiers: [],
              enabled: { _ in true }, run: { $0.usage.openRequest += 1 }),
        Entry(id: "toggle-sidebar", title: "Toggle Sidebar", key: "s", modifiers: [.command, .control],
              enabled: { _ in true }, run: { $0.setSidebar(open: !$0.sidebarOpen) }),
        Entry(id: "split-right", title: "Split Right", key: "d", modifiers: .command,
              enabled: hasPane, run: { model in focusedPane(model).map { model.split($0, .right) } }),
        Entry(id: "split-down", title: "Split Down", key: "d", modifiers: [.command, .shift],
              enabled: hasPane, run: { model in focusedPane(model).map { model.split($0, .down) } }),
        Entry(id: "close-pane", title: "Close Pane", key: "w", modifiers: [.command, .control],
              enabled: { focusedPane($0) != nil }, run: { model in focusedPane(model).map { model.closePane($0) } }),
        Entry(id: "switch-view", title: "Switch Agent Pane Between Chat and Terminal", key: "t", modifiers: [.command, .shift],
              enabled: { otherView($0) != nil },
              run: { model in
                  guard let pane = focusedPane(model), let view = otherView(model) else { return }
                  model.switchView(pane, to: view)
              }),
    ]

    static func entry(_ id: String) -> Entry? { all.first { $0.id == id } }

    /// Run action `id` as the palette, a menu or the debug endpoint asks, and log it.
    static func run(_ id: String, _ model: AppModel) {
        guard let entry = entry(id), entry.enabled(model) else {
            Log.info("action \(id): not run (unknown or not available now)")
            return
        }
        Log.info("action \(id): run")
        entry.run(model)
    }

    /// The actions the palette lists: those that mean something now.
    static func paletteActions(_ model: AppModel) -> [PaletteAction] {
        all.filter { $0.enabled(model) }.map { PaletteAction(id: $0.id, title: $0.title, shortcut: $0.shortcutText) }
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
        if mode == .newSpace { askRoots() }
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
        Log.info("command palette: tab -> query \"\(palette.query)\"")
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
