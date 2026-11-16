import SwiftUI
import Textual

/// `StructuredText{markdown, style?}`: Textual's `StructuredText` drawing block Markdown (headings, lists, tables, code
/// blocks, quotes, thematic breaks, images) under the name Textual gives it (ADR-0046). `style` names a set of block styles:
/// `default` (Textual's, when the page names none), `gitHub` (Textual's), or `chat` (the styles a reply in the agent chat
/// draws with, from the chat appearance in the environment). Images load from their URL with Textual's own loader. The font
/// is the page's, so a `font` modifier on the component sets the body size every block scales from.
struct A2UIStructuredTextView: View {
    let node: ComponentNode
    @Environment(\.chatAppearance) private var look
    var body: some View {
        let markdown = node.props["markdown"]?.text ?? ""
        let text = StructuredText(markdown: markdown)
        switch node.props["style"]?.stringValue ?? "default" {
        case "default": text
        case "gitHub": text.textual.structuredTextStyle(.gitHub)
        case "chat": text.chatMarkdownStyle(look, agent: "a2ui:\(node.instance.id)")
        case let other:
            A2UIUnsupportedView(node: ComponentNode(instance: ComponentInstance(id: node.instance.id, type: "Unsupported",
                properties: ["name": .string("StructuredText style \(other)")]), children: []))
        }
    }
}
