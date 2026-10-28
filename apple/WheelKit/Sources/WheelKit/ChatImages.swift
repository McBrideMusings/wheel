import Foundation

/// Images in a reply: a Markdown image, or a line that is only a path or URL of an image file,
/// is drawn in the transcript when `chat.images.inline` is on, and stays a link when it is off.
public enum ChatImages {
    /// The file types a reply may show.
    public static let extensions: Set<String> = ["png", "jpg", "jpeg", "gif", "webp"]

    /// Whether `target` names an image file by its extension, ignoring a query or fragment.
    public static func isImageTarget(_ target: String) -> Bool {
        var path = target
        if let cut = path.firstIndex(where: { $0 == "?" || $0 == "#" }) { path = String(path[..<cut]) }
        guard let dot = path.lastIndex(of: "."), path[path.index(after: dot)...].allSatisfy({ $0.isLetter || $0.isNumber }) else { return false }
        return extensions.contains(path[path.index(after: dot)...].lowercased())
    }

    /// Whether `word` is a bare path or URL that can stand alone as an image: absolute, `~/`,
    /// `./` or a web address, with no space, ending in an image extension.
    static func isBareImage(_ word: String) -> Bool {
        guard !word.isEmpty, !word.contains(where: \.isWhitespace), isImageTarget(word) else { return false }
        return word.hasPrefix("/") || word.hasPrefix("~/") || word.hasPrefix("./")
            || word.hasPrefix("http://") || word.hasPrefix("https://")
    }

    /// `markdown` with every image written as Markdown image syntax (or, with `inline` off, as a
    /// link): a line that is only a bare image path or URL becomes `![](it)`; `![alt](it)` becomes
    /// `[alt](it)` when images are off. Fenced code and inline code are left alone.
    public static func rewrite(_ markdown: String, inline: Bool) -> String {
        var fence: (char: Character, length: Int)?
        var out: [String] = []
        for line in markdown.split(separator: "\n", omittingEmptySubsequences: false) {
            let text = String(line)
            let trimmed = text.drop(while: { $0 == " " })
            if let open = fence {
                out.append(text)
                let run = trimmed.prefix(while: { $0 == open.char }).count
                if run >= open.length, trimmed.dropFirst(run).allSatisfy({ $0 == " " }) { fence = nil }
                continue
            }
            if let c = trimmed.first, c == "`" || c == "~" {
                let run = trimmed.prefix(while: { $0 == c }).count
                if run >= 3 {
                    fence = (c, run)
                    out.append(text)
                    continue
                }
            }
            let bare = text.trimmingCharacters(in: .whitespaces)
            if inline, isBareImage(bare) {
                out.append("![](\(bare))")
            } else if !inline {
                out.append(unimaged(text))
            } else {
                out.append(text)
            }
        }
        return out.joined(separator: "\n")
    }

    /// `line` with each `![alt](target)` outside inline code as `[alt](target)`; a target that is
    /// a local file becomes a link that opens it the way a file path does.
    private static func unimaged(_ line: String) -> String {
        let chars = Array(line)
        var out = ""
        var i = 0
        while i < chars.count {
            if chars[i] == "`" {
                var run = 0
                while i + run < chars.count, chars[i + run] == "`" { run += 1 }
                if let close = closeOfCode(chars, from: i + run, run: run) {
                    out += String(chars[i..<(close + run)])
                    i = close + run
                } else {
                    out += String(chars[i..<(i + run)])
                    i += run
                }
            } else if chars[i] == "!", i + 1 < chars.count, chars[i + 1] == "[",
                      let closeAlt = chars[(i + 2)...].firstIndex(of: "]"),
                      closeAlt + 1 < chars.count, chars[closeAlt + 1] == "(",
                      let closeTarget = chars[(closeAlt + 2)...].firstIndex(of: ")") {
                let alt = String(chars[(i + 2)..<closeAlt])
                let target = String(chars[(closeAlt + 2)..<closeTarget])
                let web = target.hasPrefix("http://") || target.hasPrefix("https://")
                let address = web ? target : ChatLinkAction.url(file: target, line: nil)
                out += "[\(alt.isEmpty ? target : alt)](\(address))"
                i = closeTarget + 1
            } else {
                out.append(chars[i])
                i += 1
            }
        }
        return out
    }

    private static func closeOfCode(_ chars: [Character], from: Int, run: Int) -> Int? {
        var i = from
        while i < chars.count {
            if chars[i] == "`" {
                var n = 0
                while i + n < chars.count, chars[i + n] == "`" { n += 1 }
                if n == run { return i }
                i += n
            } else {
                i += 1
            }
        }
        return nil
    }
}
