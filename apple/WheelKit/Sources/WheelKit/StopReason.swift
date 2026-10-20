import Foundation

/// Why a turn ended, in words, as the transcript's notice shows it.
public enum StopReason {
    /// The words for an ACP stop reason, or nil for a normal end, which needs no words. A reason this
    /// client does not know is spelled out (`some_reason` as `Some Reason`) rather than shown as a
    /// protocol token.
    public static func words(for reason: String) -> String? {
        switch reason {
        case "end_turn": nil
        case "max_tokens": "Stopped at its length limit"
        case "max_turn_requests": "Stopped at its request limit"
        case "refusal": "Declined to continue"
        case "cancelled": "Cancelled"
        case let other: other.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }
}
