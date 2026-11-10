import AppKit
import SwiftUI
import WheelKit

/// Makes one `NSWindow` for each record of the model's window layout and closes the ones whose
/// record went (ADR-0014). The windows are AppKit's, not SwiftUI scenes', so they open at launch
/// with no scene to hold them, in any number, and reopen from the saved layout.
@MainActor
final class WindowManager: NSObject, NSWindowDelegate, WindowHost {
    static let idPrefix = "wheel-window-"
    /// A little window is a Wheel window too (keys, selection and screenshots treat it as one); its id has this longer prefix.
    static let littlePrefix = idPrefix + "little-"

    private unowned let model: AppModel
    private var windows: [String: NSWindow] = [:]
    private var littles: [String: NSWindow] = [:]
    /// Windows this manager is closing because their record went; the person did not close them.
    private var closing: Set<String> = []

    /// The frame the system saved for a window that opened somewhere else: AppKit moves and shrinks a
    /// restored frame onto the screens it can see at that moment, and a monitor that attaches a moment
    /// after launch is not among them. The saved frame goes back when the screens change.
    private var displaced: [String: NSRect] = [:]

    init(model: AppModel) {
        self.model = model
        super.init()
        NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.putDisplacedBack() }
        }
    }

    private static func savedFrame(_ name: String) -> NSRect? {
        guard let text = UserDefaults.standard.string(forKey: "NSWindow Frame \(name)") else { return nil }
        let numbers = text.split(separator: " ").compactMap { Double($0) }
        guard numbers.count >= 4 else { return nil }
        return NSRect(x: numbers[0], y: numbers[1], width: numbers[2], height: numbers[3])
    }

    private func putDisplacedBack() {
        let screens = NSScreen.screens.map { "\($0.localizedName) \(NSStringFromRect($0.frame))" }.joined(separator: "; ")
        Log.info("screens changed: \(screens)")
        for (id, wanted) in displaced {
            guard let window = windows[id],
                  NSScreen.screens.contains(where: { $0.frame.contains(CGPoint(x: wanted.midX, y: wanted.midY)) }) else { continue }
            window.setFrame(wanted, display: true)
            displaced[id] = nil
            Log.info("window \(id.prefix(8)) put back at its saved frame \(NSStringFromRect(window.frame)) on \(window.screen?.localizedName ?? "no screen")")
        }
    }

    /// The `NSWindow` that shows record `id`, if it is open.
    func window(_ id: String) -> NSWindow? { windows[id] }

    func littleWindow(_ id: String) -> NSWindow? { littles[id] }

    func sync() {
        let wanted = Set(model.layout.windows.map(\.id))
        for record in model.layout.windows {
            if let window = windows[record.id] {
                Self.show(title: model.title(of: record), of: window)
                Self.apply(pinned: record.pinned, to: window)
            } else {
                open(record)
            }
        }
        for id in windows.keys where !wanted.contains(id) {
            closing.insert(id)
            windows[id]?.close()
        }
        let wantedLittles = Set(model.layout.littles.map(\.id))
        for little in model.layout.littles where littles[little.id] == nil { openLittle(little) }
        for little in model.layout.littles {
            let title = model.spaceName(little.space) ?? "Wheel"
            if let window = littles[little.id] {
                if window.title != title { window.title = title }
                Self.apply(pinned: little.pinned, to: window)
            }
        }
        for id in littles.keys where !wantedLittles.contains(id) {
            closing.insert(id)
            littles[id]?.close()
        }
    }

    /// The title the Window menu lists for a main window, whose own title stays empty so the system never draws it over the title row.
    private static func show(title: String, of window: NSWindow) {
        NSApp.changeWindowsItem(window, title: title, filename: false)
    }

    /// A pinned window floats above every unpinned window of any app and follows the person to every desktop.
    static func apply(pinned: Bool, to window: NSWindow) {
        let level: NSWindow.Level = pinned ? .floating : .normal
        if window.level != level { window.level = level }
        if pinned { window.collectionBehavior.insert(.canJoinAllSpaces) } else { window.collectionBehavior.remove(.canJoinAllSpaces) }
    }

    /// A little window: the space's grid under a 30 pt header strip, the traffic lights in the strip.
    private func openLittle(_ little: LittleWindow) {
        let controller = NSHostingController(rootView: LittleWindowView(model: model, littleID: little.id))
        // The window keeps the size it is given; a pane's own content never grows it.
        controller.sizingOptions = []
        let window = NSWindow(contentViewController: controller)
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        window.identifier = NSUserInterfaceItemIdentifier(Self.littlePrefix + little.id)
        window.title = model.spaceName(little.space) ?? "Wheel"
        window.isReleasedWhenClosed = false
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.tabbingMode = .disallowed
        window.delegate = self
        window.contentMinSize = NSSize(width: 320, height: 200)
        let size = NSSize(width: 480, height: 330)
        let final: NSRect
        if let card = model.littleOrigin {
            final = Self.frame(size, centeredOn: card)
        } else {
            window.setContentSize(size)
            // The size and place a person gave this space's little window last time.
            if !window.setFrameAutosaveName("WheelLittle-\(little.space)") { window.center() }
            final = window.frame
        }
        littles[little.id] = window
        Self.apply(pinned: little.pinned, to: window)
        if let card = model.littleOrigin, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            // The card grows into the window: AppKit's own frame animation, from the card's frame to the window's.
            window.setFrame(card, display: true)
            window.orderFront(nil)
            window.setFrame(final, display: true, animate: true)
        } else {
            window.setFrame(final, display: true)
            window.orderFront(nil)
        }
        if model.littleOrigin != nil { window.setFrameAutosaveName("WheelLittle-\(little.space)") }
        Log.info("little window \(little.id.prefix(8)) opened for space \(little.space) at \(NSStringFromRect(window.frame)), owner \(little.owner.map { String($0.prefix(8)) } ?? "none")")
    }

    /// A window of `size` centred on `card`, kept on the screen the card is on.
    private static func frame(_ size: NSSize, centeredOn card: CGRect) -> NSRect {
        let screen = NSScreen.screens.first { $0.frame.contains(CGPoint(x: card.midX, y: card.midY)) } ?? NSScreen.main
        var frame = NSRect(x: card.midX - size.width / 2, y: card.midY - size.height / 2, width: size.width, height: size.height)
        if let visible = screen?.visibleFrame {
            frame.origin.x = min(max(frame.minX, visible.minX), visible.maxX - frame.width)
            frame.origin.y = min(max(frame.minY, visible.minY), visible.maxY - frame.height)
        }
        return frame
    }

    func moveLittle(_ id: String, to card: CGRect) {
        guard let window = littles[id] else { return }
        window.setFrame(Self.frame(window.frame.size, centeredOn: card), display: true)
        Log.info("little window \(id.prefix(8)) moved to \(NSStringFromRect(window.frame))")
    }

    func bringFrontLittle(_ id: String) {
        guard let window = littles[id] else { return }
        if window.isMiniaturized { window.deminiaturize(nil) }
        window.makeKeyAndOrderFront(nil)
    }

    private func open(_ record: WindowRecord) {
        let controller = NSHostingController(rootView: ContentView(model: model, settings: model.settings, windowID: record.id))
        // The window keeps the size the system saved for it; the content only sets its minimum.
        controller.sizingOptions = [.minSize]
        let window = NSWindow(contentViewController: controller)
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        window.identifier = NSUserInterfaceItemIdentifier(Self.idPrefix + record.id)
        window.isReleasedWhenClosed = false
        // The content draws its own title row (`TitleRow`) under a transparent title bar, so the
        // system's own title stays empty: the Window menu's entry is set by `show(title:)`. The empty
        // toolbar only gives the title bar its height, with the traffic lights centred in it.
        window.title = ""
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.toolbar = NSToolbar(identifier: "WheelTitleRow")
        window.toolbarStyle = .unifiedCompact
        // A press in the title bar would move the window before a tab's own drag sees it; only the
        // title row's empty ground moves the window (`TitleBarGround` calls `performDrag`).
        window.isMovable = false
        window.tabbingMode = .disallowed
        window.delegate = self
        window.setContentSize(NSSize(width: 1200, height: 800))
        let restored = window.setFrameAutosaveName("WheelWindow-\(record.id)")
        // A frame the system saved for this window is kept as it was: same screen, same place, same size.
        // Only a window with no saved frame is centred, and cascaded below another visible one.
        if !restored {
            window.center()
            if let last = windows.values.first(where: { $0.isVisible }) {
                window.cascadeTopLeft(from: NSPoint(x: last.frame.minX, y: last.frame.maxY))
            }
        }
        if restored, let wanted = Self.savedFrame("WheelWindow-\(record.id)"), abs(wanted.minX - window.frame.minX) > 1 || abs(wanted.width - window.frame.width) > 1 {
            displaced[record.id] = wanted
            Log.info("window \(record.id.prefix(8)) restored to \(NSStringFromRect(window.frame)), saved \(NSStringFromRect(wanted))")
        }
        Log.info("screens at open: \(NSScreen.screens.map { "\($0.localizedName) \(NSStringFromRect($0.frame))" }.joined(separator: "; "))")
        Log.info("window \(record.id.prefix(8)) opened \(restored ? "at its saved frame" : "with no saved frame") \(NSStringFromRect(window.frame)) on \(window.screen?.localizedName ?? "no screen")")
        windows[record.id] = window
        Self.show(title: model.title(of: record), of: window)
        putDisplacedBack()
        Self.apply(pinned: record.pinned, to: window)
        // `-g` launches and debug runs must not take the keyboard from whatever the person is using.
        if NSApp.isActive { window.makeKeyAndOrderFront(nil) } else { window.orderFront(nil) }
        Log.info("window \(record.id.prefix(8)) opened, \(model.layout.spaces(of: record).count) spaces")
    }

    func bringFront(_ id: String) {
        guard let window = windows[id] else { return }
        if window.isMiniaturized { window.deminiaturize(nil) }
        if NSApp.isActive { window.makeKeyAndOrderFront(nil) } else { window.orderFront(nil) }
    }

    static func littleID(of window: NSWindow) -> String? {
        guard let raw = window.identifier?.rawValue, raw.hasPrefix(Self.littlePrefix) else { return nil }
        return String(raw.dropFirst(Self.littlePrefix.count))
    }

    static func id(of window: NSWindow) -> String? {
        guard let raw = window.identifier?.rawValue, raw.hasPrefix(Self.idPrefix) else { return nil }
        return String(raw.dropFirst(Self.idPrefix.count))
    }

    /// The red light on the last window showing a session that holds spaces asks first; the sheet's answer closes it.
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard let id = Self.id(of: sender), Self.littleID(of: sender) == nil, !model.quitting,
              model.windowCloseNeedsAnswer(id) else { return true }
        model.askCloseWindow(id)
        return false
    }

    func windowWillClose(_ notification: Notification) {
        if let window = notification.object as? NSWindow, let little = Self.littleID(of: window) {
            littles[little] = nil
            if closing.remove(little) != nil { return }
            Log.info("little window \(little.prefix(8)) closed by the person")
            model.littleClosedByPerson(little)
            return
        }
        guard let window = notification.object as? NSWindow, let id = Self.id(of: window) else { return }
        windows[id] = nil
        // A window that is gone for good takes the size the system saved for it along.
        if !model.quitting { NSWindow.removeFrame(usingName: "WheelWindow-\(id)") }
        if closing.remove(id) != nil { return }
        Log.info("window \(id.prefix(8)) closed by the person")
        model.windowClosedByPerson(id)
    }

    func windowWillStartLiveResize(_ notification: Notification) { model.windowResize(began: true) }

    func windowDidEndLiveResize(_ notification: Notification) { model.windowResize(began: false) }

    func windowDidBecomeKey(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, let id = Self.id(of: window) else { return }
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
