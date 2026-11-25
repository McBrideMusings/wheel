import SwiftUI
import WheelKit
import WheelUI

/// The composer row's right side on the phone: how full the agent's context is, and a button named for its model
/// that opens the agent's settings sheet.
struct ModelButton: View {
    let model: PhoneModel
    let pane: String
    @ObservedObject var chat: AgentChat
    @State private var open = false

    var body: some View {
        HStack(spacing: 8) {
            if let context = chat.context {
                ContextRing(fraction: context.fraction, hot: context.percent > 80)
                    .frame(width: 16, height: 16)
                    .accessibilityLabel("Context used")
                    .accessibilityValue(context.shortText)
            }
            Button {
                model.requestPaneInfo(pane)
                open = true
            } label: {
                Text(Self.shortName(of: chat.config) ?? "Settings")
                    .font(.subheadline)
                    .lineLimit(1)
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Model")
            .accessibilityIdentifier("model menu")
            .accessibilityValue(Self.shortName(of: chat.config) ?? "")
        }
        .foregroundStyle(.secondary)
        .sheet(isPresented: $open) {
            AgentSettingsSheet(chat: chat, info: model.paneInfo[pane]) { model.setConfig(chat, $0, to: $1) }
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
    }

    /// The current model's name up to any parenthesis: "Sonnet 5.5 (1M context)" is "Sonnet 5.5".
    static func shortName(of config: [ConfigOption]) -> String? {
        guard let option = config.first(where: { $0.category == "model" }), case let .select(current, choices) = option.kind
        else { return nil }
        let name = choices.first { $0.value == current }?.name ?? current
        return name.components(separatedBy: " (").first ?? name
    }
}

/// Every setting the agent's harness offers, in the order it lists them, then the pane's info below a gap; scrolls
/// at half height and opens to full height.
private struct AgentSettingsSheet: View {
    @ObservedObject var chat: AgentChat
    let info: [InfoField]?
    let set: (ConfigOption, String) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        Form {
            if !chat.config.isEmpty {
                Section {
                    ForEach(chat.config) { option in row(option) }
                }
            }
            Section {
                PaneInfoView(fields: info)
            }
        }
        // The sheet is presented from the composer's row, whose secondary style it would otherwise inherit.
        .foregroundStyle(.primary)
        .safeAreaInset(edge: .bottom) {
            Button { dismiss() } label: {
                Text("Done").font(.headline).foregroundStyle(.white).frame(maxWidth: .infinity, minHeight: 44)
            }
            .buttonStyle(.borderedProminent)
            .buttonBorderShape(.capsule)
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            .background(.bar)
            .accessibilityIdentifier("settings done")
        }
    }

    /// The control the harness asked for. A pick shows what the agent took when its new list arrives, not what was tapped.
    @ViewBuilder private func row(_ option: ConfigOption) -> some View {
        switch option.kind {
        case let .select(current, choices) where choices.contains(where: { $0.value == current }):
            Picker(option.name, selection: Binding(get: { current }, set: { set(option, $0) })) {
                ForEach(choices) { Text($0.name).tag($0.value) }
            }
            .pickerStyle(.menu)
            .accessibilityIdentifier(option.category == "model" ? "model picker" : "config \(option.id)")
        case let .select(current, choices):
            // No choices, or a current value the list lacks: show it rather than a picker selecting nothing.
            LabeledContent(option.name, value: choices.first { $0.value == current }?.name ?? current)
        case let .boolean(current):
            Toggle(option.name, isOn: Binding(get: { current }, set: { set(option, $0 ? "true" : "false") }))
                .accessibilityIdentifier("config \(option.id)")
        case .unknown:
            LabeledContent(option.name, value: "Not shown on this iPhone")
        }
    }
}
