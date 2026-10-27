import Foundation

/// Finding a skill by a few typed letters, for the picker a composer opens after the agent's
/// skill prefix.
public enum SkillSearch {
    /// The character that starts a skill in an agent's composer: `/` for Claude, `$` for Codex.
    /// Nil for an agent with no skill syntax wheel knows.
    public static func prefix(ofAgent name: String) -> Character? {
        switch name {
        case "claude": "/"
        case "codex": "$"
        default: nil
        }
    }

    /// The query when `draft` is a skill being typed: it starts with `prefix` and the rest holds
    /// no whitespace yet. Nil once the skill name is finished or the draft is something else.
    public static func query(in draft: String, prefix: Character) -> String? {
        guard draft.first == prefix else { return nil }
        let rest = draft.dropFirst()
        guard !rest.contains(where: \.isWhitespace) else { return nil }
        return String(rest)
    }

    /// The skills `query` matches, best first. Every query letter must appear in a skill's name
    /// in order. A name that starts with the query, then one that contains it, then a looser
    /// in-order match, rank first; a description containing the query matches last. An empty
    /// query lists every skill by name.
    public static func rank(_ query: String, in skills: [SkillInfo]) -> [SkillInfo] {
        let q = query.lowercased()
        if q.isEmpty { return skills.sorted { $0.name < $1.name } }
        var scored: [(skill: SkillInfo, score: Int)] = []
        for skill in skills {
            let name = skill.name.lowercased()
            if name.hasPrefix(q) {
                scored.append((skill, 0))
            } else if name.contains(q) {
                scored.append((skill, 1))
            } else if let gaps = subsequenceGaps(q, in: name) {
                scored.append((skill, 2 + gaps))
            } else if skill.description.lowercased().contains(q) {
                scored.append((skill, 1000))
            }
        }
        return scored.sorted { ($0.score, $0.skill.name) < ($1.score, $1.skill.name) }.map(\.skill)
    }

    /// How many name letters sit between the first and last matched letter beyond the query's own,
    /// when every query letter appears in order; nil when one does not.
    private static func subsequenceGaps(_ q: String, in name: String) -> Int? {
        var at = name.startIndex
        var first: String.Index?
        var last = name.startIndex
        for ch in q {
            guard let hit = name[at...].firstIndex(of: ch) else { return nil }
            if first == nil { first = hit }
            last = hit
            at = name.index(after: hit)
        }
        guard let first else { return 0 }
        return name.distance(from: first, to: last) + 1 - q.count
    }
}
