import Foundation

/// The `<wheel-inbox>` block wheeld attaches after the person's words when it sends an orchestrator their message
/// (`daemon/src/inbox.rs`, `attach`). The chat shows only what the person typed; the events show in Tasks.
public enum InboxBlock {
    /// `text` with the attached block taken out; a message with no block comes back as it is.
    public static func words(_ text: String) -> String {
        guard let open = text.range(of: "<wheel-inbox"), let close = text.range(of: "</wheel-inbox>", range: open.upperBound..<text.endIndex)
        else { return text }
        return (text[..<open.lowerBound] + text[close.upperBound...]).trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
