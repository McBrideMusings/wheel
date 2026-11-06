import AppKit

/// The menu a right-click on the menu bar item opens: Stop Server (Start Server while it is
/// stopped or unreachable), Restart Server and Quit Wheel. SwiftUI's `MenuBarExtra` with the window
/// style answers only a left-click, so a local monitor catches the right-click on the status
/// item's own window. Stop Server works while the server is unreachable, which is when a server
/// that restarts itself in a loop needs stopping.
@MainActor
final class StatusItemMenu: NSObject {
    private let model: AppModel
    private var monitor: Any?

    init(model: AppModel) {
        self.model = model
        super.init()
        monitor = NSEvent.addLocalMonitorForEvents(matching: .rightMouseDown) { [weak self] event in
            guard let self, let window = event.window,
                  String(describing: type(of: window)).contains("StatusBar"),
                  let view = window.contentView else { return event }
            MainActor.assumeIsolated { NSMenu.popUpContextMenu(self.menu(), with: event, for: view) }
            return nil
        }
    }

    /// Whether the server is running, so the first item stops it; otherwise it starts it.
    private var running: Bool { model.connection == .connected && !model.serverStoppedByMenu }

    func menu() -> NSMenu {
        let menu = NSMenu()
        if running {
            menu.addItem(item("Stop Server…", #selector(stopServer)))
        } else {
            menu.addItem(item("Start Server", #selector(startServer)))
            // Unreachable and restarting itself: stopping unloads it, so it stays down.
            if !model.serverStoppedByMenu { menu.addItem(item("Stop Server", #selector(stopServer))) }
        }
        menu.addItem(item("Restart Server", #selector(restartServer)))
        menu.addItem(.separator())
        let quit = item("Quit Wheel", #selector(quit))
        quit.keyEquivalent = "q"
        menu.addItem(quit)
        return menu
    }

    private func item(_ title: String, _ action: Selector) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
        item.target = self
        return item
    }

    /// A running server holds terminals and agents, so ask first; one that is not running has
    /// nothing to lose, and is stopped at once.
    @objc private func stopServer() {
        if !running {
            model.stopServer()
            return
        }
        let alert = NSAlert()
        alert.messageText = "Stop Wheel Server?"
        alert.informativeText = "Terminals and running agents in \(model.spaces.count) \(model.spaces.count == 1 ? "space" : "spaces") stop."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Stop Server")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate()
        if alert.runModal() == .alertFirstButtonReturn { model.stopServer() }
    }

    @objc private func startServer() { model.startServer() }
    @objc private func restartServer() { model.restartServer() }
    @objc private func quit() { NSApp.terminate(nil) }
}
