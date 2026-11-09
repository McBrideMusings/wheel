import SwiftUI

/// The Desk mode's body: a placeholder until the agent cards are drawn.
struct DeskView: View {
    var body: some View {
        VStack(spacing: 6) {
            Image(systemName: "rectangle.3.group").font(.system(size: 28)).foregroundStyle(.tertiary)
            Text("Desk").font(.headline)
            Text("Every agent will be a card here.").foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("desk")
    }
}
