import SwiftUI
import WheelKit

/// A popover over a reply: the lines of a file on the daemon's machine around the one the reply
/// named, that line highlighted, and a button that opens the file in a pane beside the chat.
struct FilePreview: View {
    @Environment(\.chatAppearance) private var look
    let preview: ChatLinkResolver.Preview
    let session: String
    let openInPane: () -> Void
    @Environment(\.chatLinks) private var links
    @State private var peek: FilePeek?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text(preview.path)
                    .font(look.meta)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.head)
                Spacer(minLength: 8)
                Button("Open in pane", action: openInPane)
                    .controlSize(.small)
            }
            content
        }
        .padding(12)
        .frame(minWidth: 320, idealWidth: 480, maxWidth: 560)
        .onAppear {
            links?.preview(path: preview.path, line: preview.line, session: session) { peek = $0 }
        }
    }

    @ViewBuilder private var content: some View {
        if let peek {
            if !peek.found {
                Text("No file at \(preview.path).").font(look.line).foregroundStyle(.secondary)
            } else if let error = peek.error {
                Text(error.prefix(1).uppercased() + error.dropFirst()).font(look.line).foregroundStyle(.secondary)
            } else {
                ScrollView([.horizontal, .vertical]) {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(peek.lines.enumerated()), id: \.offset) { offset, text in
                            row(number: peek.start + offset, text: text)
                        }
                    }
                }
                .frame(maxHeight: 280)
            }
        } else {
            ProgressView().controlSize(.small)
        }
    }

    private func row(number: Int, text: String) -> some View {
        let hit = number == preview.line
        return HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("\(number)")
                .foregroundStyle(.tertiary)
                .frame(minWidth: 32, alignment: .trailing)
            Text(text.isEmpty ? " " : text)
                .foregroundStyle(.primary)
                .fixedSize()
        }
        .font(look.codeFont(look.codeSize))
        .padding(.vertical, 1)
        .padding(.horizontal, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(hit ? Color.accentColor.opacity(0.22) : .clear)
        .accessibilityIdentifier(hit ? "file-preview-hit" : "file-preview-row")
    }
}
