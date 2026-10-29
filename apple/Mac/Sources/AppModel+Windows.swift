import AppKit
import Foundation
import SwiftUI
import WheelKit

/// What opens and closes the app's windows. The model holds which window holds which space
/// (`layout`); the host makes an `NSWindow` for each record and says when a person closed one.
@MainActor
protocol WindowHost: AnyObject {
    /// Make, retitle and close windows until they match the model's records.
    func sync()
    /// Order a window front and make it key.
    func bringFront(_ window: String)
    /// The `NSWindow` showing a record, if it is open.
    func window(_ id: String) -> NSWindow?
    /// Order a little window front.
    func bringFrontLittle(_ id: String)
    /// Move a little window so it is centred on `card`, a rectangle in screen coordinates.
    func moveLittle(_ id: String, to card: CGRect)
    /// The `NSWindow` showing a little window's record, if it is open.
    func littleWindow(_ id: String) -> NSWindow?
}

/// Where the app keeps its own preferences. `WHEEL_PREFS_SUITE` names another domain, so a check
/// run never touches the person's windows.
enum Prefs {
    nonisolated(unsafe) static let store: UserDefaults = ProcessInfo.processInfo.environment["WHEEL_PREFS_SUITE"].flatMap { UserDefaults(suiteName: $0) } ?? .standard
}

extension WindowLayout {
    private static let key = "windowLayout"

    /// The layout saved by the last run; empty on a first launch.
    static func saved() -> WindowLayout {
        guard let data = Prefs.store.data(forKey: key),
              let layout = try? JSONDecoder().decode(WindowLayout.self, from: data) else { return WindowLayout() }
        return layout
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) { Prefs.store.set(data, forKey: Self.key) }
    }
}

extension AppModel {
    /// "Restore windows at launch": reopen the windows as they were. Off, launch opens one window
    /// holding every space.
    var restoresWindows: Bool {
        get {
            _ = restoreWindowsChanges
            return Prefs.store.object(forKey: "restoreWindows") as? Bool ?? true
        }
        set {
            Prefs.store.set(newValue, forKey: "restoreWindows")
            restoreWindowsChanges += 1
            Log.info("restore windows at launch: \(newValue)")
        }
    }

    /// Every space some window shows; their terminals draw.
    var shownSpaces: Set<String> { Set(layout.windows.compactMap(\.active)).union(layout.littles.map(\.space)) }

    /// The spaces in no window, in the order they went there.
    var backgroundSpaces: [SpaceInfo] { layout.background.compactMap { id in spaces.first { $0.id == id } } }

    func spaces(in window: String) -> [SpaceInfo] {
        (layout.window(window)?.spaces ?? []).compactMap { id in spaces.first { $0.id == id } }
    }

    func active(in window: String) -> String? { layout.window(window)?.active }

    func sidebarOpen(in window: String) -> Bool { layout.window(window)?.sidebar ?? true }

    func spaceName(_ id: String) -> String? { spaces.first { $0.id == id }?.name }

    func title(of window: WindowRecord) -> String { layout.title(of: window) { spaceName($0) } }

    /// The id and name of a window or a space, as a CLI argument names it: its id, else its name.
    func windowID(named text: String) -> Result<String, WindowFailure> {
        if layout.window(text) != nil { return .success(text) }
        let named = layout.windows.filter { title(of: $0) == text }
        switch named.count {
        case 1: return .success(named[0].id)
        case 0: return .failure(.init("no window \(text); the windows are \(layout.windows.map { "\(title(of: $0)) (\($0.id))" }.joined(separator: ", "))"))
        default: return .failure(.init("\(named.count) windows are named \(text); use one of their ids: \(named.map(\.id).joined(separator: ", "))"))
        }
    }

    func spaceID(named text: String) -> Result<String, WindowFailure> {
        if spaces.contains(where: { $0.id == text }) { return .success(text) }
        let named = spaces.filter { $0.name == text }
        switch named.count {
        case 1: return .success(named[0].id)
        case 0: return .failure(.init("no space \(text); the spaces are \(spaces.map { "\($0.name) (\($0.id))" }.joined(separator: ", "))"))
        default: return .failure(.init("\(named.count) spaces are named \(text); use one of their ids: \(named.map(\.id).joined(separator: ", "))"))
        }
    }

    // MARK: Reconciling with the daemon

    /// Match the windows to the spaces wheeld reports, and follow it when something else (a CLI
    /// command, another client) moved its active space.
    func reconcileWindows(_ spaces: [SpaceInfo], daemonActive active: String?) {
        let ids = spaces.map(\.id)
        let firstReport = !reconciled
        reconciled = true
        let restores = restoresWindows
        let last = lastDaemonActive
        // A space wheeld made to run with no window (`wheel space new --background`) stays in none,
        // including one made while this app was quit. Any other new space joins the focused window
        // as a tab; only a focus request, which makes it wheeld's active space, shows it.
        let unshown = Set(spaces.filter(\.background).map(\.id))
        var littleToFront: String?
        var littleAfter: (String, CGRect?)?
        editWindows(commit: { _, changed in changed }) { layout in
            if firstReport {
                // With nothing saved (a first launch) every space goes to one window.
                let saved = layout.windows.contains { !$0.spaces.isEmpty } || !layout.background.isEmpty
                if restores { layout.reconcile(ids, background: saved ? unshown : []) } else { layout.collapse(ids) }
                Log.info("windows \(restores ? "restored" : "collapsed into one"): \(describe(layout))")
            } else {
                let fresh = layout.reconcile(ids, background: unshown)
                // The space a `pane_move` made goes where the drop or the menu said.
                if let pending = pendingNewSpace, Date().timeIntervalSince(pending.at) >= 10 {
                    pendingNewSpace = nil
                } else if let pending = pendingNewSpace,
                          let space = fresh.first(where: { id in spaces.first { $0.id == id }?.root.panes.contains { $0.id == pending.pane } == true }) {
                    pendingNewSpace = nil
                    do {
                        layout.place(space, in: pending.window, at: pending.position)
                        if pending.little { littleAfter = (space, pending.card) }
                    }
                }
                for space in fresh {
                    let window = layout.window(holding: space)
                    Log.info("new space \(space) opened in \(window.map { "window \($0.id)" } ?? "the background")")
                }
            }
            if let active, active != last {
                if let holder = layout.window(holding: active) {
                    if holder.active != active { layout.show(active, in: holder.id) }
                }
                littleToFront = layout.little(showing: active)?.id
            }
        }
        if let littleToFront { windowHost?.bringFrontLittle(littleToFront) }
        lastDaemonActive = active
        if let (space, card) = littleAfter { openLittle(for: space, from: card) }
    }

    /// `wheel space focus` (or `pane focus`) on a space that runs with no window opens it in a little
    /// window that belongs to none, and changes no other window. wheeld says so with `space_focused`
    /// after every such request, so a space that was already active opens one too.
    func spaceFocusRequested(_ space: String) {
        var littleToFront: String?
        editWindows(commit: { _, changed in changed }) { layout in
            guard layout.background.contains(space) else { return }
            layout.openLittle(space)
            Log.info("space \(space) focused by a request; opened in a little window")
        }
        littleToFront = layout.little(showing: space)?.id
        if let littleToFront { windowHost?.bringFrontLittle(littleToFront) }
    }

    private func describe(_ layout: WindowLayout) -> String {
        let windows = layout.windows.map { "\($0.id.prefix(8)):\($0.spaces.joined(separator: ","))" }.joined(separator: " ")
        return "\(windows) background=\(layout.background.joined(separator: ","))"
    }

    /// At launch, before wheeld has said anything: open the windows as they were saved. A first
    /// launch, or one after every window was closed, opens one (empty, or holding the background
    /// spaces), which wheeld's first report fills. With "Restore windows at launch" off, one
    /// window holds every space.
    func openLaunchWindows() {
        let restores = restoresWindows
        editWindows { layout in
            if !restores {
                layout.collapse(layout.windows.flatMap(\.spaces) + layout.background)
            } else if layout.windows.isEmpty {
                layout.newWindow(spaces: layout.background)
            }
        }
    }

    /// The Dock icon with no window open: one window holding the spaces that run in the background.
    func openWindowForBackground() {
        newWindow(spaces: layout.background)
    }

    // MARK: Changing the windows — the menus, the popover, drags, the debug endpoint and `wheel window` all call these.

    /// Save the layout and bring the open windows and the drawn surfaces in line with it.
    func commitWindows() {
        layout.save()
        // A sheet asked in a window that has since gone (its last space moved out) has nobody to answer it.
        if let closing = closingWindow, layout.window(closing) == nil { closingWindow = nil }
        // Measurements of windows that are gone would aim a drop at them.
        let open = Set(layout.windows.map(\.id))
        rowFrames = rowFrames.filter { open.contains($0.key) }
        sidebarFrames = sidebarFrames.filter { open.contains($0.key) }
        tabStripFrames = tabStripFrames.filter { open.contains($0.key) }
        cardFrames = cardFrames.filter { open.contains($0.key) || $0.key == MenuBarWindow.backgroundKey }
        windowHost?.sync()
        updateVisibility()
        Log.info("windows: \(describe(layout)) focused=\(layout.focused?.prefix(8) ?? "none")")
        if let pending = pendingSwitch {
            pendingSwitch = nil
            let ms = Int(((ProcessInfo.processInfo.systemUptime - pending.pressAt) * 1000).rounded())
            Log.info("[tab] switch \(pending.space.prefix(8)): mouse-down to windows: \(ms) ms")
        }
    }

    /// A mouse-down on a space tab or sidebar row; the click it belongs to switches the space.
    func notePress(space: String) {
        guard let event = NSApp.currentEvent, event.type == .leftMouseDown else { return }
        spacePressAt = event.timestamp
    }

    /// A click on a space tab or sidebar row: show the space, timing it from the mouse-down.
    func tapSpace(_ space: String, in window: String) {
        let now = ProcessInfo.processInfo.systemUptime
        // A press that was no click (a drag, the close button) is older than the click it is not part of.
        let pressAt = spacePressAt.flatMap { now - $0 < 1 ? $0 : nil } ?? now
        spacePressAt = nil
        pendingSwitch = (space, pressAt)
        show(space: space, in: window)
        pendingSwitch = nil
    }

    /// A person closed a window with its close button or ⌘W: its spaces go to the background. At
    /// quit the windows close too, and the layout stays as it was.
    func windowClosedByPerson(_ id: String) {
        guard !quitting, layout.window(id) != nil else { return }
        editWindows { $0.close(window: id) }
    }

    /// A window became the key window: its space is the one CLI commands act on.
    func windowBecameFocused(_ id: String) {
        guard layout.window(id) != nil else { return }
        if layout.focused != id { editLayoutSilently { $0.focused = id } }
        if let space = layout.window(id)?.active, space != daemonActive { daemonFocus(space: space) }
    }

    @discardableResult
    func newWindow(name: String? = nil, spaces: [String] = []) -> String {
        let sidebar = layout.focusedWindow?.sidebar ?? true
        let id = editWindows { $0.newWindow(name: name, spaces: spaces, sidebar: sidebar) }
        windowHost?.bringFront(id)
        if let first = spaces.first { daemonFocus(space: first) }
        return id
    }

    /// What the person chose in a main window's close sheet.
    enum WindowCloseAnswer { case background, stop, cancel }

    /// The red light, ⇧⌘W and `wheel window close`: a window holding spaces asks what to do with
    /// them in a sheet on that window; one with none closes.
    func askCloseWindow(_ id: String) {
        guard let record = layout.window(id) else { return }
        guard !record.spaces.isEmpty else { return closeWindow(id) }
        closingWindow = id
        windowHost?.bringFront(id)
        Log.info("close window \(id.prefix(8)) asked: \(closeSummary(of: record))")
    }

    /// "It holds 3 spaces with 6 panes. 1 of them is in a little window."
    func closeSummary(of window: WindowRecord) -> String {
        let held = spaces(in: window.id)
        let panes = held.reduce(0) { $0 + $1.root.panes.count }
        let little = window.spaces.filter { layout.little(showing: $0) != nil }.count
        var text = "It holds \(held.count) \(held.count == 1 ? "space" : "spaces") with \(panes) \(panes == 1 ? "pane" : "panes")."
        if little > 0 { text += " \(little) of them \(little == 1 ? "is" : "are") in a little window." }
        return text
    }

    func answerWindowClose(_ answer: WindowCloseAnswer) {
        guard let id = closingWindow else { return }
        closingWindow = nil
        Log.info("close window \(id.prefix(8)) answered: \(answer)")
        switch answer {
        case .background: closeWindow(id)
        case .stop: closeWindow(id, stopping: true)
        case .cancel: break
        }
    }

    /// Close window `id` with no question: its spaces go to the background, or with `stopping`
    /// every process in them stops and the spaces close.
    func closeWindow(_ id: String, stopping: Bool = false) {
        guard layout.window(id) != nil else { return }
        if closingWindow == id { closingWindow = nil }
        let stopped = editWindows { $0.close(window: id, stopping: stopping) }
        for space in stopped { closeSpace(space) }
    }

    /// Move `space` into `window` (nil: the background) at `position`.
    @discardableResult
    func place(_ space: String, in window: String?, at position: Int? = nil) -> Bool {
        guard spaces.contains(where: { $0.id == space }) else { return false }
        guard editWindows(commit: { placed, _ in placed }, { $0.place(space, in: window, at: position) }) else { return false }
        if let window, let shown = layout.window(window)?.active, shown == space { daemonFocus(space: space) }
        return true
    }

    @discardableResult
    func tearOff(_ space: String) -> String? {
        guard spaces.contains(where: { $0.id == space }) else { return nil }
        let id = editWindows { $0.tearOff(space) }
        windowHost?.bringFront(id)
        daemonFocus(space: space)
        return id
    }

    /// Open `space` in a little window that belongs to the main window holding it (one that belongs
    /// to none for a background space), or bring its open one front. `card` is where the drag's
    /// preview card was: the window grows from it. Returns nil when the layout places the space nowhere.
    @discardableResult
    func openLittle(for space: String, from card: CGRect? = nil) -> String? {
        guard spaces.contains(where: { $0.id == space }) else { return nil }
        if let open = layout.little(showing: space) {
            // A space already in a little window moves that window to the drop instead of opening a second.
            if let card { windowHost?.moveLittle(open.id, to: card) }
            goToLittle(open.id)
            return open.id
        }
        littleOrigin = card
        guard let id = editWindows({ $0.openLittle(space) }) else {
            littleOrigin = nil
            Log.info("little window for \(space): the layout places it nowhere")
            return nil
        }
        littleOrigin = nil
        windowHost?.bringFrontLittle(id)
        daemonFocus(space: space)
        return id
    }

    func goToLittle(_ id: String) {
        guard layout.little(id) != nil else { return }
        windowHost?.bringFrontLittle(id)
        if let space = layout.little(id)?.space { daemonFocus(space: space) }
    }

    /// Close a little window; its owner's tab shows the grid again.
    func closeLittle(_ id: String) {
        guard layout.little(id) != nil else { return }
        editWindows { $0.closeLittle(id) }
    }

    /// Bring `space` back into its tab: close the little window that shows it.
    func bringBack(space: String) {
        guard let little = layout.little(showing: space) else { return }
        closeLittle(little.id)
    }

    /// A person closed a little window with its red light: the space returns to its tab.
    func littleClosedByPerson(_ id: String) {
        guard !quitting else { return }
        closeLittle(id)
    }

    /// Show `space` in window `window`: a click on its row or tab. A space from another window or
    /// the background moves here first.
    func show(space: String, in window: String) {
        guard spaces.contains(where: { $0.id == space }), layout.window(window) != nil else { return }
        let held = layout.window(window)?.spaces.contains(space) == true
        editWindows {
            if held { $0.show(space, in: window) } else { $0.place(space, in: window) }
            $0.focused = window
        }
        daemonFocus(space: space)
    }

    /// Show `space` in the window that holds it, putting it in the focused window first when it
    /// runs in the background, and bring that window front.
    func focus(space: String) {
        guard spaces.contains(where: { $0.id == space }) else { return }
        if let little = layout.little(showing: space) {
            goToLittle(little.id)
            return
        }
        let window = editWindows { layout -> String in
            let window: String
            if let holder = layout.window(holding: space) {
                window = holder.id
                layout.show(space, in: window)
            } else if let target = layout.focusedWindow?.id {
                window = target
                layout.place(space, in: target)
            } else {
                window = layout.newWindow(spaces: [space])
            }
            layout.focused = window
            return window
        }
        windowHost?.bringFront(window)
        daemonFocus(space: space)
    }

    func renameWindow(_ id: String, to name: String?) {
        guard layout.window(id) != nil else { return }
        editWindows { $0.rename(window: id, to: name) }
    }

    /// Pin a main window above every unpinned window of any app, or let it stack normally again.
    func setPinned(_ pinned: Bool, window id: String) {
        guard let window = layout.window(id), window.pinned != pinned else { return }
        editWindows { $0.setPinned(pinned, window: id) }
        Log.info("window \(id.prefix(8)) \(pinned ? "pinned" : "unpinned")")
    }

    /// The same for a little window.
    func setPinned(_ pinned: Bool, little id: String) {
        guard let little = layout.little(id), little.pinned != pinned else { return }
        editWindows { $0.setPinned(pinned, little: id) }
        Log.info("little window \(id.prefix(8)) \(pinned ? "pinned" : "unpinned")")
    }

    func setSidebar(open: Bool, in window: String? = nil) {
        guard let id = window ?? layout.focusedWindow?.id, layout.window(id)?.sidebar != open else { return }
        withAnimation { editLayoutSilently { $0.setSidebar(open, in: id) } }
        Log.info("sidebar of window \(id.prefix(8)) \(open ? "opened" : "closed")")
    }

    /// Redraw only the surfaces: which terminals are on screen changed, the panes did not.
    func updateVisibility() {
        let plan = SurfacePlan(spaces, active: active, shown: shownSpaces, existing: Set(terminals.keys), focusKey: focusKey)
        for (term, visible) in plan.visible { terminals[term]?.view.isSurfaceVisible = visible }
        updatePageVisibility()
    }

    /// A web or browser pane's page in a space no window shows stays mounted (no reload) but is
    /// hidden: a hidden NSView keeps no cursor rects or tracking areas, so its page cannot set the
    /// cursor over the pane on screen above it. WebKit also treats a hidden page as not visible
    /// (`document.visibilityState`, animation frames), as it does a page in a hidden tab. A pane
    /// that already left the layout keeps its state while it fades out.
    func updatePageVisibility() {
        let shown = shownSpaces
        var space: [String: String] = [:]
        for s in spaces { for pane in s.root.panes { space[pane.id] = s.id } }
        func set(_ view: NSView, _ pane: String) {
            guard let id = space[pane] else { return }
            let hidden = !shown.contains(id)
            if view.isHidden != hidden { view.isHidden = hidden }
        }
        for (pane, web) in webPanes { set(web.view, pane) }
        for (pane, browser) in browserPanes { set(browser.view, pane) }
    }
}

/// Why a window or space command was refused, in the words the CLI prints.
struct WindowFailure: Error, Equatable {
    let message: String
    init(_ message: String) { self.message = message }
}

// MARK: Renaming — a space's name is wheeld's, a window's is this app's.

extension AppModel {
    /// Name a space: the daemon keeps it and tells every client, the iPhone included. Returns why
    /// the name is refused before it is sent; the daemon's own refusal arrives later in
    /// `spaceRenameError`.
    @discardableResult
    func renameSpace(_ id: String, to text: String) -> String? {
        let name = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else {
            spaceRenameError[id] = "A space needs a name."
            return spaceRenameError[id]
        }
        guard connection == .connected else {
            spaceRenameError[id] = "Not connected to wheeld."
            return spaceRenameError[id]
        }
        if spaces.first(where: { $0.id == id })?.name == name {
            spaceRenameError[id] = nil
            if renamingSpace == id { renamingSpace = nil }
            return nil
        }
        let req = client.req()
        renameReqs[req] = id
        spaceRenameError[id] = nil
        Log.info("rename space \(id) to \(name)")
        client.send(.spaceRename(req: req, space: id, name: name))
        return nil
    }

    func beginRename(space id: String) {
        spaceRenameError[id] = nil
        renamingWindow = nil
        renamingSpace = id
    }

    func beginRename(window id: String) {
        renamingSpace = nil
        renamingWindow = id
    }

    func endRenaming() {
        if let space = renamingSpace { spaceRenameError[space] = nil }
        renamingSpace = nil
        renamingWindow = nil
    }

    /// Return in a window's title field: the name, or none when it is empty.
    func commitRename(window id: String, to text: String) {
        renameWindow(id, to: text)
        if renamingWindow == id { renamingWindow = nil }
    }
}

// MARK: `wheel window` — the daemon forwards each command here and returns the answer.

extension AppModel {
    func answerWindowCommand(id: UInt64, _ command: WindowCommand) {
        let result = windowCommand(command)
        switch result {
        case let .success(data):
            let json = (try? JSONSerialization.data(withJSONObject: data, options: [.sortedKeys, .fragmentsAllowed])).map { String(decoding: $0, as: UTF8.self) }
            Log.info("window command \(command) answered: \(json ?? "?")")
            client.send(.windowAnswer(id: id, data: json ?? "null", error: nil))
        case let .failure(failure):
            Log.info("window command \(command) refused: \(failure.message)")
            client.send(.windowAnswer(id: id, data: nil, error: failure.message))
        }
    }

    func windowCommand(_ command: WindowCommand) -> Result<[String: Any], WindowFailure> {
        switch command {
        case .ls:
            let named: (SpaceInfo) -> [String: Any] = { ["id": $0.id, "name": $0.name] }
            return .success([
                "windows": layout.windows.map { window -> [String: Any] in
                    [
                        "id": window.id,
                        "name": window.name ?? NSNull(),
                        "title": title(of: window),
                        "spaces": spaces(in: window.id).map(named),
                        "active": window.active ?? NSNull(),
                        "sidebar": window.sidebar,
                        "focused": window.id == layout.focusedWindow?.id,
                    ]
                },
                "background": backgroundSpaces.map(named),
                "little": layout.littles.map { little -> [String: Any] in
                    ["id": little.id, "space": spaceName(little.space).map { ["id": little.space, "name": $0] } ?? ["id": little.space],
                     "owner": little.owner ?? NSNull()]
                },
            ])
        case let .new(name, space):
            var held: [String] = []
            if let space {
                switch spaceID(named: space) {
                case let .success(id): held = [id]
                case let .failure(failure): return .failure(failure)
                }
            }
            return .success(["window": newWindow(name: name, spaces: held)])
        case let .rename(window, name):
            switch windowID(named: window) {
            case let .success(id):
                renameWindow(id, to: name)
                return .success(["window": id])
            case let .failure(failure): return .failure(failure)
            }
        case let .close(window):
            switch windowID(named: window) {
            case let .success(id):
                askCloseWindow(id)
                return .success(["window": id])
            case let .failure(failure): return .failure(failure)
            }
        case let .spaceMove(space, to):
            let spaceID: String
            switch self.spaceID(named: space) {
            case let .success(id): spaceID = id
            case let .failure(failure): return .failure(failure)
            }
            if to == "background" {
                place(spaceID, in: nil)
                return .success(["space": spaceID, "to": "background"])
            }
            switch windowID(named: to) {
            case let .success(id):
                place(spaceID, in: id)
                return .success(["space": spaceID, "to": id])
            case let .failure(failure): return .failure(failure)
            }
        }
    }
}
