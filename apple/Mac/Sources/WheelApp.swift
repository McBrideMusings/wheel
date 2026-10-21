import SwiftUI
import WheelKit
import WheelUI

@main
struct WheelApp: App {
    @StateObject private var model = AppModel()
    @State private var started = false
    #if DEBUG
    @State private var debug: DebugServer?
    #endif

    init() {
        // Everything the window shows lives in the daemon (ADR-0001), so AppKit has nothing to
        // restore — and a restore that comes back without the main window starts no connection,
        // since the window's task is what starts it. Every launch opens the main window instead.
        UserDefaults.standard.register(defaults: ["ApplePersistenceIgnoreState": true])
        UILog.sink = { Log.info($0) }
    }

    var body: some Scene {
        // One window: the active space's grid.
        Window("Wheel", id: "main") {
            ContentView(model: model, settings: model.settings)
                .task {
                    guard !started else { return }
                    started = true
                    // WHEEL_APPEARANCE=light|dark pins the appearance, so both can be checked on one machine.
                    switch ProcessInfo.processInfo.environment["WHEEL_APPEARANCE"] {
                    case "light": NSApp.appearance = NSAppearance(named: .aqua)
                    case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
                    default: break
                    }
                    #if DEBUG
                    let server = DebugServer(model: model)
                    server.start()
                    debug = server
                    #endif
                    model.start()
                }
        }
        // The coding agents' usage, opened by the strip's dial.
        Window("Usage", id: "usage") {
            UsageWindow(model: model, usage: model.usage)
        }
        .windowResizability(.contentSize)
        // Wheel's own settings, which live in the daemon: the app menu's Settings… (⌘,).
        Settings {
            SettingsView(model: model, settings: model.settings)
        }
        .commands { WheelCommands(model: model) }
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
        CommandGroup(replacing: .newItem) { item(PaletteAction.openFolderID) }
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
