import Foundation

/// Names for sessions nobody named: an adjective and a noun, like "tidal-thicket".
public enum SessionNames {
    static let adjectives = [
        "bold", "brisk", "calm", "clear", "cool", "crisp", "dusky", "eager", "faint", "gentle",
        "golden", "hazy", "hushed", "keen", "lucid", "mellow", "misty", "quiet", "rapid", "silver",
        "steady", "swift", "tidal", "warm", "wild",
    ]
    static let nouns = [
        "basin", "birch", "brook", "canyon", "cedar", "cove", "delta", "dune", "fjord", "glade",
        "harbor", "heath", "island", "lagoon", "meadow", "mesa", "ridge", "river", "shoal", "summit",
        "thicket", "tundra", "valley", "willow", "grove",
    ]

    /// A name not in `taken`; a number is added only when every pair is used.
    public static func next(avoiding taken: [String]) -> String {
        let used = Set(taken)
        let pairs = adjectives.flatMap { a in nouns.map { "\(a)-\($0)" } }.shuffled()
        if let free = pairs.first(where: { !used.contains($0) }) { return free }
        let base = pairs[0]
        return (2...).lazy.map { "\(base)-\($0)" }.first { !used.contains($0) }!
    }
}
