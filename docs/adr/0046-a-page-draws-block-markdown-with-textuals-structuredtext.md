# ADR-0046: A page draws block Markdown with Textual's StructuredText

A page names `StructuredText{markdown, style?}` to draw a Markdown document: headings, lists, tables, code blocks, block quotes, thematic breaks and images. The client draws it with `StructuredText` from Textual (`gonzalezreal/textual`, the library WheelUI already links for the agent chat's transcript), under the name Textual gives it, as ADR-0031 has a page name SwiftUI's controls.

Checked first, under ADR-0031:

- SwiftUI's `Text` reads Markdown through `AttributedString(markdown:)`, which keeps inline syntax only (bold, italic, code, links, strikethrough); headings, lists, tables, quotes, code blocks and images come out as plain runs or are dropped. SwiftUI has no block Markdown view.
- The `Text` component's `markdown: true` is that same inline reading (`inlineOnlyPreservingWhitespace`), so it cannot carry a block.
- The A2UI library (`a2ui-swift`) has no Markdown component; its `Text` is plain text.
- Textual's `StructuredText` draws every block above from a Markdown string, styles each block through its own style protocols, and loads images through an attachment loader. It is the one view in the libraries WheelUI links that does the job.

`style` names a set of block styles and nothing else: `default` (Textual's own, when the page names none), `gitHub` (Textual's), or `chat` (the styles a reply in the agent chat draws with: `chatMarkdownStyle`, the one modifier the transcript and this component share, never a copy). The default is the library's, so the client adds no look the page did not ask for (ADR-0038); a `font` modifier on the component sets the size every block scales from. Images load from their URL with Textual's own loader. A `style` the client does not know draws the unsupported marker in place.

This adds no Wheel component: the client maps one more library component under its library name.
