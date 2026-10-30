import SwiftUI
import WheelKit

/// The chat's type scale (DESIGN.md): three sizes and two weights. Content and the composer read
/// at `body`; tool, reasoning and permission lines at `line`; times, counts and settings at
/// `meta`. Semibold marks a name or a heading, never a whole line. Monospace is for code only.
///
/// These are the fixed fonts of everything that is not the transcript. The transcript reads its
/// own through `ChatAppearance`, which the `chat.*` settings set per space.
public enum ChatFont {
    public static let body = Font.body
    public static let line = Font.callout
    public static let meta = Font.caption
    public static let code = Font.system(.callout, design: .monospaced)
}

extension ChatAppearance {
    private var design: Font.Design {
        switch family {
        case .system: .default
        case .serif: .serif
        case .rounded: .rounded
        case .mono: .monospaced
        }
    }

    private func font(_ size: Double) -> Font { .system(size: size, design: design) }

    public var body: Font { font(size) }
    public var line: Font { font(lineSize) }
    public var meta: Font { font(metaSize) }
    /// Code is monospace whatever the text around it is set in.
    public var code: Font { .system(size: lineSize, design: .monospaced) }
    /// Code inside a sentence, as a multiple of the text around it.
    public static let inlineCodeScale: CGFloat = 0.92

    /// The type the selection geometry lays out body text and tool lines in.
    public var bodyProse: ProseStyle { ProseStyle(family: family, size: size, lineSpacing: lineSpacing(for: size)) }
    public var lineProse: ProseStyle { ProseStyle(family: family, size: lineSize, lineSpacing: lineSpacing(for: lineSize)) }
}

private struct ChatAppearanceKey: EnvironmentKey {
    static let defaultValue = ChatAppearance.standard
}

extension EnvironmentValues {
    /// How the chat under this view draws; set by the transcript from the space's `chat.*` settings.
    public var chatAppearance: ChatAppearance {
        get { self[ChatAppearanceKey.self] }
        set { self[ChatAppearanceKey.self] = newValue }
    }
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
