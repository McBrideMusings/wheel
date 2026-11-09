import Foundation

/// What read-aloud says for an agent's reply: the prose, with the markup a voice would read out
/// loud taken away.
public enum SpokenText {
    /// `markdown` as speech: fenced code blocks dropped, inline code and emphasis marks removed,
    /// links reduced to their text, headings and list marks removed, lines joined into sentences.
    public static func from(_ markdown: String) -> String {
        var kept: [Substring] = []
        var fenced = false
        for line in markdown.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                fenced.toggle()
                continue
            }
            if !fenced { kept.append(line) }
        }
        var text = kept.joined(separator: "\n")
        text = text.replacingOccurrences(of: #"!\[([^\]]*)\]\([^)]*\)"#, with: "$1", options: .regularExpression)
        text = text.replacingOccurrences(of: #"\[([^\]]*)\]\([^)]*\)"#, with: "$1", options: .regularExpression)
        text = text.replacingOccurrences(of: #"(?m)^\s{0,3}#{1,6}\s+"#, with: "", options: .regularExpression)
        text = text.replacingOccurrences(of: #"(?m)^\s*(?:[-*+]|\d+[.)])\s+"#, with: "", options: .regularExpression)
        text = text.replacingOccurrences(of: #"(?m)^\s*>\s?"#, with: "", options: .regularExpression)
        text = text.replacingOccurrences(of: #"https?://\S+"#, with: "a link", options: .regularExpression)
        // Emphasis and code marks go; an underscore inside a name (read_aloud) stays.
        text = text.replacingOccurrences(of: #"[`*~]+|(?<![A-Za-z0-9])_+|_+(?![A-Za-z0-9])"#, with: "", options: .regularExpression)
        let lines = text.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        return lines.map { line in
            guard let last = line.last, !".!?:;".contains(last) else { return line }
            return line + "."
        }.joined(separator: " ")
    }
}

extension Transcript {
    /// The agent's reply to the last message the person sent: its text rows after that message,
    /// in order. Empty when the agent has said nothing since.
    public var lastReply: String {
        let start = rows.lastIndex { row in
            if case .user(_, queued: false) = row.kind { true } else { false }
        }.map { $0 + 1 } ?? 0
        return rows[start...].compactMap { row in
            if case let .agent(text) = row.kind { text } else { nil }
        }.joined(separator: "\n\n")
    }
}
