import SwiftUI

/// The text styles a selectable prose block is drawn in, which the platform maps to its own font.
public enum ProseStyle: Sendable {
    case body, callout
}

/// What the transcript needs from the platform it runs on, as closures the app supplies through
/// the environment: input monitors, selection geometry and its highlight. The default does
/// nothing, which is a transcript with no custom scroll keys and no cross-row selection.
public struct TranscriptPlatform: Sendable {
    /// The transcript for `chat` appeared.
    public var appear: @MainActor @Sendable (SessionChat) -> Void
    /// The transcript for `chat` left the screen.
    public var disappear: @MainActor @Sendable (SessionChat) -> Void
    /// The selection drawn behind one row's prose of `text`.
    public var proseHighlight: @MainActor @Sendable (_ text: String, _ id: Int, _ style: ProseStyle, _ chat: SessionChat) -> AnyView
    /// The selection drawn behind a row that is not prose.
    public var itemHighlight: @MainActor @Sendable (_ item: ChatItem, _ chat: SessionChat) -> AnyView

    public init(
        appear: @escaping @MainActor @Sendable (SessionChat) -> Void,
        disappear: @escaping @MainActor @Sendable (SessionChat) -> Void,
        proseHighlight: @escaping @MainActor @Sendable (String, Int, ProseStyle, SessionChat) -> AnyView,
        itemHighlight: @escaping @MainActor @Sendable (ChatItem, SessionChat) -> AnyView
    ) {
        self.appear = appear
        self.disappear = disappear
        self.proseHighlight = proseHighlight
        self.itemHighlight = itemHighlight
    }

    public static let none = TranscriptPlatform(
        appear: { _ in },
        disappear: { _ in },
        proseHighlight: { _, _, _, _ in AnyView(EmptyView()) },
        itemHighlight: { _, _ in AnyView(EmptyView()) }
    )
}

private struct TranscriptPlatformKey: EnvironmentKey {
    static let defaultValue = TranscriptPlatform.none
}

extension EnvironmentValues {
    public var transcriptPlatform: TranscriptPlatform {
        get { self[TranscriptPlatformKey.self] }
        set { self[TranscriptPlatformKey.self] = newValue }
    }
}

extension View {
    /// Marks a view as a row's prose for the transcript selection: reports where it sits, and
    /// draws the selected characters behind it.
    func selectableText(_ text: String, id: Int, chat: SessionChat, style: ProseStyle) -> some View {
        modifier(SelectableText(text: text, id: id, chat: chat, style: style))
    }
}

private struct SelectableText: ViewModifier {
    let text: String
    let id: Int
    let chat: SessionChat
    let style: ProseStyle
    @Environment(\.transcriptPlatform) private var platform

    func body(content: Content) -> some View {
        content
            .background { platform.proseHighlight(text, id, style, chat) }
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
                chat.selection.targets[id] = TranscriptSelection.Target(frame: $0, style: style)
            }
            .onDisappear { chat.selection.targets[id] = nil }
    }
}
