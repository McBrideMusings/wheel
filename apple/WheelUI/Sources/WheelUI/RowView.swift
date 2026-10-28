import SwiftUI
import Textual
#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
import WheelKit

/// What a row draws that the row itself does not hold, as a value: the transcript, which observes
/// the model and the chat, hands it down, and a row redraws when it changes rather than for every
/// change to the model.
public struct RowContext: Equatable {
    /// This session's tool calls with a request waiting on a person.
    public var waiting: Set<String> = []
    /// Each waiting request's place in the queue by tool call, and how many wait in all.
    public var places: [String: Int] = [:]
    public var waitingCount = 0
    /// The session is working on a turn.
    public var busy = false
    /// Rows and groups the person opened, by id.
    public var expanded: Set<Int> = []

    public init(waiting: Set<String> = [], places: [String: Int] = [:], waitingCount: Int = 0, busy: Bool = false, expanded: Set<Int> = []) {
        self.waiting = waiting
        self.places = places
        self.waitingCount = waitingCount
        self.busy = busy
        self.expanded = expanded
    }

    func isWaiting(_ call: String?) -> Bool { call.map(waiting.contains) ?? false }
}

/// One laid-out entry: a row, or a group of tool calls.
struct ChatItemView: View {
    let chat: SessionChat
    let item: ChatItem
    /// The last part of a finished turn, which carries the turn's copy button.
    let copyable: Bool
    let context: RowContext

    @Environment(\.transcriptPlatform) private var platform

    var body: some View {
        content.background {
            if !item.isProse { platform.itemHighlight(item, chat) }
        }
    }

    @ViewBuilder private var content: some View {
        switch item {
        case let .row(row):
            TranscriptRowView(chat: chat, row: row, copyable: copyable, context: context)
        case let .tools(group):
            ToolGroupView(chat: chat, group: group, context: context)
        case let .work(group):
            ToolGroupView(turn: true, chat: chat, group: group, context: context)
        case let .edits(group):
            EditsCard(chat: chat, group: group, context: context)
        }
    }
}

/// One row, observing its own `RowModel` so a chunk streamed into it redraws it and no other row.
struct TranscriptRowView: View {
    let chat: SessionChat
    @ObservedObject var row: RowModel
    let copyable: Bool
    let context: RowContext

    var body: some View {
        let _ = row.drew()
        let current = row.row
        RowView(chat: chat, row: current, copyable: copyable, context: context)
    }
}

extension TranscriptRow.Kind {
    /// The kind a view keeps: reply and thought text trimmed into a new string. SwiftUI holds the
    /// last value it drew, so a view holding the transcript's own string would make the next
    /// appended chunk copy the whole reply.
    public var displayed: TranscriptRow.Kind {
        switch self {
        case let .agent(text): .agent(Self.trimmed(text))
        case let .thought(text): .thought(Self.trimmed(text))
        default: self
        }
    }

    private static func trimmed(_ text: String) -> String {
        let start = text.firstIndex { !$0.isWhitespace } ?? text.endIndex
        let end = text.lastIndex { !$0.isWhitespace }.map { text.index(after: $0) } ?? start
        return String(decoding: text.utf8[start..<end], as: UTF8.self)
    }
}

public struct RowView: View {
    @Environment(\.chatAppearance) private var look
    let chat: SessionChat
    let row: TranscriptRow
    let copyable: Bool
    let context: RowContext

    public init(chat: SessionChat, row: TranscriptRow, copyable: Bool, context: RowContext) {
        self.chat = chat
        self.row = row
        self.copyable = copyable
        self.context = context
    }

    public var body: some View {
        switch row.kind.displayed {
        case let .user(text, queued):
            UserMessage(chat: chat, row: row, text: text, queued: queued)
        case let .agent(text):
            AgentText(chat: chat, id: row.id, text: text, copyable: copyable)
        case let .thought(text):
            ReasoningLine(chat: chat, row: row, text: text, context: context)
        case let .tool(title, kind, status, content):
            ToolLine(chat: chat, row: row, title: title, kind: kind, status: status, content: content, context: context)
        case let .plan(entries):
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(entries.enumerated()), id: \.offset) { _, entry in
                    Label(entry.content, systemImage: entry.status == "completed" ? "checkmark.square" : "square")
                        .font(look.line)
                        .foregroundStyle(entry.status == "completed" ? .secondary : .primary)
                }
            }
        case let .mode(mode):
            Label("Mode: \(mode)", systemImage: "arrow.triangle.swap").font(look.meta).foregroundStyle(.secondary)
        case let .notice(text):
            Text(text).font(look.meta).foregroundStyle(.secondary)
        case let .error(text):
            Label(text, systemImage: "exclamationmark.triangle").font(look.line).foregroundStyle(.red)
        }
    }
}

/// The person's message, drawn by `chat.message.style` and `.align`, with its send time under it
/// when `chat.timestamps` is on.
struct UserMessage: View {
    @Environment(\.chatAppearance) private var look
    let chat: SessionChat
    let row: TranscriptRow
    let text: String
    let queued: Bool

    var body: some View {
        let right = look.messageAlign == .right
        VStack(alignment: right ? .trailing : .leading, spacing: 2) {
            if look.messageStyle == .plain {
                Text("You").font(look.meta).fontWeight(.semibold).foregroundStyle(.secondary)
            }
            message
            if queued {
                Text("Queued").font(look.meta).foregroundStyle(.secondary)
            } else if look.timestamps, let started = row.started {
                Text(started.formatted(date: .omitted, time: .shortened)).font(look.meta).foregroundStyle(.tertiary)
            }
        }
        .padding(right ? .leading : .trailing, 40)
        .frame(maxWidth: .infinity, alignment: right ? .trailing : .leading)
    }

    @ViewBuilder private var message: some View {
        let body = Text(text)
            .font(look.body)
            .lineSpacing(look.lineSpacing(for: look.size))
            .selectableText(.plain(text), id: row.id, chat: chat, style: look.bodyProse)
        switch look.messageStyle {
        case .bubble:
            body
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .background(Color.primary.opacity(queued ? 0.04 : 0.08), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .foregroundStyle(queued ? .secondary : .primary)
        case .accent:
            body
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .background(Color.accentColor.opacity(queued ? 0.5 : 1), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .foregroundStyle(.white)
        case .plain:
            body.foregroundStyle(queued ? .secondary : .primary)
        }
    }
}

/// A reply: the agent's Markdown at body size, with headings kept close to it so a `#` does not
/// shout over the conversation. The copy button sits under the last reply of a finished turn,
/// and under any other reply while the pointer is over it.
struct AgentText: View {
    @Environment(\.chatAppearance) private var look
    let chat: SessionChat
    let id: Int
    let text: String
    let copyable: Bool
    @State private var hovering = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            // A half-received fence still renders as text, which is what makes this safe to use on
            // a row that grows chunk by chunk.
            StructuredText(markdown: text)
                .font(look.body)
                .textual.paragraphStyle(ChatParagraphStyle(
                    lineSpacing: look.lineSpacing(for: look.size) / look.size,
                    spacing: look.paragraphSpacing))
                .textual.headingStyle(ChatHeadingStyle(spacing: look.paragraphSpacing))
                .textual.inlineStyle(.chat)
                .frame(maxWidth: .infinity, alignment: .leading)
                .selectableText(.markdown(text), id: id, chat: chat, style: look.bodyProse)
            // Only the end of a finished turn keeps a line for the button; an earlier reply shows it
            // over its own corner while the pointer is on it, so the parts of a turn stay close.
            if copyable { copy }
        }
        .overlay(alignment: .topTrailing) {
            if hovering && !copyable { copy.offset(y: -2) }
        }
        .onHover { hovering = $0 }
        #if !os(macOS)
        // No pointer to hover with: a long press on any reply copies it.
        .contextMenu { Button("Copy", systemImage: "doc.on.doc") { copyToPasteboard(text) } }
        #endif
    }

    private var copy: some View {
        Button {
            copyToPasteboard(text)
        } label: {
            Image(systemName: "doc.on.doc").font(look.meta)
        }
        .buttonStyle(.borderless)
        .foregroundStyle(.secondary)
        .help("Copy this reply")
    }
}

/// Headings one step above body at most: in a narrow card a 30pt `#` pushes the reply off screen.
struct ChatHeadingStyle: StructuredText.HeadingStyle {
    private static let fontScales: [CGFloat] = [1.25, 1.125, 1, 1, 1, 1]
    /// The paragraph spacing, in points; a heading sits 1.5 paragraphs below the block above it.
    let spacing: Double

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .textual.fontScale(Self.fontScales[min(configuration.headingLevel, 6) - 1])
            .textual.blockSpacing(StructuredText.BlockSpacing(top: spacing * 1.5, bottom: spacing * 0.5))
            .fontWeight(.semibold)
    }
}

/// A paragraph with the line height and the space above it that the `chat.*` settings give.
struct ChatParagraphStyle: StructuredText.ParagraphStyle {
    /// Extra space between lines, as a fraction of the font size.
    let lineSpacing: CGFloat
    /// Points above the paragraph.
    let spacing: Double

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .textual.lineSpacing(.fontScaled(lineSpacing))
            .textual.blockSpacing(StructuredText.BlockSpacing(top: spacing))
    }
}

extension InlineStyle {
    /// Inline code smaller than the text around it, on a faint fill, so a path in a sentence
    /// reads as code without outweighing the words.
    static var chat: InlineStyle {
        InlineStyle()
            .code(.monospaced, .fontScale(0.88), .backgroundColor(Color.primary.opacity(0.07)))
            .strong(.fontWeight(.semibold))
    }
}

/// Puts `text` on the system pasteboard.
func copyToPasteboard(_ text: String) {
    #if canImport(AppKit)
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(text, forType: .string)
    #else
    UIPasteboard.general.string = text
    #endif
}
