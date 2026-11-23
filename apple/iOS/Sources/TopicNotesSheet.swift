import SwiftUI
import Textual

/// The orchestrator's current topic (ADR-0054): `threads/current` under its folder names it, and `threads/<id>.md`
/// holds its notes, front matter (`title:`, `session:`) then a Markdown body, as the folder's `tools/note` writes them.
struct OrchestratorTopic: Equatable {
    let id: String
    let title: String
    let notes: String

    /// The topic `id` from its notes file's lines; a file with no `title:` is titled by its id.
    init(id: String, lines: [String]) {
        self.id = id
        var body = lines[...]
        var title: String?
        if body.first == "---", let end = body.dropFirst().firstIndex(of: "---") {
            for line in body[body.startIndex + 1 ..< end] {
                let parts = line.split(separator: ":", maxSplits: 1)
                if parts.count == 2, parts[0].trimmingCharacters(in: .whitespaces) == "title" {
                    title = parts[1].trimmingCharacters(in: .whitespaces)
                }
            }
            body = body[(end + 1)...]
        }
        self.title = title.flatMap { $0.isEmpty ? nil : $0 } ?? id
        notes = body.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// The id `threads/current` holds, when it is one `tools/switch` would write: letters, digits, `-`, `.` and `_`.
    static func currentID(_ lines: [String]) -> String? {
        let id = lines.first?.trimmingCharacters(in: .whitespaces) ?? ""
        guard !id.isEmpty, id != ".", id != "..", id.allSatisfy({ $0.isLetter || $0.isNumber || "-._".contains($0) }) else { return nil }
        return id
    }
}

/// The context line under Orchestrator mode's title: the current topic's title; a tap opens its notes.
struct TopicLine: View {
    let topic: OrchestratorTopic
    @State private var open = false

    var body: some View {
        Button {
            Log.info("topic notes: opened \(topic.id)")
            open = true
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "doc.text").font(.caption)
                Text(topic.title).lineLimit(1).truncationMode(.tail)
                Image(systemName: "chevron.right").font(.caption2.weight(.semibold))
            }
            .font(.footnote)
            .frame(minHeight: 22)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(.tint)
        .accessibilityIdentifier("topic")
        .accessibilityLabel(topic.title)
        .sheet(isPresented: $open) {
            TopicNotesSheet(topic: topic)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
    }
}

/// M8: the current topic's notes, read-only, drawn by the Markdown renderer the chat uses.
struct TopicNotesSheet: View {
    let topic: OrchestratorTopic

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                Text("Topic notes").font(.footnote).foregroundStyle(.secondary)
                Text(topic.title).font(.title3.weight(.bold)).accessibilityIdentifier("topic notes title")
                if topic.notes.isEmpty {
                    Text("No notes yet").font(.subheadline).foregroundStyle(.secondary)
                } else {
                    StructuredText(markdown: topic.notes)
                        .font(.subheadline)
                        .textual.headingStyle(NotesHeadingStyle())
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 20)
            .padding(.bottom, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityIdentifier("topic notes")
    }
}

/// A notes heading labels its section, as M8 draws them: body size, semibold, secondary, so the topic's title stays
/// the largest text on the sheet.
private struct NotesHeadingStyle: StructuredText.HeadingStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .textual.fontScale(configuration.headingLevel == 1 ? 1.125 : 1)
            .textual.blockSpacing(StructuredText.BlockSpacing(top: 14, bottom: 4))
            .fontWeight(.semibold)
            .foregroundStyle(.secondary)
    }
}
