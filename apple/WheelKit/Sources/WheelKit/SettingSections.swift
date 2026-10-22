import Foundation

/// A box of settings that share a key prefix (`browser.` is Browser), as the Mac's Settings window
/// and the phone's Settings screen both group the daemon's list.
public struct SettingSection: Sendable, Equatable {
    public let name: String
    public var settings: [SettingInfo]

    /// Keys grouped by the segment before their first dot, sections in the order their first key
    /// arrived.
    public static func grouping(_ settings: [SettingInfo]) -> [SettingSection] {
        var out: [SettingSection] = []
        for setting in settings {
            let name = label(String(setting.key.split(separator: ".").first ?? Substring(setting.key)))
            if let i = out.firstIndex(where: { $0.name == name }) {
                out[i].settings.append(setting)
            } else {
                out.append(SettingSection(name: name, settings: [setting]))
            }
        }
        return out
    }

    /// A key segment as a label: `homepage` is Homepage, `font_size` is Font size.
    public static func label(_ segment: String) -> String {
        let words = segment.replacingOccurrences(of: "_", with: " ")
        return words.prefix(1).uppercased() + words.dropFirst()
    }

    /// A whole key's last segment as a label: `browser.font_size` is Font size.
    public static func rowLabel(_ key: String) -> String {
        label(String(key.split(separator: ".").last ?? Substring(key)))
    }
}
