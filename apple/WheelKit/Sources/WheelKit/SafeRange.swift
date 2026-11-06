import Foundation

/// A closed range from two runtime values that cannot trap, whichever is larger.
func orderedRange<T: Comparable>(_ a: T, _ b: T) -> ClosedRange<T> {
    // swiftlint:disable:next closed_range_between_values
    min(a, b) ... max(a, b)
}

/// A regular expression for a constant pattern; `nil` (never for a valid pattern) instead of a trap.
func safeRegex(_ pattern: String) -> NSRegularExpression? {
    try? NSRegularExpression(pattern: pattern)
}
