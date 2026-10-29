import Foundation
import SwiftUI
import Textual

/// What a reply draws beyond plain Markdown: typeset maths, a swatch beside a hex colour in
/// inline code, and a local-time popover on an ISO 8601 timestamp. `ChatRich.prepare` rewrites a
/// reply's Markdown into what Textual draws and reports what it found, so the debug `/state`
/// reads back the same facts the screen shows.
enum ChatRich {
    struct Math: Equatable { let latex: String; let display: Bool }
    struct Time: Equatable { let iso: String; let local: String }

    struct Prepared {
        var markdown: String
        /// The swatch pseudo-emoji the rewrite added; Textual's `.emoji` extension turns each into a swatch.
        var emoji: Set<Emoji> = []
        var syntax: [AttributedStringMarkdownParser.SyntaxExtension] = []
        var math: [Math] = []
        var swatches: [String] = []
        var times: [Time] = []
    }

    /// The URL scheme of a timestamp link, and the prefix of a swatch's shortcode and URL.
    static let timeScheme = "wheel-time"
    static let swatchScheme = "wheel-swatch"
    private static let swatchPrefix = "swatch_"

    nonisolated(unsafe) private static let hexCode = /^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/
    nonisolated(unsafe) private static let isoStamp = /\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})/
    nonisolated(unsafe) private static let codeSpan = /`+[^`\n]+`+/
    nonisolated(unsafe) private static let linkDestination = /\[[^\]\n]*\]\([^)\n]*\)|<[a-z]+:[^>\s]*>|https?:\/\/[^\s)>]+/
    /// Pandoc's rule for `$…$`: no space just inside either `$`, and no digit right after the
    /// closing one, so "costs $5 and $10" stays money.
    nonisolated(unsafe) private static let inlineMath = /\$(?!\$)(?!\s)((?:\\\$|[^$\n])*?[^$\s\n\\])\$(?![0-9$])/
    nonisolated(unsafe) private static let blockMath = /(?s)\$\$(.+?)\$\$/
    nonisolated(unsafe) private static let mathFence = /\$\$/

    static func prepare(_ markdown: String, now: Date = .now, zone: TimeZone = .current) -> Prepared {
        var out: [String] = []
        var prepared = Prepared(markdown: "")
        prepared.math = maths(in: markdown)
        var fence: Substring?
        var inBlockMath = false
        for line in markdown.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.drop(while: { $0 == " " })
            if let open = fence {
                if trimmed.hasPrefix(open) { fence = nil }
                out.append(String(line))
                continue
            }
            // A `$$` block that spans lines is LaTeX to the last line; nothing in it is prose.
            let fences = line.matches(of: mathFence).count
            if inBlockMath || fences % 2 == 1 {
                if fences % 2 == 1 { inBlockMath.toggle() }
                out.append(String(line))
                continue
            }
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                fence = trimmed.prefix(3)
                out.append(String(line))
                continue
            }
            out.append(rewrite(String(line), into: &prepared, now: now, zone: zone))
        }
        prepared.markdown = out.joined(separator: "\n")
        if !prepared.math.isEmpty { prepared.syntax.append(.math) }
        if !prepared.emoji.isEmpty { prepared.syntax.append(.emoji(prepared.emoji)) }
        return prepared
    }

    /// One prose line: hex-colour code spans get a swatch, timestamps outside code and link
    /// destinations become links.
    private static func rewrite(_ line: String, into prepared: inout Prepared, now: Date, zone: TimeZone) -> String {
        var result = ""
        var rest = line[...]
        while !rest.isEmpty {
            let code = rest.firstMatch(of: codeSpan)
            let destination = rest.firstMatch(of: linkDestination)
            let next = [code?.range, destination?.range].compactMap { $0 }.min { $0.lowerBound < $1.lowerBound }
            guard let next else {
                result += stamped(String(rest), into: &prepared, now: now, zone: zone)
                break
            }
            result += stamped(String(rest[..<next.lowerBound]), into: &prepared, now: now, zone: zone)
            let protected = String(rest[next])
            result += protected
            if let code, code.range == next {
                let body = protected.trimmingCharacters(in: CharacterSet(charactersIn: "`"))
                if let hex = body.wholeMatch(of: hexCode)?.1 {
                    let name = String(hex).lowercased()
                    prepared.swatches.append("#" + name)
                    prepared.emoji.insert(Emoji(shortcode: swatchPrefix + name, url: URL(string: "\(swatchScheme):\(name)")!))
                    result += " :\(swatchPrefix)\(name):"
                }
            }
            rest = rest[next.upperBound...]
        }
        return result
    }

    /// Prose with its timestamps linked. When the reply holds maths, a `$` outside a maths span
    /// (a price) becomes a fullwidth `＄`, because Textual's own maths pattern would pair it with
    /// the next `$` and typeset what lies between.
    private static func stamped(_ text: String, into prepared: inout Prepared, now: Date, zone: TimeZone) -> String {
        guard !prepared.math.isEmpty, text.contains("$") else {
            return linkedTimes(text, into: &prepared, now: now, zone: zone)
        }
        var result = ""
        var rest = text[...]
        while let m = [rest.firstMatch(of: blockMath)?.range, rest.firstMatch(of: inlineMath)?.range]
            .compactMap({ $0 }).min(by: { $0.lowerBound < $1.lowerBound }) {
            result += linkedTimes(String(rest[..<m.lowerBound]), into: &prepared, now: now, zone: zone)
                .replacingOccurrences(of: "$", with: "\u{FF04}")
            result += rest[m]
            rest = rest[m.upperBound...]
        }
        return result + linkedTimes(String(rest), into: &prepared, now: now, zone: zone).replacingOccurrences(of: "$", with: "\u{FF04}")
    }

    private static func linkedTimes(_ text: String, into prepared: inout Prepared, now: Date, zone: TimeZone) -> String {
        var result = ""
        var rest = text[...]
        while let match = rest.firstMatch(of: isoStamp) {
            let iso = String(rest[match.range])
            result += rest[..<match.range.lowerBound]
            if let local = localTime(iso, now: now, zone: zone) {
                prepared.times.append(Time(iso: iso, local: local))
                result += "[\(iso)](\(timeScheme):\(iso.replacingOccurrences(of: "+", with: "%2B")))"
            } else {
                result += iso
            }
            rest = rest[match.range.upperBound...]
        }
        return result + rest
    }

    /// Every `$$…$$` and `$…$` in the prose of `markdown`, in order.
    static func maths(in markdown: String) -> [Math] {
        var found: [Math] = []
        var fence: Substring?
        var prose: [String] = []
        for line in markdown.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.drop(while: { $0 == " " })
            if let open = fence {
                if trimmed.hasPrefix(open) { fence = nil }
                continue
            }
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                fence = trimmed.prefix(3)
                continue
            }
            prose.append(String(line).replacing(codeSpan, with: ""))
        }
        var text = prose.joined(separator: "\n")
        for match in text.matches(of: blockMath) { found.append(Math(latex: String(match.1), display: true)) }
        text = text.replacing(blockMath, with: "")
        for match in text.matches(of: inlineMath) { found.append(Math(latex: String(match.1), display: false)) }
        return found
    }

    /// `12:04pm EDT` for 16:04:58Z in New York; the date goes first when it is not today.
    static func localTime(_ iso: String, now: Date = .now, zone: TimeZone = .current) -> String? {
        // The formatter wants seconds and a `+hh:mm` zone; ISO 8601 allows both to be shorter.
        guard let parts = iso.wholeMatch(of: /(\d{4}-\d{2}-\d{2}T\d{2}:\d{2})(:\d{2})?(\.\d+)?(Z|[+-]\d{2}(?::?\d{2})?)/) else { return nil }
        var offset = String(parts.4)
        if offset != "Z" {
            let digits = offset.dropFirst().filter(\.isNumber)
            offset = String(offset.first!) + digits.prefix(2) + ":" + (digits.count > 2 ? digits.suffix(2) : "00")
        }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = parts.3 == nil ? [.withInternetDateTime] : [.withInternetDateTime, .withFractionalSeconds]
        let date = formatter.date(from: "\(parts.1)\(parts.2 ?? ":00")\(parts.3 ?? "")\(offset)")
        guard let date else { return nil }
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = zone
        let clock = DateFormatter()
        clock.locale = Locale(identifier: "en_US_POSIX")
        clock.timeZone = zone
        clock.amSymbol = "am"
        clock.pmSymbol = "pm"
        clock.dateFormat = calendar.isDate(date, inSameDayAs: now) ? "h:mma zzz" : "MMM d, h:mma zzz"
        return clock.string(from: date)
    }

    /// The ISO text a timestamp link carries.
    static func timestamp(of url: URL) -> String? {
        guard url.scheme == timeScheme else { return nil }
        return url.absoluteString.dropFirst(timeScheme.count + 1).removingPercentEncoding
    }
}

/// What the replies of a chat draw, for the debug `/state`.
public enum ChatRichReport {
    public static func report(of replies: [String]) -> [String: Any] {
        let prepared = replies.map { ChatRich.prepare($0) }
        return [
            "math": prepared.flatMap(\.math).map { ["latex": $0.latex, "display": $0.display] as [String: Any] },
            "swatches": prepared.flatMap(\.swatches),
            "times": prepared.flatMap(\.times).map { ["iso": $0.iso, "local": $0.local] },
        ]
    }
}

/// A small rounded square in a hex colour, drawn in the line of text beside the code that names it.
struct SwatchAttachment: Textual.Attachment {
    let hex: String
    let side: CGFloat

    var description: String { "" }
    var selectionStyle: AttachmentSelectionStyle { .text }

    var body: some View {
        RoundedRectangle(cornerRadius: 2.5, style: .continuous)
            .fill(Color(hex: hex) ?? .clear)
            .overlay(RoundedRectangle(cornerRadius: 2.5, style: .continuous).strokeBorder(Color.primary.opacity(0.25), lineWidth: 0.5))
            .frame(width: side, height: side)
    }

    func baselineOffset(in _: TextEnvironmentValues) -> CGFloat { -side * 0.12 }
    func sizeThatFits(_: ProposedViewSize, in _: TextEnvironmentValues) -> CGSize { CGSize(width: side, height: side) }
}

/// Resolves the `wheel-swatch:<hex>` URLs `ChatRich` writes into a reply to swatches.
struct SwatchLoader: AttachmentLoader {
    let side: CGFloat

    func attachment(for url: URL, text _: String, environment _: ColorEnvironmentValues) async throws -> SwatchAttachment {
        SwatchAttachment(hex: url.absoluteString.dropFirst(ChatRich.swatchScheme.count + 1).description, side: side)
    }
}

extension Color {
    /// `rgb`, `rrggbb` or `rrggbbaa` hex digits, no `#`.
    init?(hex: String) {
        var digits = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
        if digits.count == 3 { digits = digits.map { "\($0)\($0)" }.joined() }
        guard digits.count == 6 || digits.count == 8, let value = UInt64(digits, radix: 16) else { return nil }
        let shift: UInt64 = digits.count == 8 ? 8 : 0
        let alpha = digits.count == 8 ? Double(value & 0xFF) / 255 : 1
        self.init(.sRGB,
                  red: Double((value >> (16 + shift)) & 0xFF) / 255,
                  green: Double((value >> (8 + shift)) & 0xFF) / 255,
                  blue: Double((value >> shift) & 0xFF) / 255,
                  opacity: alpha)
    }
}
