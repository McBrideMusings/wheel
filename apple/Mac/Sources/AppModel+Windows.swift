import AppKit
import Foundation
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
        get { Prefs.store.object(forKey: "restoreWindows") as? Bool ?? true }
        set {
            Prefs.store.set(newValue, forKey: "restoreWindows")
            objectWillChange.send()
            Log.info("restore windows at launch: \(newValue)")
        }
    }

    /// Every space some window shows; their terminals draw.
    var shownSpaces: Set<String> { Set(layout.windows.compactMap(\.active)) }

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
        let before = layout
        if !reconciled {
            reconciled = true
            if restoresWindows { layout.reconcile(ids) } else { layout.collapse(ids) }
            Log.info("windows \(restoresWindows ? "restored" : "collapsed into one"): \(describe(layout))")
        } else {
            let fresh = layout.reconcile(ids)
            if !fresh.isEmpty { Log.info("new spaces \(fresh) opened in \(layout.focusedWindow.map { "window \($0.id)" } ?? "the background")") }
        }
        if let active, active != lastDaemonActive, let holder = layout.window(holding: active), holder.active != active {
            layout.show(active, in: holder.id)
        }
        lastDaemonActive = active
        if layout != before { commitWindows() }
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
        if !restoresWindows {
            layout.collapse(layout.windows.flatMap(\.spaces) + layout.background)
        } else if layout.windows.isEmpty {
            layout.newWindow(spaces: layout.background)
        }
        commitWindows()
    }

    /// The Dock icon with no window open: one window holding the spaces that run in the background.
    func openWindowForBackground() {
        newWindow(spaces: layout.background)
    }

    // MARK: Changing the windows — the menus, the popover, drags, the debug endpoint and `wheel window` all call these.

    /// Save the layout and bring the open windows and the drawn surfaces in line with it.
    func commitWindows() {
        layout.save()
        windowHost?.sync()
        updateVisibility()
        Log.info("windows: \(describe(layout)) focused=\(layout.focused?.prefix(8) ?? "none")")
    }

    /// A person closed a window with its close button or ⌘W: its spaces go to the background. At
    /// quit the windows close too, and the layout stays as it was.
    func windowClosedByPerson(_ id: String) {
        guard !quitting, layout.window(id) != nil else { return }
        layout.close(window: id)
        commitWindows()
    }

    /// A window became the key window: its space is the one CLI commands act on.
    func windowBecameFocused(_ id: String) {
        guard layout.window(id) != nil else { return }
        if layout.focused != id {
            layout.focused = id
            layout.save()
            objectWillChange.send()
        }
        if let space = layout.window(id)?.active, space != daemonActive { daemonFocus(space: space) }
    }

    @discardableResult
    func newWindow(name: String? = nil, spaces: [String] = []) -> String {
        let id = layout.newWindow(name: name, spaces: spaces, sidebar: layout.focusedWindow?.sidebar ?? true)
        commitWindows()
        windowHost?.bringFront(id)
        if let first = spaces.first { daemonFocus(space: first) }
        return id
    }

    func closeWindow(_ id: String) {
        guard layout.window(id) != nil else { return }
        layout.close(window: id)
        commitWindows()
    }

    /// Move `space` into `window` (nil: the background) at `position`.
    @discardableResult
    func place(_ space: String, in window: String?, at position: Int? = nil) -> Bool {
        guard spaces.contains(where: { $0.id == space }), layout.place(space, in: window, at: position) else { return false }
        commitWindows()
        if let window, let shown = layout.window(window)?.active, shown == space { daemonFocus(space: space) }
        return true
    }

    @discardableResult
    func tearOff(_ space: String) -> String? {
        guard spaces.contains(where: { $0.id == space }) else { return nil }
        let id = layout.tearOff(space)
        commitWindows()
        windowHost?.bringFront(id)
        daemonFocus(space: space)
        return id
    }

    /// Show `space` in window `window`: a click on its row or tab. A space from another window or
    /// the background moves here first.
    func show(space: String, in window: String) {
        guard spaces.contains(where: { $0.id == space }), layout.window(window) != nil else { return }
        if layout.window(window)?.spaces.contains(space) == true {
            layout.show(space, in: window)
        } else {
            layout.place(space, in: window)
        }
        layout.focused = window
        commitWindows()
        daemonFocus(space: space)
    }

    /// Show `space` in the window that holds it, putting it in the focused window first when it
    /// runs in the background, and bring that window front.
    func focus(space: String) {
        guard spaces.contains(where: { $0.id == space }) else { return }
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
        commitWindows()
        windowHost?.bringFront(window)
        daemonFocus(space: space)
    }

    func renameWindow(_ id: String, to name: String?) {
        guard layout.window(id) != nil else { return }
        layout.rename(window: id, to: name)
        commitWindows()
    }

    func setSidebar(open: Bool, in window: String? = nil) {
        guard let id = window ?? layout.focusedWindow?.id, layout.window(id)?.sidebar != open else { return }
        layout.setSidebar(open, in: id)
        layout.save()
        objectWillChange.send()
        Log.info("sidebar of window \(id.prefix(8)) \(open ? "opened" : "closed")")
    }

    /// Redraw only the surfaces: which terminals are on screen changed, the panes did not.
    func updateVisibility() {
        let plan = SurfacePlan(spaces, active: active, shown: shownSpaces, existing: Set(terminals.keys), focusKey: focusKey)
        for (term, visible) in plan.visible { terminals[term]?.view.isSurfaceVisible = visible }
    }
}

/// Why a window or space command was refused, in the words the CLI prints.
struct WindowFailure: Error, Equatable {
    let message: String
    init(_ message: String) { self.message = message }
}
