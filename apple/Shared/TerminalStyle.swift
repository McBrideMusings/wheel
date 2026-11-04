import GhosttyTerminal
import WheelKit

/// How a space's `terminal.*` settings become a Ghostty controller's configuration and colours,
/// for the Mac app and the phone alike.
enum TerminalStyle {
    /// Padding inside the terminal's own background, then the look's font and cursor. A non-nil
    /// `keybinds` clears Ghostty's default keybinds and binds only those, so a key the app leaves
    /// unbound reaches the program in the terminal.
    static func configuration(_ look: TerminalAppearance, keybinds: [String]? = nil) -> TerminalConfiguration {
        TerminalConfiguration(startingFrom: .default) {
            $0.withWindowPaddingX(10)
            $0.withWindowPaddingY(8)
            if let keybinds {
                $0.withCustom("keybind", "clear")
                for bind in keybinds { $0.withCustom("keybind", bind) }
            }
            for line in look.configLines { $0.withCustom(line.key, line.value) }
        }
    }

    /// Ghostty's own light/dark pair for the default theme, else the named theme's two palettes.
    static func theme(_ look: TerminalAppearance) -> TerminalTheme {
        func colours(_ palette: TerminalPalette) -> TerminalConfiguration {
            TerminalConfiguration {
                $0.withBackground(palette.background)
                $0.withForeground(palette.foreground)
                for (index, colour) in palette.ansi.enumerated() { $0.withPalette(index, color: colour) }
            }
        }
        guard let light = TerminalPalette.of(look.theme, dark: false), let dark = TerminalPalette.of(look.theme, dark: true) else {
            return .default
        }
        return TerminalTheme(light: colours(light), dark: colours(dark))
    }
}
