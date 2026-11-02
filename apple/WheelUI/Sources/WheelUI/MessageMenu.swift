import Foundation

/// What a right-click on a message offers: its two copies, then (after a separator) attaching it
/// to the next send. The same three on the person's messages and the agent's replies.
public enum MessageAction: String, CaseIterable, Sendable {
    case copyPlain = "copy"
    case copyMarkdown = "markdown"
    case attach = "attach"

    public var title: String {
        switch self {
        case .copyPlain: "Copy message"
        case .copyMarkdown: "Copy message as Markdown"
        case .attach: "Attach message as context"
        }
    }

    public var symbol: String {
        switch self {
        case .copyPlain: "doc.on.doc"
        case .copyMarkdown: "doc.richtext"
        case .attach: "paperclip"
        }
    }

    /// A separator sits before this item.
    public var separatedAbove: Bool { self == .attach }
}

extension AgentChat {
    /// A message's text as the person reads it, and as its Markdown source: the same string for the
    /// person's own message, which is typed as written. Nil for a row that is not a message.
    public func messageTexts(row id: Int) -> (plain: String, markdown: String)? {
        guard let item = items.first(where: { $0.id == id }), case let .row(row) = item else { return nil }
        switch row.row.kind.displayed {
        case let .user(text, _): return (text, text)
        case let .agent(text): return (RenderedProse.markdown(text).text, text)
        default: return nil
        }
    }

    /// Whether a right-click on this row opens the message menu.
    public func isMessage(row id: Int) -> Bool { messageTexts(row: id) != nil }

    /// Run one menu item on a message; false when the row is not a message.
    @discardableResult
    public func perform(_ action: MessageAction, onMessage id: Int) -> Bool {
        guard let texts = messageTexts(row: id) else { return false }
        UILog.info("transcript \(name): message menu \(action.rawValue) on row \(id)")
        switch action {
        case .copyPlain: copyToPasteboard(texts.plain)
        case .copyMarkdown: copyToPasteboard(texts.markdown)
        case .attach: attachAsContext(texts.markdown)
        }
        return true
    }

    /// Put a message at the top of the draft as a quoted block, so the next send carries it. The
    /// draft's own text stays under it.
    public func attachAsContext(_ markdown: String) {
        let quoted = markdown.split(separator: "\n", omittingEmptySubsequences: false)
            .map { $0.isEmpty ? ">" : "> \($0)" }
            .joined(separator: "\n")
        draft = draft.isEmpty ? quoted + "\n\n" : quoted + "\n\n" + draft
        composerFocusRequest += 1
    }
}
