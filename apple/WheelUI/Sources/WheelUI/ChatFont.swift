import SwiftUI

/// The chat's type scale (DESIGN.md): three sizes and two weights. Content and the composer read
/// at `body`; tool, reasoning and permission lines at `line`; times, counts and settings at
/// `meta`. Semibold marks a name or a heading, never a whole line. Monospace is for code only.
public enum ChatFont {
    public static let body = Font.body
    public static let line = Font.callout
    public static let meta = Font.caption
    public static let code = Font.system(.callout, design: .monospaced)
}

/// Where the UI package writes its diagnostics. The app points `sink` at its own log file; until
/// it does, nothing is written.
@MainActor
public enum UILog {
    public static var sink: (String) -> Void = { _ in }

    static func info(_ message: String) {
        sink(message)
    }
}
