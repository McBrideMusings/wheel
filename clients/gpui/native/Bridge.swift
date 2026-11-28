import AppKit
import WheelKit

// The native half of the GPUI client: the window's key routing. GPUI draws every pane, and the Rust
// pane host (clients/gpui/src/appkit/pane_host.rs) makes, places, masks and focuses the native views
// some panes hold (web pages, maps, video streams). Here a chord GPUI's keymap runs moves the keyboard
// to GPUI's view before it lands, a key equivalent pressed in a native view stays that view's, and a
// check posts keys and menu items without the OS keyboard. Every C entrypoint is called on the main thread.

/// A frame the bridge sends the Rust side on its own initiative: the C callback `attach` was
/// given, with the bridge's window id and one JSON object.
public typealias Emit = @convention(c) (Int32, UnsafePointer<CChar>) -> Void

@MainActor final class Native: NSObject {
    let root: NSView
    /// The address of the slot view (a subview of `root`) that held the keyboard before the key
    /// `routeKey` moved; the Rust side names its pane.
    var heldSlot: UInt?
    var monitor: Any?
    /// The chords the GPUI view runs an action for now, in wheeld's spelling (`keymap` op).
    var routed: Set<String> = []

    init(root: NSView, hidden: Bool) {
        self.root = root
        super.init()
        Self.keyEquivalentsFollowTheKeyboard(root)
        // Test mode: no Dock icon, activation, ordering or key-window call, deaf to the pointer.
        if hidden {
            NSApp.setActivationPolicy(.prohibited)
            root.window?.ignoresMouseEvents = true
        }
        // Hardware keys reach the focused native view first; the pane-cycling shortcuts are the
        // GPUI view's, so a native pane hands the window's keyboard back to it for those.
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.routeKey(event)
            return event
        }
    }

    /// A chord the GPUI view's keymap runs now belongs to it, wherever the keyboard is, as the
    /// Swift app's key monitor runs its keymap before the pane sees the key: make the GPUI view the
    /// first responder before the key lands.
    func routeKey(_ event: NSEvent) {
        guard event.window === root.window else { return }
        // The slot that held the keyboard is named for this key only.
        heldSlot = nil
        guard root.window?.firstResponder !== root, let chord = KeyChord(event: event), routed.contains(chord.text) else { return }
        // The slot that held the keyboard, so the GPUI view knows where cycling starts.
        var view = root.window?.firstResponder as? NSView
        while let v = view, v.superview !== root { view = v.superview }
        heldSlot = view.map { UInt(bitPattern: Unmanaged.passUnretained($0).toOpaque()) }
        _ = root.window?.makeFirstResponder(root)
    }

    /// The item in `menu` whose key equivalent is `key` with `modifiers`.
    static func menuItem(_ menu: NSMenu?, key: String, modifiers: NSEvent.ModifierFlags) -> NSMenuItem? {
        for item in menu?.items ?? [] {
            if let found = menuItem(item.submenu, key: key, modifiers: modifiers) { return found }
            if item.keyEquivalent == key, item.keyEquivalentModifierMask == modifiers { return item }
        }
        return nil
    }

    /// GPUI's view classes whose `performKeyEquivalent:` `keyEquivalentsFollowTheKeyboard` replaced.
    static var keyEquivalentClasses = Set<ObjectIdentifier>()

    /// While a native pane's view inside GPUI's holds the keyboard, a key equivalent goes to the
    /// views under GPUI's, as NSView passes one on, instead of to GPUI. AppKit offers a key carrying Command, Control or
    /// Function (an arrow, ⌘C) to every view in the window before the first responder, and GPUI's
    /// view holds every pane's native view, so GPUI sent an arrow pressed in a web page to its own
    /// text input context, which typed the key's character (0x1C for left) into the page. Every
    /// chord GPUI's keymap runs moves the keyboard to GPUI's view before it lands (`routeKey`), so
    /// none of them is lost.
    static func keyEquivalentsFollowTheKeyboard(_ view: NSView) {
        guard let cls = object_getClass(view), keyEquivalentClasses.insert(ObjectIdentifier(cls)).inserted else { return }
        let sel = #selector(NSView.performKeyEquivalent(with:))
        guard let own = class_getInstanceMethod(cls, sel), let base = class_getInstanceMethod(NSView.self, sel), own != base else { return }
        typealias Imp = @convention(c) (NSView, Selector, NSEvent) -> Bool
        let gpui = unsafeBitCast(method_getImplementation(own), to: Imp.self)
        let nsview = unsafeBitCast(method_getImplementation(base), to: Imp.self)
        let block: @convention(block) (NSView, NSEvent) -> Bool = { view, event in
            // A native pane's view inside GPUI's holds the keyboard: the key is that view's.
            if let held = view.window?.firstResponder as? NSView, held !== view, held.isDescendant(of: view) { return nsview(view, sel, event) }
            return gpui(view, sel, event)
        }
        method_setImplementation(own, imp_implementationWithBlock(block))
    }

    /// The object a nil-targeted action reaches from `window`, as AppKit looks for it: the first
    /// responder and its chain, the window's delegate, the application, its delegate.
    static func target(for action: Selector, from window: NSWindow?) -> NSObject? {
        var responder: NSResponder? = window?.firstResponder
        while let r = responder {
            if r.responds(to: action) { return r }
            responder = r.nextResponder
        }
        let rest: [NSObject?] = [window?.delegate as? NSObject, NSApp, NSApp.delegate as? NSObject]
        return rest.compactMap { $0 }.first { $0.responds(to: action) }
    }

    /// What AppKit asks `target` before it sends `item`'s action: `validateMenuItem:`, else
    /// `validateUserInterfaceItem:`, else enabled.
    static func validates(_ target: NSObject, _ item: NSMenuItem) -> Bool {
        typealias Validate = @convention(c) (NSObject, Selector, NSMenuItem) -> Bool
        for sel in [NSSelectorFromString("validateMenuItem:"), NSSelectorFromString("validateUserInterfaceItem:")] where target.responds(to: sel) {
            guard let imp = class_getMethodImplementation(type(of: target), sel) else { continue }
            return unsafeBitCast(imp, to: Validate.self)(target, sel, item)
        }
        return true
    }

    /// The window is closing: drop the key monitor.
    func detach() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
    }

    // MARK: commands
    func command(_ value: [String: Any]) -> [String: Any] {
        switch value["op"] as? String {
        case "keymap":
            routed = Set((value["chords"] as? [String]) ?? [])
        case "post_key":
            // Real NSEvents to NSWindow.sendEvent: first-responder routing and the views' own key
            // handling run as for hardware events; nothing touches the OS keyboard or focus.
            let text = value["text"] as! String
            let code = UInt16(value["code"] as? Int ?? 0)
            var flags: NSEvent.ModifierFlags = []
            if value["command"] as? Bool == true { flags.insert(.command) }
            if value["shift"] as? Bool == true { flags.insert(.shift) }
            if value["control"] as? Bool == true { flags.insert(.control) }
            if value["option"] as? Bool == true { flags.insert(.option) }
            // An arrow, Home, End or F-key carries these two flags from a hardware keyboard.
            if value["function"] as? Bool == true { flags.formUnion([.function, .numericPad]) }
            var claimed: String?
            for eventType in [NSEvent.EventType.keyDown, .keyUp] {
                if let event = NSEvent.keyEvent(with: eventType, location: .zero, modifierFlags: flags,
                    timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: root.window?.windowNumber ?? 0,
                    context: nil, characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: code) {
                    if eventType == .keyDown {
                        routeKey(event)
                        // NSApplication offers a key down carrying Command, Control or Function to the
                        // window's views as a key equivalent before the first responder gets it.
                        if !flags.isDisjoint(with: [.command, .control, .function]), root.window?.performKeyEquivalent(with: event) == true {
                            claimed = root.window?.firstResponder.map { String(describing: type(of: $0)) }
                            continue
                        }
                    }
                    root.window?.sendEvent(event)
                }
            }
            // The first responder's class when a view claimed the key down as a key equivalent.
            return ["claimed_by": claimed as Any? ?? NSNull()]
        case "menu_key":
            // The main menu's item for a key equivalent, sent as AppKit sends it from the key window:
            // validated, from the first responder. A hidden window is never key, so the menu's own
            // dispatch (which a web view hands a key it does not use) finds no target there.
            let key = value["key"] as! String
            guard let item = Self.menuItem(NSApp.mainMenu, key: key, modifiers: .command), let action = item.action else { return ["sent": false, "item": NSNull()] }
            guard let target = Self.target(for: action, from: root.window), Self.validates(target, item) else { return ["sent": false, "item": item.title] }
            return ["sent": NSApp.sendAction(action, to: target, from: item), "item": item.title, "target": String(describing: type(of: target))]
        default: break
        }
        return status()
    }

    /// The window's keyboard and visibility; `panes` is empty for the Rust pane host to fill.
    func status() -> [String: Any] {
        ["pid": ProcessInfo.processInfo.processIdentifier, "errors": [String](),
         "root_focused": root.window?.firstResponder === root, "held_slot": heldSlot.map { $0 as Any } ?? NSNull(),
         "first_responder": root.window?.firstResponder.map { String(describing: type(of: $0)) } ?? "nil",
         "key_window": root.window?.isKeyWindow ?? false, "visible": root.window?.isVisible ?? false,
         "app_active": NSApp.isActive, "native_subviews": root.subviews.count, "panes": [String: Any]()]
    }
}

/// One Native per GPUI window, by the id `attach` hands back.
@MainActor private var natives: [Int32: Native] = [:]
@MainActor private var nextWindow: Int32 = 1
@_cdecl("wheel_native_attach") @MainActor
public func attach(_ pointer: UnsafeMutableRawPointer, _ hidden: Int32, _ emit: @escaping Emit) -> Int32 {
    let id = nextWindow
    nextWindow += 1
    natives[id] = Native(root: Unmanaged<NSView>.fromOpaque(pointer).takeUnretainedValue(), hidden: hidden != 0)
    return id
}
@_cdecl("wheel_native_command") @MainActor
public func command(_ pointer: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar> {
    do {
        let value = try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as! [String: Any]
        let window = Int32(value["window"] as? Int ?? 0)
        var result: [String: Any]
        if value["op"] as? String == "detach" {
            natives.removeValue(forKey: window)?.detach()
            result = ["detached": true]
        } else {
            // GPUI may render before the bridge attaches to the window.
            result = natives[window]?.command(value) ?? ["detached": true]
        }
        let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
        return strdup(String(decoding: data, as: UTF8.self))!
    } catch { return strdup("{\"error\":\"JSON bridge failed\"}")! }
}
@_cdecl("wheel_native_free")
public func releaseString(_ pointer: UnsafeMutableRawPointer) { free(pointer) }
