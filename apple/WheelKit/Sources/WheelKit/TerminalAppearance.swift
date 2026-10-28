import Foundation

/// How a terminal draws, as the daemon resolved the `terminal.*` settings for one space. A key the
/// daemon does not list, or a value this build does not know, leaves the built-in default for that
/// field.
public struct TerminalAppearance: Sendable, Equatable {
    public enum Theme: String, Sendable, CaseIterable { case standard = "default", solarized, gruvbox, github }
    public enum Cursor: String, Sendable, CaseIterable { case block, bar, underline }

    /// A font family name; empty is the terminal's own font.
    public var fontFamily = ""
    /// Points.
    public var fontSize: Double = 14
    /// A multiple of the font's own row height.
    public var lineHeight: Double = 1.0
    public var theme: Theme = .standard
    public var cursor: Cursor = .block
    public var cursorBlink = true

    public static let standard = TerminalAppearance()

    public init() {}

    public init(settings: [SettingInfo]) {
        func value(_ key: String) -> String? { settings.first { $0.key == key }?.value }
        if let v = value("terminal.font.family") { fontFamily = v }
        if let v = value("terminal.font.size").flatMap(Double.init) { fontSize = v }
        if let v = value("terminal.line_height").flatMap(Double.init) { lineHeight = v }
        if let v = value("terminal.theme").flatMap(Theme.init(rawValue:)) { theme = v }
        if let v = value("terminal.cursor.style").flatMap(Cursor.init(rawValue:)) { cursor = v }
        if let v = value("terminal.cursor.blink") { cursorBlink = v == "true" }
    }

    /// Ghostty's `adjust-cell-height`: the row height's change from the font's own, as a percentage.
    public var cellHeightAdjustment: String {
        let percent = Int(((lineHeight - 1) * 100).rounded())
        return "\(percent)%"
    }

    /// The Ghostty config lines this appearance sets apart from colours, as `(key, value)`.
    public var configLines: [(key: String, value: String)] {
        var lines: [(String, String)] = [("font-size", String(format: "%g", fontSize))]
        if !fontFamily.isEmpty { lines.append(("font-family", fontFamily)) }
        lines.append(("adjust-cell-height", cellHeightAdjustment))
        lines.append(("cursor-style", cursor.rawValue))
        lines.append(("cursor-style-blink", cursorBlink ? "true" : "false"))
        return lines
    }
}

/// The colours of one terminal theme in one appearance: the 16 palette entries, then the page.
public struct TerminalPalette: Sendable, Equatable {
    public let background: String
    public let foreground: String
    /// `#rrggbb` for ANSI colours 0 to 15.
    public let ansi: [String]

    /// The palette for `theme` in the light or dark appearance; nil for the standard theme, which
    /// is the terminal's own.
    public static func of(_ theme: TerminalAppearance.Theme, dark: Bool) -> TerminalPalette? {
        switch theme {
        case .standard: nil
        case .solarized: dark ? solarizedDark : solarizedLight
        case .gruvbox: dark ? gruvboxDark : gruvboxLight
        case .github: dark ? githubDark : githubLight
        }
    }

    private static let solarizedAnsi = [
        "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
        "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
    ]
    static let solarizedDark = TerminalPalette(background: "#002b36", foreground: "#839496", ansi: solarizedAnsi)
    static let solarizedLight = TerminalPalette(background: "#fdf6e3", foreground: "#657b83", ansi: solarizedAnsi)

    static let gruvboxDark = TerminalPalette(background: "#282828", foreground: "#ebdbb2", ansi: [
        "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
        "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
    ])
    static let gruvboxLight = TerminalPalette(background: "#fbf1c7", foreground: "#3c3836", ansi: [
        "#fbf1c7", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#7c6f64",
        "#928374", "#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71", "#427b58", "#3c3836",
    ])

    static let githubDark = TerminalPalette(background: "#0d1117", foreground: "#c9d1d9", ansi: [
        "#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4",
        "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#f0f6fc",
    ])
    static let githubLight = TerminalPalette(background: "#ffffff", foreground: "#24292f", ansi: [
        "#24292f", "#cf222e", "#116329", "#4d2d00", "#0969da", "#8250df", "#1b7c83", "#6e7781",
        "#57606a", "#a40e26", "#1a7f37", "#633c01", "#218bff", "#a475f9", "#3192aa", "#8c959f",
    ])
}
