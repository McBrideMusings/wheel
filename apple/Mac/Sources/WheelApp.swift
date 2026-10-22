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
    private var badgeWatch: AnyCancellable?
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
        badgeWatch = model.objectWillChange.receive(on: DispatchQueue.main).sink { [weak self] _ in
            DispatchQueue.main.async { self?.updateBadge() }
        }
    }

    private func updateBadge() {
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

/// The menus: each item runs its entry in `CommandActions`, as the command palette does.
struct WheelCommands: Commands {
    @ObservedObject var model: AppModel

    private func item(_ id: String, as title: String? = nil) -> some View {
        let entry = CommandActions.entry(id)
        return Button(title ?? entry?.title ?? id) { CommandActions.run(id, model) }
            .keyboardShortcut(entry?.key.map { KeyboardShortcut($0, modifiers: entry?.modifiers ?? []) })
            .disabled(!(entry?.enabled(model) ?? false))
    }

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            item("new-window")
            item(PaletteAction.openFolderID)
            Divider()
            item("rename-window")
        }
        CommandGroup(replacing: .appSettings) { item("settings", as: "Settings…") }
        CommandGroup(after: .sidebar) {
            item("toggle-sidebar")
            Button("Command Palette…") { model.openCommandPalette() }
                .keyboardShortcut("p", modifiers: [.command, .shift])
        }
        CommandGroup(after: .windowList) { item("usage", as: "Usage") }
        CommandMenu("Pane") {
            item("split-right")
            item("split-down")
            item("close-pane")
            Divider()
            item("switch-view")
        }
    }
}
