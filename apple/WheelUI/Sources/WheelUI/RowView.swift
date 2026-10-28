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
            VStack(alignment: .trailing, spacing: 2) {
                Text(text)
                    .font(ChatFont.body)
                    .selectableText(.plain(text), id: row.id, chat: chat, style: .body)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .background(Color.primary.opacity(queued ? 0.04 : 0.08), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .foregroundStyle(queued ? .secondary : .primary)
                if queued {
                    Text("Queued").font(ChatFont.meta).foregroundStyle(.secondary)
                }
            }
            .padding(.leading, 40)
            .frame(maxWidth: .infinity, alignment: .trailing)
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
                        .font(ChatFont.line)
                        .foregroundStyle(entry.status == "completed" ? .secondary : .primary)
                }
            }
        case let .mode(mode):
            Label("Mode: \(mode)", systemImage: "arrow.triangle.swap").font(ChatFont.meta).foregroundStyle(.secondary)
        case let .notice(text):
            Text(text).font(ChatFont.meta).foregroundStyle(.secondary)
        case let .error(text):
            Label(text, systemImage: "exclamationmark.triangle").font(ChatFont.line).foregroundStyle(.red)
        }
    }
}

/// A reply: the agent's Markdown at body size, with headings kept close to it so a `#` does not
/// shout over the conversation. The copy button sits under the last reply of a finished turn,
/// and under any other reply while the pointer is over it.
struct AgentText: View {
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
                .font(ChatFont.body)
                .textual.headingStyle(ChatHeadingStyle())
                .textual.inlineStyle(.chat)
                .frame(maxWidth: .infinity, alignment: .leading)
                .selectableText(.markdown(text), id: id, chat: chat, style: .body)
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
            Image(systemName: "doc.on.doc").font(ChatFont.meta)
        }
        .buttonStyle(.borderless)
        .foregroundStyle(.secondary)
        .help("Copy this reply")
    }
}

/// Headings one step above body at most: in a narrow card a 30pt `#` pushes the reply off screen.
struct ChatHeadingStyle: StructuredText.HeadingStyle {
    private static let fontScales: [CGFloat] = [1.25, 1.125, 1, 1, 1, 1]

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .textual.fontScale(Self.fontScales[min(configuration.headingLevel, 6) - 1])
            .textual.blockSpacing(.fontScaled(top: 1.2, bottom: 0.5))
            .fontWeight(.semibold)
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
