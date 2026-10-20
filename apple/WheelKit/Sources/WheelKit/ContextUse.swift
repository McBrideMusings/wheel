import Foundation

/// How full an agent's context window is, as the agent last reported it. The daemon computes
/// nothing beyond the two numbers; the percent and the words below are how a client says them.
public struct ContextUse: Decodable, Equatable, Sendable {
    public let used: UInt64
    public let size: UInt64

    public init(used: UInt64, size: UInt64) {
        self.used = used
        self.size = size
    }

    /// The share of the window in use, 0 to 100 and over, rounded; 0 for a window of no size.
    public var percent: Int {
        guard size > 0 else { return 0 }
        return Int(min((Double(used) * 100 / Double(size)).rounded(), 999_999))
    }

    /// The share of the window in use as a fraction for a ring, held between 0 and 1.
    public var fraction: Double {
        guard size > 0 else { return 0 }
        return min(max(Double(used) / Double(size), 0), 1)
    }

    /// What the header shows beside the ring.
    public var shortText: String { "\(percent)%" }

    /// The raw figures in the short units a header has room for: "74k / 200k".
    public var rawText: String { "\(Self.short(used)) / \(Self.short(size))" }

    /// What the header shows beside the ring: the percent, or the raw figures when `raw`.
    public func text(raw: Bool) -> String { raw ? rawText : shortText }

    /// The exact numbers, for the hover.
    public var helpText: String {
        "Context window: \(Self.grouped(used)) of \(Self.grouped(size)) tokens used (\(percent)%)"
    }

    private static func short(_ n: UInt64) -> String {
        func trimmed(_ v: Double) -> String {
            let s = String(format: "%.1f", v)
            return s.hasSuffix(".0") ? String(s.dropLast(2)) : s
        }
        switch n {
        case 1_000_000...: return trimmed(Double(n) / 1_000_000) + "M"
        case 1_000...: return "\(Int((Double(n) / 1_000).rounded()))k"
        default: return String(n)
        }
    }

    private static func grouped(_ n: UInt64) -> String {
        let f = NumberFormatter()
        f.numberStyle = .decimal
        f.locale = Locale(identifier: "en_US_POSIX")
        f.groupingSeparator = ","
        f.usesGroupingSeparator = true
        return f.string(from: NSNumber(value: n)) ?? String(n)
    }
}
