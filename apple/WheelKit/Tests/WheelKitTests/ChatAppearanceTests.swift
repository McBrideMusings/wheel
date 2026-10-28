import Foundation
import Testing
@testable import WheelKit

@Suite struct ChatAppearanceTests {
    @Test func readsResolvedValuesAndTheirKinds() throws {
        let json = """
        [{"key":"chat.font.size","value":"17","origin":"project","default":"13","help":"h","kind":"int","min":10,"max":22},
         {"key":"chat.density","value":"compact","origin":"machine","default":"comfortable","help":"h","kind":"choice","choices":["compact","comfortable","spacious"]},
         {"key":"chat.max_width","value":"680","origin":"machine","default":"none","help":"h","kind":"choice","choices":["none","680"]},
         {"key":"chat.font.family","value":"serif","origin":"default","default":"system","help":"h","kind":"choice"}]
        """
        let list = try JSONDecoder().decode([SettingInfo].self, from: Data(json.utf8))
        #expect(list[0].kind == .int && list[0].min == 10 && list[0].max == 22)
        #expect(list[1].choices == ["compact", "comfortable", "spacious"])
        let look = ChatAppearance(settings: list)
        #expect(look.size == 17)
        #expect(look.density == .compact && look.turnGap == 14)
        #expect(look.maxWidth == 680)
        #expect(look.family == .serif)
        #expect(look.lineHeight == 1.45)
    }

    @Test func noneMeansFullWidthAndAnEmptyListMeansTheDefaults() {
        let none = SettingInfo(key: "chat.max_width", value: "none", origin: .default, default: "none", help: "")
        #expect(ChatAppearance(settings: [none]).maxWidth == nil)
        #expect(ChatAppearance(settings: []) == .standard)
    }

    @Test func readsTheQuoteAndCodeKeys() {
        func set(_ key: String, _ value: String) -> SettingInfo {
            SettingInfo(key: key, value: value, origin: .machine, default: "", help: "")
        }
        let look = ChatAppearance(settings: [
            set("chat.quote.style", "italic"), set("code.theme", "solarized"), set("code.font", "courier-new"),
            set("code.size", "15"), set("code.wrap", "wrap"), set("code.line_numbers", "true"),
        ])
        #expect(look.quoteStyle == .italic && look.codeTheme == .solarized && look.codeFont == .courierNew)
        #expect(look.codeSize == 15 && look.codeWrap == .wrap && look.lineNumbers)
        #expect(ChatAppearance.standard.quoteStyle == .bar && ChatAppearance.standard.codeTheme == .wheelMuted)
    }
}

@Suite struct SettingsTomlTests {
    @Test func groupsKeysIntoTablesAndQuotesOnlyWords() {
        func info(_ key: String, _ kind: SettingKind) -> SettingInfo {
            SettingInfo(key: key, value: "", origin: .default, default: "", help: "", kind: kind)
        }
        let text = SettingsToml.render([
            (info("chat.font.family", .choice), "serif"),
            (info("chat.font.size", .int), "17"),
            (info("chat.timestamps", .bool), "true"),
            (info("code.wrap", .choice), "wrap"),
        ])
        #expect(text == "[chat.font]\nfamily = \"serif\"\nsize = 17\n\n[chat]\ntimestamps = true\n\n[code]\nwrap = \"wrap\"")
    }
}
