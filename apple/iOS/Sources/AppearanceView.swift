import SwiftUI
import WheelKit

/// Settings › Appearance: pickers over the `terminal.*`, `chat.*` and `code.*` keys, writing the same
/// `wheel.toml` keys the Mac's Settings window writes, to this machine's file or the chosen project's.
struct AppearanceView: View {
    let model: PhoneModel
    let layer: SettingField.Layer
    @Environment(\.colorScheme) private var scheme

    private func setting(_ key: String) -> SettingInfo? { model.settings.first { $0.key == key } }

    /// What the shown file holds for `key`: its own value, else the layer below's.
    private func value(_ key: String) -> String {
        guard let setting = setting(key) else { return "" }
        let stored = layer == .global ? setting.machine : setting.project
        return stored ?? (layer == .global ? setting.default : setting.global)
    }

    private func write(_ key: String, _ text: String) {
        model.setSetting(SettingField(layer: layer, key: key), to: text)
    }

    private func binding(_ key: String) -> Binding<String> {
        Binding(get: { value(key) }, set: { write(key, $0) })
    }

    private func choices(_ key: String, fallback: [String]) -> [String] {
        let listed = setting(key)?.choices ?? []
        return listed.isEmpty ? fallback : listed
    }

    private func sizes(_ key: String, fallback: ClosedRange<Int>) -> [Int] {
        let low = setting(key)?.min.map(Int.init) ?? fallback.lowerBound
        let high = setting(key)?.max.map(Int.init) ?? fallback.upperBound
        return Array(low...high)
    }

    private var sizeBinding: (String) -> Binding<Int> {
        { key in Binding(get: { Int(value(key)) ?? 14 }, set: { write(key, String($0)) }) }
    }

    /// The terminal fonts the phone offers; blank is the terminal's own.
    private var terminalFonts: [String] {
        let current = value("terminal.font.family")
        let base = ["", "Menlo", "Courier New", "Courier"]
        return base.contains(current) ? base : base + [current]
    }

    private var thinkingShown: Binding<Bool> {
        Binding(get: { value("chat.thinking") != "hidden" }, set: { write("chat.thinking", $0 ? "collapsed" : "hidden") })
    }

    var body: some View {
        Form {
            Section("Terminal theme") {
                HStack(spacing: 12) {
                    ForEach(choices("terminal.theme", fallback: ["default", "solarized", "gruvbox", "github"]), id: \.self) { name in
                        Swatch(name: name, chosen: value("terminal.theme") == name, dark: scheme == .dark) { write("terminal.theme", name) }
                    }
                }
                .padding(.vertical, 4)
                Picker("Font", selection: binding("terminal.font.family")) {
                    ForEach(terminalFonts, id: \.self) { Text($0.isEmpty ? "SF Mono" : $0).tag($0) }
                }
                .accessibilityIdentifier("appearance-terminal-font")
                Picker("Size", selection: sizeBinding("terminal.font.size")) {
                    ForEach(sizes("terminal.font.size", fallback: 8...36), id: \.self) { Text("\($0) pt").tag($0) }
                }
                .accessibilityIdentifier("appearance-terminal-size")
                Picker("Cursor", selection: binding("terminal.cursor.style")) {
                    ForEach(choices("terminal.cursor.style", fallback: ["block", "bar", "underline"]), id: \.self) { Text($0.capitalized).tag($0) }
                }
                .accessibilityIdentifier("appearance-terminal-cursor")
            }
            Section("Chat") {
                Picker("Font", selection: binding("chat.font.family")) {
                    ForEach(choices("chat.font.family", fallback: ["system", "serif", "rounded", "mono"]), id: \.self) { Text($0.capitalized).tag($0) }
                }
                .accessibilityIdentifier("appearance-chat-font")
                Picker("Size", selection: sizeBinding("chat.font.size")) {
                    ForEach(sizes("chat.font.size", fallback: 10...22), id: \.self) { Text("\($0) pt").tag($0) }
                }
                .accessibilityIdentifier("appearance-chat-size")
                Picker("Density", selection: binding("chat.density")) {
                    ForEach(choices("chat.density", fallback: ["compact", "comfortable", "spacious"]), id: \.self) { Text($0.capitalized).tag($0) }
                }
                .accessibilityIdentifier("appearance-chat-density")
                Toggle("Show thinking", isOn: thinkingShown)
                    .accessibilityIdentifier("appearance-chat-thinking")
            }
            Section("Code theme") {
                ForEach(choices("code.theme", fallback: ["wheel-muted", "xcode", "github", "mono", "solarized"]), id: \.self) { name in
                    Button {
                        write("code.theme", name)
                    } label: {
                        HStack {
                            Text(name).foregroundStyle(.primary)
                            Spacer()
                            if value("code.theme") == name { Image(systemName: "checkmark").foregroundStyle(.tint) }
                        }
                    }
                    .accessibilityIdentifier("appearance-code-\(name)")
                }
            }
            Section {} footer: {
                Text(layer == .global
                     ? "Saved to \(setting("terminal.theme")?.machineFile ?? "~/.wheel/wheel.toml") on the Mac, so the Mac app and this phone match. A project's wheel.toml overrides it."
                     : "Saved to \(setting("terminal.theme")?.projectFile ?? "the project's wheel.toml") on the Mac. It overrides the machine's value here and on the Mac.")
            }
        }
        .navigationTitle("Appearance")
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// One terminal theme: the page and text colours of its palette, with a ring while chosen.
private struct Swatch: View {
    let name: String
    let chosen: Bool
    let dark: Bool
    let pick: () -> Void

    private var colours: (back: Color, fore: Color) {
        guard let theme = TerminalAppearance.Theme(rawValue: name), let palette = TerminalPalette.of(theme, dark: dark) else {
            return dark ? (Color(white: 0.11), .white) : (.white, .black)
        }
        return (Color(hex: palette.background), Color(hex: palette.foreground))
    }

    var body: some View {
        Button(action: pick) {
            VStack(spacing: 4) {
                Text("$ ls\nsrc/")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(colours.fore)
                    .frame(width: 56, height: 44)
                    .background(colours.back, in: RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(chosen ? Color.accentColor : Color.secondary.opacity(0.3), lineWidth: chosen ? 2 : 0.5))
                Text(name).font(.caption2).foregroundStyle(.secondary)
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(name) terminal theme")
        .accessibilityAddTraits(chosen ? .isSelected : [])
        .accessibilityIdentifier("appearance-theme-\(name)")
    }
}

private extension Color {
    /// `#rrggbb`.
    init(hex: String) {
        let digits = hex.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        let n = UInt32(digits, radix: 16) ?? 0
        self.init(red: Double((n >> 16) & 255) / 255, green: Double((n >> 8) & 255) / 255, blue: Double(n & 255) / 255)
    }
}
