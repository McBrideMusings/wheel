import SwiftUI
import WheelKit
import WheelUI

/// The agent's model as a small menu, its current name with a chevron, for the composer's row; nothing when
/// the agent offers no model choice. A pick checks the model the agent took, not the one tapped.
struct ModelMenu: View {
    @ObservedObject var chat: AgentChat
    let setModel: (ConfigOption, String) -> Void

    var body: some View {
        if let option = chat.config.first(where: { $0.category == "model" }), case let .select(current, choices) = option.kind {
            let name = choices.first { $0.value == current }?.name ?? current
            Menu {
                ForEach(choices) { choice in
                    Button { setModel(option, choice.value) } label: {
                        if choice.value == current { Label(choice.name, systemImage: "checkmark") } else { Text(choice.name) }
                    }
                    .accessibilityIdentifier("model \(choice.value)")
                }
            } label: {
                HStack(spacing: 3) {
                    Text(name).lineLimit(1)
                    Image(systemName: "chevron.down").font(.caption2.weight(.semibold))
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                .contentShape(Rectangle())
            }
            .accessibilityLabel("Model")
            .accessibilityIdentifier("model menu")
            .accessibilityValue(name)
        }
    }
}
