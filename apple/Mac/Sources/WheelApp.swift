import SwiftUI

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
    }

    var body: some Scene {
        // One window: the lead card beside the active space's grid.
        Window("Wheel", id: "main") {
            ContentView(model: model, settings: model.settings, sidebar: model.sidebar)
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
        .commands {
            // A keyboard-triggered change does not animate.
            CommandGroup(after: .sidebar) {
                Button(model.sidebar.mode == .split ? "Hide Lead Sidebar" : (model.sidebar.mode == .full ? "Show Spaces" : "Show Lead Sidebar")) {
                    model.sidebar.toggleHidden(animated: false)
                }
                .keyboardShortcut("s", modifiers: [.control, .command])
                Button(model.sidebar.mode == .full ? "Restore Lead Sidebar" : "Expand Lead to Full Window") {
                    model.sidebar.toggleFull(animated: false)
                }
                .keyboardShortcut("l", modifiers: [.control, .command])
            }
        }
        // Wheel's own settings, which live in the daemon: the app menu's Settings… (⌘,).
        Settings {
            SettingsView(model: model, settings: model.settings)
        }
    }
}
