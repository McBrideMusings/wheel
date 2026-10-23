import Foundation

/// One key and its modifiers, in the spelling wheeld stores a `keybind` trigger in:
/// `ctrl+opt+shift+cmd+<key>`, modifiers in that order, the key a lowercase letter, a digit,
/// `f1`–`f12` or a name (`enter`, `arrow_left`, `bracket_left`, …). wheeld decides every alias, so
/// this reads that one spelling only.
public struct KeyChord: Hashable, Sendable {
    public struct Modifiers: OptionSet, Hashable, Sendable {
        public let rawValue: Int
        public init(rawValue: Int) { self.rawValue = rawValue }

        public static let ctrl = Modifiers(rawValue: 1)
        public static let opt = Modifiers(rawValue: 2)
        public static let shift = Modifiers(rawValue: 4)
        public static let cmd = Modifiers(rawValue: 8)

        /// In the order they are written and printed: ⌃⌥⇧⌘.
        static let order: [(Modifiers, name: String, symbol: String)] = [
            (.ctrl, "ctrl", "⌃"), (.opt, "opt", "⌥"), (.shift, "shift", "⇧"), (.cmd, "cmd", "⌘"),
        ]
    }

    public let key: String
    public let modifiers: Modifiers

    /// The named keys that type a character, with the character they type unshifted. wheeld's
    /// `keybind.rs` names the same keys.
    public static let printing: [String: Character] = [
        "comma": ",", "period": ".", "slash": "/", "semicolon": ";", "quote": "'", "backquote": "`",
        "minus": "-", "equal": "=", "backslash": "\\", "bracket_left": "[", "bracket_right": "]",
    ]

    /// The keys a chord can name, with what a menu prints for each.
    static let named: [String: String] = [
        "enter": "↩", "tab": "⇥", "space": "Space", "escape": "⎋", "backspace": "⌫", "delete": "⌦",
        "arrow_up": "↑", "arrow_down": "↓", "arrow_left": "←", "arrow_right": "→",
        "home": "↖", "end": "↘", "page_up": "⇞", "page_down": "⇟",
    ].merging(printing.mapValues(String.init)) { a, _ in a }

    /// The character a printing key types unshifted: `d`, `1`, `[` for `bracket_left`.
    public static func character(_ key: String) -> Character? {
        if let c = printing[key] { return c }
        return key.count == 1 ? key.first : nil
    }

    /// The key a printing character is on: `d`, `1`, `bracket_left` for `[`.
    public static func name(of character: Character) -> String? {
        if character.isASCII, character.isLetter || character.isNumber { return String(character).lowercased() }
        return printing.first { $0.value == character }?.key
    }

    public init(key: String, modifiers: Modifiers) {
        self.key = key
        self.modifiers = modifiers
    }

    /// Nil for anything not in the stored spelling.
    public init?(_ text: String) {
        var parts = text.split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        guard let key = parts.popLast(), Self.isKey(key) else { return nil }
        var mods: Modifiers = []
        var next = 0
        for part in parts {
            // Each modifier once, in order.
            guard let i = Modifiers.order[next...].firstIndex(where: { $0.name == part }) else { return nil }
            mods.insert(Modifiers.order[i].0)
            next = i + 1
        }
        self.init(key: key, modifiers: mods)
    }

    static func isKey(_ key: String) -> Bool {
        if key.count == 1, let c = key.first, c.isASCII, c.isLowercase || c.isNumber { return true }
        if key.hasPrefix("f"), let n = Int(key.dropFirst()), (1 ... 12).contains(n), key == "f\(n)" { return true }
        return named[key] != nil
    }

    /// `shift+cmd+d`, as wheeld stores it.
    public var text: String {
        (Modifiers.order.filter { modifiers.contains($0.0) }.map(\.name) + [key]).joined(separator: "+")
    }

    /// `⇧⌘D`, as a menu prints it.
    public var symbol: String {
        Modifiers.order.filter { modifiers.contains($0.0) }.map(\.symbol).joined() + (Self.named[key] ?? key.uppercased())
    }
}

/// Which chord runs which action: the app's built-in bindings with the `keybind` setting applied
/// over them, entry by entry, the way Ghostty applies its `keybind` lines. An action has at most
/// one chord and a chord runs at most one action.
public struct Keymap: Sendable, Equatable {
    /// By action id.
    public private(set) var chords: [String: KeyChord]
    /// What in the setting was not applied, one line each, to show beside it.
    public private(set) var problems: [String] = []

    /// `defaults` is every action the app has, with its built-in chord or none.
    public init(defaults: [(action: String, chord: KeyChord?)], setting: String = "") {
        chords = Dictionary(defaults.compactMap { d in d.chord.map { (d.action, $0) } }, uniquingKeysWith: { a, _ in a })
        let actions = Set(defaults.map(\.action))
        for entry in setting.split(separator: ",").map(String.init) where !entry.isEmpty {
            if entry == "clear" {
                chords = [:]
                continue
            }
            let parts = entry.split(separator: "=", maxSplits: 1).map(String.init)
            guard parts.count == 2, let chord = KeyChord(parts[0]) else {
                problems.append("\(entry): not a binding this app can read")
                continue
            }
            let action = parts[1]
            if action != "unbind", !actions.contains(action) {
                problems.append("\(entry): Wheel has no action \(action)")
                continue
            }
            for (other, bound) in chords where bound == chord { chords[other] = nil }
            if action != "unbind" { chords[action] = chord }
        }
    }

    public func action(for chord: KeyChord) -> String? {
        chords.first { $0.value == chord }?.key
    }
}
