import AppKit
import WheelKit

/// The keys a chord can name, by key code: what a key event is read as, and what the debug
/// endpoint sends for a chord. Printing keys carry the US layout's characters, unshifted and
/// shifted; a key event is read by what its layout types, so these matter only for sending.
enum KeyCodes {
    struct Key {
        let code: UInt16
        let plain: String
        let shifted: String
    }

    static let keys: [String: Key] = {
        var keys: [String: Key] = [:]
        let letters: [(String, UInt16)] = [
            ("a", 0x00), ("s", 0x01), ("d", 0x02), ("f", 0x03), ("h", 0x04), ("g", 0x05), ("z", 0x06), ("x", 0x07),
            ("c", 0x08), ("v", 0x09), ("b", 0x0B), ("q", 0x0C), ("w", 0x0D), ("e", 0x0E), ("r", 0x0F), ("y", 0x10),
            ("t", 0x11), ("o", 0x1F), ("u", 0x20), ("i", 0x22), ("p", 0x23), ("l", 0x25), ("j", 0x26), ("k", 0x28),
            ("n", 0x2D), ("m", 0x2E),
        ]
        for (c, code) in letters { keys[c] = Key(code: code, plain: c, shifted: c.uppercased()) }
        // The digit row and the punctuation keys, each with what Shift makes it type on a US layout.
        let printing: [(String, UInt16, String)] = [
            ("1", 0x12, "!"), ("2", 0x13, "@"), ("3", 0x14, "#"), ("4", 0x15, "$"), ("6", 0x16, "^"), ("5", 0x17, "%"),
            ("equal", 0x18, "+"), ("9", 0x19, "("), ("7", 0x1A, "&"), ("minus", 0x1B, "_"), ("8", 0x1C, "*"), ("0", 0x1D, ")"),
            ("bracket_right", 0x1E, "}"), ("bracket_left", 0x21, "{"), ("quote", 0x27, "\""), ("semicolon", 0x29, ":"),
            ("backslash", 0x2A, "|"), ("comma", 0x2B, "<"), ("slash", 0x2C, "?"), ("period", 0x2F, ">"), ("backquote", 0x32, "~"),
        ]
        for (name, code, shifted) in printing {
            keys[name] = Key(code: code, plain: KeyChord.character(name).map(String.init) ?? name, shifted: shifted)
        }
        for (name, key) in special { keys[name] = key }
        return keys
    }()

    /// Keys that type no printing character, read by their key code whatever the layout.
    static let special: [String: Key] = {
        func fn(_ scalar: Int) -> String { String(Character(UnicodeScalar(scalar)!)) }
        var keys: [String: Key] = [
            "enter": Key(code: 0x24, plain: "\r", shifted: "\r"),
            "tab": Key(code: 0x30, plain: "\t", shifted: "\t"),
            "space": Key(code: 0x31, plain: " ", shifted: " "),
            "backspace": Key(code: 0x33, plain: "\u{7F}", shifted: "\u{7F}"),
            "escape": Key(code: 0x35, plain: "\u{1B}", shifted: "\u{1B}"),
            "arrow_left": Key(code: 0x7B, plain: fn(NSLeftArrowFunctionKey), shifted: fn(NSLeftArrowFunctionKey)),
            "arrow_right": Key(code: 0x7C, plain: fn(NSRightArrowFunctionKey), shifted: fn(NSRightArrowFunctionKey)),
            "arrow_down": Key(code: 0x7D, plain: fn(NSDownArrowFunctionKey), shifted: fn(NSDownArrowFunctionKey)),
            "arrow_up": Key(code: 0x7E, plain: fn(NSUpArrowFunctionKey), shifted: fn(NSUpArrowFunctionKey)),
            "home": Key(code: 0x73, plain: fn(NSHomeFunctionKey), shifted: fn(NSHomeFunctionKey)),
            "end": Key(code: 0x77, plain: fn(NSEndFunctionKey), shifted: fn(NSEndFunctionKey)),
            "page_up": Key(code: 0x74, plain: fn(NSPageUpFunctionKey), shifted: fn(NSPageUpFunctionKey)),
            "page_down": Key(code: 0x79, plain: fn(NSPageDownFunctionKey), shifted: fn(NSPageDownFunctionKey)),
            "delete": Key(code: 0x75, plain: fn(NSDeleteFunctionKey), shifted: fn(NSDeleteFunctionKey)),
        ]
        let fCodes: [UInt16] = [0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F]
        for (i, code) in fCodes.enumerated() {
            let c = fn(NSF1FunctionKey + i)
            keys["f\(i + 1)"] = Key(code: code, plain: c, shifted: c)
        }
        return keys
    }()

    /// The keypad's Enter is Enter.
    private static let specialByCode: [UInt16: String] = Dictionary(
        special.map { ($0.value.code, $0.key) } + [(0x4C, "enter")], uniquingKeysWith: { a, _ in a })
    private static let byCode: [UInt16: String] = Dictionary(keys.map { ($0.value.code, $0.key) }, uniquingKeysWith: { a, _ in a })

    /// The name of the key an event's key code is, for a key that types no printing character.
    static func specialName(_ code: UInt16) -> String? { specialByCode[code] }

    /// The key at that key code's place on a US keyboard: what a layout whose key types no
    /// character a chord can name (Cyrillic, or AZERTY's digit row) is read as.
    static func usName(_ code: UInt16) -> String? { byCode[code] }
}

extension KeyChord {
    /// The chord a key event is: its modifiers, and its key by key code for the keys that type no
    /// character, else by the character the layout types for it with no modifier held, else by
    /// where the key sits on a US keyboard.
    init?(event: NSEvent) {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        var mods: Modifiers = []
        if flags.contains(.control) { mods.insert(.ctrl) }
        if flags.contains(.option) { mods.insert(.opt) }
        if flags.contains(.shift) { mods.insert(.shift) }
        if flags.contains(.command) { mods.insert(.cmd) }
        if let name = KeyCodes.specialName(event.keyCode) {
            self.init(key: name, modifiers: mods)
            return
        }
        let typed = event.characters(byApplyingModifiers: []).flatMap { $0.count == 1 ? $0.first : nil }
        guard let name = typed.flatMap(KeyChord.name(of:)) ?? KeyCodes.usName(event.keyCode) else { return nil }
        self.init(key: name, modifiers: mods)
    }
}

extension AppModel {
    /// Read every key pressed in a Wheel window against the keymap before anything else sees it,
    /// as Ghostty reads its `keybind`s: a bound key whose action can run now runs it and goes no
    /// further; any other key goes on to the menus and the pane holding the keyboard.
    func installKeyMonitor() {
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            let ran = MainActor.assumeIsolated { self?.runKey(event) ?? false }
            return ran ? nil : event
        }
    }

    /// Run the action `event` is bound to; false when the keymap leaves the key to the rest of the app.
    func runKey(_ event: NSEvent) -> Bool {
        // Esc closes the shortcut sheet wherever the keyboard is, a terminal included.
        if shortcutsOpen, event.window?.isWheelWindow == true, KeyCodes.specialName(event.keyCode) == "escape",
           event.modifierFlags.intersection(.deviceIndependentFlagsMask).isEmpty {
            closeShortcuts()
            return true
        }
        guard event.window?.isWheelWindow == true, let chord = KeyChord(event: event), let id = keymap.action(for: chord) else {
            return false
        }
        guard let entry = CommandActions.entry(id, self), entry.enabled(self) else {
            Log.info("key \(chord.text): \(id) cannot run now; the key goes on")
            return false
        }
        Log.info("key \(chord.text): \(id)")
        CommandActions.run(id, self)
        return true
    }
}
