import SwiftUI

/// An agent's logo from the asset catalog (`AgentLogo-<name>`), a template drawn in the surrounding
/// foreground style (grey, never the vendor's colours); an SF Symbol for an agent Wheel has no logo for.
struct AgentLogo: View {
    let agent: String

    static let logos: Set<String> = ["claude", "codex", "pi"]

    var body: some View {
        if Self.logos.contains(agent) {
            Image("AgentLogo-\(agent)").renderingMode(.template).resizable().scaledToFit()
                .foregroundStyle(.secondary)
                .accessibilityLabel(agent)
        } else {
            Image(systemName: "bubble.left.and.text.bubble.right").resizable().scaledToFit()
                .foregroundStyle(.secondary)
                .accessibilityLabel(agent)
        }
    }

    /// A menu row titled `title` with `agent`'s logo.
    @ViewBuilder static func label(_ title: String, agent: String) -> some View {
        if logos.contains(agent) {
            Label(title, image: "AgentLogo-\(agent)")
        } else {
            Label(title, systemImage: "bubble.left.and.text.bubble.right")
        }
    }
}
