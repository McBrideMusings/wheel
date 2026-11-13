import SwiftUI

/// An agent's logo from the asset catalog (`AgentLogo-<name>`), a template drawn in the surrounding
/// foreground style (grey, never the vendor's colours); an SF Symbol for an agent Wheel has no logo for.
struct AgentLogo: View {
    let agent: String
    /// A colour in place of the grey: orange on an agent that needs the person.
    var tint: Color?

    static let logos: Set<String> = ["claude", "codex", "pi"]

    var body: some View {
        let style = tint.map(AnyShapeStyle.init) ?? AnyShapeStyle(.secondary)
        if Self.logos.contains(agent) {
            Image("AgentLogo-\(agent)").renderingMode(.template).resizable().scaledToFit()
                .foregroundStyle(style)
                .accessibilityLabel(agent)
        } else {
            Image(systemName: "bubble.left.and.text.bubble.right").resizable().scaledToFit()
                .foregroundStyle(style)
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
