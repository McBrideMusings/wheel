import SwiftUI
import WheelKit
import WheelUI

/// The Chat tab: every `chat.*` and `code.*` key the daemon lists, drawn from its schema — a
/// control by kind, the key's description, where the shown file sets it — over a Machine or a
/// Project file. A change goes to the daemon, which redraws every open chat.
struct ChatSettings: View {
    @ObservedObject var model: AppModel
    @ObservedObject var settings: SettingsModel
    private var layer: SettingLayer { settings.chatLayer }
    private var query: String { settings.chatQuery }
    private var matches: [SettingInfo] { settings.chatRows() }

    private var projectName: String {
        model.spaces.first { $0.id == settings.project }?.name ?? "none"
    }

    /// `chat.font.family` is Font family; `code.size` is Code size.
    static func title(_ setting: SettingInfo) -> String {
        let parts = setting.key.split(separator: ".").map(String.init)
        let words = parts.first == "code" ? parts : Array(parts.dropFirst())
        return SettingSection.label(words.joined(separator: " "))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                TextField("Search settings", text: $settings.chatQuery)
                    .textFieldStyle(.roundedBorder)
                Picker("Layer", selection: $settings.chatLayer) {
                    Text("Machine").tag(SettingLayer.global)
                    Text("Project · \(projectName)").tag(SettingLayer.project)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                .disabled(settings.project == nil)
            }
            if matches.isEmpty {
                Text("No setting matches “\(query)”.")
                    .font(ChatFont.body).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity).padding(.vertical, 30)
            } else {
                VStack(spacing: 0) {
                    ForEach(Array(matches.enumerated()), id: \.element.key) { index, setting in
                        if index > 0 { Divider() }
                        ChatSettingRow(model: model, settings: settings, setting: setting, field: settings.field(layer, setting.key))
                            .id(setting.key)
                    }
                }
                .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
            }
            footer
            if settings.chatShowToml { tomlView }
        }
        .fixedSize(horizontal: false, vertical: true)
    }

    private var changed: [SettingInfo] {
        settings.settings(in: layer, topic: .chat).filter { settings.stored(settings.field(layer, $0.key)) != nil }
    }

    private var footer: some View {
        HStack(spacing: 12) {
            Text("\(changed.count) changed from the defaults").font(ChatFont.line).foregroundStyle(.secondary)
            Spacer()
            Toggle("Show wheel.toml", isOn: $settings.chatShowToml).toggleStyle(.checkbox)
            Button("Reset all") {
                for setting in changed { model.clearSetting(settings.field(layer, setting.key)) }
            }
            .disabled(changed.isEmpty)
        }
    }

    /// Every key the shown file holds, as the file reads.
    private var tomlView: some View {
        let text = settings.chatToml()
        return Text(text)
            .font(.system(.callout, design: .monospaced))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.separator, lineWidth: 0.5))
            .accessibilityIdentifier("chat-settings-toml")
    }
}

/// One key: its title and description, the control for its kind, and under the title where the
/// shown file sets it, with a reset that removes it from that file.
private struct ChatSettingRow: View {
    @ObservedObject var model: AppModel
    @ObservedObject var settings: SettingsModel
    let setting: SettingInfo
    let field: SettingsModel.Field

    /// What the shown file sets; nil when it sets nothing.
    private var stored: String? { settings.stored(field) }
    /// What the shown file shows: its own value, else the layer below's.
    private var value: String { stored ?? settings.inherited(field) }
    private var layerName: String { field.layer == .global ? "this machine" : "this project" }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(ChatSettings.title(setting)).font(ChatFont.body).fontWeight(.medium)
                    Text(setting.help).font(ChatFont.line).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                control
                Button { model.clearSetting(field) } label: { Image(systemName: "arrow.uturn.backward") }
                    .buttonStyle(.borderless)
                    .help("Reset to what the layer below gives")
                    .opacity(stored == nil ? 0 : 1)
                    .disabled(stored == nil)
                    .accessibilityLabel("Reset \(setting.key)")
            }
            if stored != nil {
                Text("Set in \(layerName)").font(ChatFont.meta).foregroundStyle(.secondary)
            }
            if field.layer == .global, setting.project != nil {
                Text("The project sets this too, and its value wins.").font(ChatFont.meta).foregroundStyle(.orange)
            }
            if let problem = setting.problem {
                Label(problem, systemImage: "exclamationmark.triangle").font(ChatFont.meta).foregroundStyle(.orange)
            }
            if let refusal = settings.refusals[field] {
                Label(refusal, systemImage: "xmark.circle").font(ChatFont.meta).foregroundStyle(.red)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
    }

    private func write(_ text: String) {
        guard text != value || stored == nil else { return }
        model.writeSetting(field, text)
    }

    @ViewBuilder private var control: some View {
        switch setting.kind {
        case .bool:
            Toggle("", isOn: Binding(get: { value == "true" }, set: { write($0 ? "true" : "false") }))
                .toggleStyle(.switch).labelsHidden()
        case .choice:
            Picker("", selection: Binding(get: { value }, set: { write($0) })) {
                ForEach(setting.choices, id: \.self) { Text($0).tag($0) }
            }
            .labelsHidden().fixedSize()
        case .int, .float:
            NumberField(value: value, minimum: setting.min, maximum: setting.max, whole: setting.kind == .int, commit: write)
        case .text:
            TextField("", text: Binding(get: { settings.text(field).isEmpty ? value : settings.text(field) }, set: { settings.type(field, $0) }))
                .onSubmit { model.saveSetting(field) }
                .frame(width: 180)
        }
    }
}

/// A number typed and committed with Return, with a stepper; the daemon holds the bounds, which
/// the stepper keeps to and a typed value is checked against by the daemon.
private struct NumberField: View {
    let value: String
    let minimum: Double?
    let maximum: Double?
    let whole: Bool
    let commit: (String) -> Void
    @State private var draft = ""
    @FocusState private var focused: Bool

    private var number: Double { Double(value) ?? minimum ?? 0 }
    private var step: Double { whole ? 1 : 0.05 }

    private func text(_ n: Double) -> String {
        whole ? String(Int(n.rounded())) : String(format: "%g", (n * 100).rounded() / 100)
    }

    var body: some View {
        HStack(spacing: 4) {
            TextField("", text: $draft)
                .multilineTextAlignment(.trailing)
                .frame(width: 56)
                .focused($focused)
                .onSubmit { commit(draft) }
                .onAppear { draft = value }
                .onChange(of: value) { _, new in if !focused { draft = new } }
            Stepper("", onIncrement: { bump(step) }, onDecrement: { bump(-step) })
                .labelsHidden()
        }
    }

    private func bump(_ delta: Double) {
        var n = number + delta
        if let minimum { n = Swift.max(n, minimum) }
        if let maximum { n = Swift.min(n, maximum) }
        draft = text(n)
        commit(draft)
    }
}

extension SettingsModel {
    /// The Chat tab's rows: the `chat.*` and `code.*` keys the shown layer takes whose title,
    /// description or key contains the search text.
    func chatRows() -> [SettingInfo] {
        let shown = settings(in: chatLayer, topic: .chat)
        let q = chatQuery.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return shown }
        return shown.filter { ChatSettings.title($0).lowercased().contains(q) || $0.help.lowercased().contains(q) || $0.key.lowercased().contains(q) }
    }

    /// What "Show wheel.toml" prints: every key the shown layer's file sets, as the file reads.
    func chatToml() -> String {
        let entries = settings.compactMap { setting -> (setting: SettingInfo, value: String)? in
            guard let value = stored(field(chatLayer, setting.key)) else { return nil }
            return (setting, value)
        }
        let text = SettingsToml.render(entries)
        return text.isEmpty ? "# this file sets nothing" : text
    }
}
