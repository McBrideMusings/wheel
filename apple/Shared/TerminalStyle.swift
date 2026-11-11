import GhosttyTerminal
import WheelKit

/// How a space's `terminal.*` settings become a Ghostty controller's configuration and colours,
/// for the Mac app and the phone alike.
enum TerminalStyle {
    /// The Mac's keybinds: Ghostty's defaults are cleared, so a ⌘ key Wheel's keymap leaves unbound
    /// reaches the program in the terminal, and the keys the surface still handles itself (copy,
    /// paste, select all, font size) are bound again.
    static let macKeybinds = [
        "super+c=copy_to_clipboard",
        "super+v=paste_from_clipboard",
        "super+a=select_all",
        "super+equal=increase_font_size:1",
        "super+plus=increase_font_size:1",
        "super+minus=decrease_font_size:1",
        "super+zero=reset_font_size",
    ]

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

    /// A Mac controller drawing `look`, with the Mac's keybinds.
    @MainActor static func controller(_ look: TerminalAppearance) -> TerminalController {
        TerminalController(configuration: configuration(look, keybinds: macKeybinds), theme: theme(look))
    }

    /// Restyle a Mac controller to `look` in place, so its shells keep running; whether Ghostty
    /// took the configuration and the theme.
    @MainActor @discardableResult
    static func restyle(_ controller: TerminalController, to look: TerminalAppearance) -> (config: Bool, theme: Bool) {
        (controller.setTerminalConfiguration(configuration(look, keybinds: macKeybinds)), controller.setTheme(theme(look)))
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
