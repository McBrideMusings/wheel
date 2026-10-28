import Foundation
import Testing
@testable import WheelKit

@Suite struct TerminalAppearanceTests {
    private func setting(_ key: String, _ value: String) -> SettingInfo {
        SettingInfo(key: key, value: value, origin: .machine, default: "", help: "")
    }

    @Test func readsTheTerminalKeysIntoGhosttyLines() {
        let look = TerminalAppearance(settings: [
            setting("terminal.font.size", "15"),
            setting("terminal.font.family", "Menlo"),
            setting("terminal.line_height", "1.2"),
            setting("terminal.theme", "gruvbox"),
            setting("terminal.cursor.style", "bar"),
            setting("terminal.cursor.blink", "false"),
        ])
        #expect(look.theme == .gruvbox)
        let lines = Dictionary(uniqueKeysWithValues: look.configLines.map { ($0.key, $0.value) })
        #expect(lines["font-size"] == "15")
        #expect(lines["font-family"] == "Menlo")
        #expect(lines["adjust-cell-height"] == "20%")
        #expect(lines["cursor-style"] == "bar")
        #expect(lines["cursor-style-blink"] == "false")
    }

    @Test func anEmptyListIsTheDefaultsAndAnEmptyFamilyIsLeftOut() {
        #expect(TerminalAppearance(settings: []) == .standard)
        #expect(!TerminalAppearance.standard.configLines.contains { $0.key == "font-family" })
        #expect(TerminalAppearance.standard.cellHeightAdjustment == "0%")
        var tight = TerminalAppearance.standard
        tight.lineHeight = 0.9
        #expect(tight.cellHeightAdjustment == "-10%")
    }

    @Test func everyThemeHasALightAndADarkPaletteOfSixteenColours() {
        #expect(TerminalPalette.of(.standard, dark: true) == nil)
        for theme in TerminalAppearance.Theme.allCases where theme != .standard {
            let light = TerminalPalette.of(theme, dark: false)
            let dark = TerminalPalette.of(theme, dark: true)
            #expect(light?.ansi.count == 16 && dark?.ansi.count == 16, "\(theme)")
            #expect(light?.background != dark?.background, "\(theme)")
        }
    }
}
