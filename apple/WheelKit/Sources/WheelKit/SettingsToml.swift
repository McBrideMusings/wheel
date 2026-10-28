import Foundation

/// The text of a `wheel.toml` holding some settings: what the Chat tab's "Show wheel.toml" prints.
public enum SettingsToml {
    /// One `[table]` per key prefix, in the order the keys arrive, a number or a switch written
    /// bare and any other value quoted, as the daemon writes the file. `entries` pairs a setting
    /// with the value the shown file holds for it.
    public static func render(_ entries: [(setting: SettingInfo, value: String)]) -> String {
        var tables: [(name: String, lines: [String])] = []
        for (setting, value) in entries {
            let parts = setting.key.split(separator: ".").map(String.init)
            let table = parts.dropLast().joined(separator: ".")
            let line = "\(parts.last ?? setting.key) = \(literal(value, kind: setting.kind))"
            if let i = tables.firstIndex(where: { $0.name == table }) {
                tables[i].lines.append(line)
            } else {
                tables.append((table, [line]))
            }
        }
        return tables.map { table in
            (table.name.isEmpty ? [] : ["[\(table.name)]"]) + table.lines
        }.map { $0.joined(separator: "\n") }.joined(separator: "\n\n")
    }

    private static func literal(_ value: String, kind: SettingKind) -> String {
        switch kind {
        case .int, .float, .bool: value
        case .choice where Int(value) != nil: value
        case .text, .choice:
            "\"" + value.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
        }
    }
}
