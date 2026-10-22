import AppKit
import SwiftUI
import WheelKit

/// Makes one `NSWindow` for each record of the model's window layout and closes the ones whose
/// record went (ADR-0014). The windows are AppKit's, not SwiftUI scenes', so they open at launch
/// with no scene to hold them, in any number, and reopen from the saved layout.
@MainActor
final class WindowManager: NSObject, NSWindowDelegate, WindowHost {
    static let idPrefix = "wheel-window-"

    private unowned let model: AppModel
    private var windows: [String: NSWindow] = [:]
    /// Windows this manager is closing because their record went; the person did not close them.
    private var closing: Set<String> = []

    init(model: AppModel) {
        self.model = model
    }

    /// The `NSWindow` that shows record `id`, if it is open.
    func window(_ id: String) -> NSWindow? { windows[id] }

    func sync() {
        let wanted = Set(model.layout.windows.map(\.id))
        for record in model.layout.windows {
            if let window = windows[record.id] {
                let title = model.title(of: record)
                if window.title != title { window.title = title }
            } else {
                open(record)
            }
        }
        for id in windows.keys where !wanted.contains(id) {
            closing.insert(id)
            windows[id]?.close()
        }
    }

    private func open(_ record: WindowRecord) {
        let controller = NSHostingController(rootView: ContentView(model: model, settings: model.settings, windowID: record.id))
        // The window keeps the size the system saved for it; the content only sets its minimum.
        controller.sizingOptions = [.minSize]
        let window = NSWindow(contentViewController: controller)
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.identifier = NSUserInterfaceItemIdentifier(Self.idPrefix + record.id)
        window.title = model.title(of: record)
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.delegate = self
        window.setContentSize(NSSize(width: 1200, height: 800))
        let restored = window.setFrameAutosaveName("WheelWindow-\(record.id)")
        if !restored || windows.isEmpty == false {
            window.center()
            if let last = windows.values.first(where: { $0.isVisible }) {
                let origin = window.cascadeTopLeft(from: NSPoint(x: last.frame.minX, y: last.frame.maxY))
                _ = origin
            }
        }
        windows[record.id] = window
        // `-g` launches and debug runs must not take the keyboard from whatever the person is using.
        if NSApp.isActive { window.makeKeyAndOrderFront(nil) } else { window.orderFront(nil) }
        Log.info("window \(record.id.prefix(8)) opened, \(record.spaces.count) spaces")
    }

    func bringFront(_ id: String) {
        guard let window = windows[id] else { return }
        if window.isMiniaturized { window.deminiaturize(nil) }
        if NSApp.isActive { window.makeKeyAndOrderFront(nil) } else { window.orderFront(nil) }
    }

    private func id(of window: NSWindow) -> String? {
        guard let raw = window.identifier?.rawValue, raw.hasPrefix(Self.idPrefix) else { return nil }
        return String(raw.dropFirst(Self.idPrefix.count))
    }

    func windowWillClose(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, let id = id(of: window) else { return }
        windows[id] = nil
        if closing.remove(id) != nil { return }
        Log.info("window \(id.prefix(8)) closed by the person")
        model.windowClosedByPerson(id)
    }

    func windowDidBecomeKey(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, let id = id(of: window) else { return }
        model.windowBecameFocused(id)
    }
}

extension NSApplication {
    /// The window the person is in, or the one last used: the one a screenshot or a pane draw means.
    var wheelWindow: NSWindow? {
        let all = windows.filter { $0.identifier?.rawValue.hasPrefix(WindowManager.idPrefix) == true }
        return all.first { $0.isKeyWindow } ?? all.first { $0.isMainWindow } ?? all.first
    }
}

extension NSWindow {
    var isWheelWindow: Bool { identifier?.rawValue.hasPrefix(WindowManager.idPrefix) == true }
}
