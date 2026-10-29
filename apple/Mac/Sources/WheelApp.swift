import Combine
import SwiftUI
import WheelKit
import WheelUI

@main
struct WheelApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        // The coding agents' usage, opened by the strip's dial.
        Window("Usage", id: "usage") {
            UsageWindow(model: delegate.model, usage: delegate.model.usage)
        }
        .windowResizability(.contentSize)
        // SwiftUI opens the first `Window` scene at launch; Wheel's own windows are AppKit's.
        .defaultLaunchBehavior(.suppressed)
        // Wheel's own settings, which live in the daemon: the app menu's Settings… (⌘,).
        Settings {
            SettingsView(model: delegate.model, settings: delegate.model.settings)
        }
        .commands { WheelCommands(model: delegate.model) }
        // The app's own item in the menu bar, present while the app runs, with or without a window.
        MenuBarExtra {
            MenuBarPopover(model: delegate.model)
        } label: {
            MenuBarLabel(model: delegate.model)
        }
        .menuBarExtraStyle(.window)
    }
}

/// Owns the model and the windows. The connection to wheeld starts here, at launch, so it does
/// not depend on a window being open (closing the last window leaves the app running).
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = AppModel()
    private var windows: WindowManager?
    private var lastBackgroundBadge: Int?
    #if DEBUG
    private var debug: DebugServer?
    #endif

    override init() {
        // Wheel saves and reopens its own windows (ADR-0014), so AppKit has nothing to restore.
        UserDefaults.standard.register(defaults: ["ApplePersistenceIgnoreState": true])
        UILog.sink = { Log.info($0) }
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // WHEEL_APPEARANCE=light|dark pins the appearance, so both can be checked on one machine.
        switch ProcessInfo.processInfo.environment["WHEEL_APPEARANCE"] {
        case "light": NSApp.appearance = NSAppearance(named: .aqua)
        case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
        default: break
        }
        let manager = WindowManager(model: model)
        windows = manager
        model.windowHost = manager
        #if DEBUG
        let server = DebugServer(model: model)
        server.start()
        debug = server
        #endif
        model.start()
        model.openLaunchWindows()
        // The Dock icon counts what the menu bar item counts: approvals waiting where no window looks.
        watchBadge()
    }

    /// Update the badges, and again whenever something they read changes.
    private func watchBadge() {
        withObservationTracking { updateBadge() } onChange: { [weak self] in
            DispatchQueue.main.async { self?.watchBadge() }
        }
    }

    private func updateBadge() {
        let background = model.backgroundRows.count
        if background != lastBackgroundBadge {
            lastBackgroundBadge = background
            Log.info("menu bar badge: \(background == 0 ? "none" : "\(background)")")
        }
        let count = model.unseenApprovals
        let label = count > 0 ? "\(count)" : nil
        if NSApp.dockTile.badgeLabel != label {
            NSApp.dockTile.badgeLabel = label
            Log.info("dock badge: \(label ?? "none")")
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    /// The Dock icon: with no window open, open one holding the background spaces.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { model.openWindowForBackground() }
        return true
    }

    /// Windows close as the app quits; they must not send their spaces to the background.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        model.quitting = true
        Log.info("app quitting")
        return .terminateNow
    }
}

/// The menus: each item runs its entry in `CommandActions`, as the command palette does, and shows
/// the key the keymap gives it. In a Wheel window the keymap's monitor runs a bound key before the
/// menus see it (SwiftUI brings these items up to date only when a menu opens); in another window,
/// such as Settings, the menus' key equivalents do.
struct WheelCommands: Commands {
    var model: AppModel

    private func item(_ id: String, as title: String? = nil) -> some View {
        let entry = CommandActions.entry(id, model)
        return Button(title ?? entry?.title ?? id) { CommandActions.run(id, model) }
            .keyboardShortcut(model.keymap.chords[id]?.shortcut)
            .disabled(!(entry?.enabled(model) ?? false))
    }

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            item("new-window")
            item("new-space")
            item(PaletteAction.openFolderID)
            Divider()
            item("rename-window")
        }
        CommandGroup(replacing: .saveItem) {
            item("close-pane")
            item("close-space")
            item("close-window")
        }
        CommandGroup(replacing: .appSettings) { item("settings", as: "Settings…") }
        CommandGroup(after: .sidebar) {
            item("toggle-sidebar")
            item("command-palette")
            item("shortcuts")
        }
        CommandGroup(after: .windowList) { item("usage", as: "Usage") }
        CommandMenu("Pane") {
            item("split-right")
            item("split-down")
            Divider()
            item("focus-previous")
            item("focus-next")
            ForEach(["left", "right", "up", "down"], id: \.self) { item("focus-\($0)") }
            Divider()
            ForEach(["left", "right", "up", "down"], id: \.self) { item("resize-\($0)") }
            item("equalize-splits")
            item("zoom-pane")
            Divider()
            item("switch-view")
            Menu("Show in This Pane") {
                item("pane-terminal", as: "Terminal")
                ForEach(AgentInfo.pickable(model.agents), id: \.name) { item("pane-agent-\($0.name)", as: $0.title) }
                item("pane-artifact", as: "Artifact…")
                item("pane-browser", as: "Browser")
            }
        }
        CommandMenu("Space") {
            item("previous-space")
            item("next-space")
            Divider()
            ForEach(1 ... 8, id: \.self) { n in
                item("space-\(n)", as: spaceTitle(n - 1) ?? "Space \(n)")
            }
            item("last-space", as: model.windowSpaces.isEmpty ? "Last Space" : "Last Space: \(spaceTitle(model.windowSpaces.count - 1) ?? "")")
        }
    }

    /// The name of the focused window's space at `index`, for the menu's numbered items.
    private func spaceTitle(_ index: Int) -> String? {
        let held = model.windowSpaces
        return held.indices.contains(index) ? model.spaceName(held[index]) : nil
    }
}
