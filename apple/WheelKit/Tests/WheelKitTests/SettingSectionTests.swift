import Testing
@testable import WheelKit

@Suite struct SettingSectionTests {
    private func info(_ key: String) -> SettingInfo {
        SettingInfo(key: key, value: "", origin: .default, default: "", help: "")
    }

    @Test func groupsByPrefixInArrivalOrder() {
        let sections = SettingSection.grouping([info("browser.homepage"), info("usage.pins"), info("browser.font_size")])
        #expect(sections.map(\.name) == ["Browser", "Usage"])
        #expect(sections[0].settings.map(\.key) == ["browser.homepage", "browser.font_size"])
    }

    @Test func labelsReplaceUnderscores() {
        #expect(SettingSection.rowLabel("browser.font_size") == "Font size")
    }
}
