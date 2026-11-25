import Foundation

/// Whether a transcribed voice turn says nothing: a hum, a click Parakeet heard as a syllable, or
/// a nod while the reply is read. Voice mode neither sends such a turn nor stops a reply for it.
enum SpokenFiller {
    /// Sounds that carry no words.
    static let sounds: Set<String> = [
        "hm", "hmm", "hmmm", "mm", "mmm", "mhm", "mmhmm", "mm-hmm", "uh-huh", "uh", "uhh", "um", "umm",
        "uhm", "er", "erm", "ah", "ahh", "aah", "oh", "ooh", "eh", "huh", "ha", "hah",
    ]
    /// Nods that answer a question when the person is listening, but only acknowledge a reply being read.
    static let nods: Set<String> = ["yeah", "yep", "yes", "ok", "okay", "right", "sure", "cool", "nice", "uh-huh", "mhm"]

    /// True when every word of `text` is a sound, or, while a reply is spoken, a sound or a nod.
    static func isFiller(_ text: String, whileSpeaking: Bool) -> Bool {
        let words = text.lowercased()
            .split { !($0.isLetter || $0 == "-" || $0 == "'") }
            .map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "-'")) }
            .filter { !$0.isEmpty }
        return words.allSatisfy { sounds.contains($0) || (whileSpeaking && nods.contains($0)) }
    }
}
