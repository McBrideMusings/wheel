import SwiftUI
import Textual
import WheelKit

private struct InsideQuoteKey: EnvironmentKey { static let defaultValue = false }

extension EnvironmentValues {
    /// A block quote encloses this view, so a nested quote draws its bar but no fill.
    fileprivate var insideQuote: Bool {
        get { self[InsideQuoteKey.self] }
        set { self[InsideQuoteKey.self] = newValue }
    }
}

/// A block quote as the `chat.quote.style` setting draws it. Each level of nesting is one more
/// 2 point bar, never a second panel.
struct ChatBlockQuoteStyle: StructuredText.BlockQuoteStyle {
    let style: ChatAppearance.QuoteStyle
    @Environment(\.insideQuote) private var inside

    @ViewBuilder func makeBody(configuration: Configuration) -> some View {
        let content = configuration.label
            .frame(maxWidth: .infinity, alignment: .leading)
            .environment(\.insideQuote, true)
        switch style {
        case .bar, .fill:
            let filled = style == .fill && !inside
            HStack(alignment: .top, spacing: 8) {
                Rectangle().fill(Color.secondary.opacity(0.5)).frame(width: 2)
                content.foregroundStyle(.secondary)
            }
            .fixedSize(horizontal: false, vertical: true)
            .padding(.vertical, filled ? 6 : 0)
            .padding(.trailing, filled ? 8 : 0)
            .background {
                if filled {
                    RoundedRectangle(cornerRadius: 4, style: .continuous).fill(Color.primary.opacity(0.04))
                }
            }
        case .italic:
            HStack(alignment: .top, spacing: 4) {
                Text("\u{201C}").foregroundStyle(.secondary)
                content.italic()
            }
        }
    }
}

/// The code blocks on screen, for the debug server's state and its copy route.
@MainActor
public final class CodeBlockRegistry {
    public static let shared = CodeBlockRegistry()

    public struct Entry {
        public let session: String
        public let language: String?
        public let text: String
        /// Presses the block's copy button.
        public let press: () -> Void
        fileprivate let order: Int
    }

    private var entries: [UUID: Entry] = [:]
    private var counter = 0

    func set(_ id: UUID, session: String, language: String?, text: String, press: @escaping () -> Void) {
        var order = entries[id]?.order ?? 0
        if order == 0 {
            counter += 1
            order = counter
        }
        entries[id] = Entry(session: session, language: language, text: text, press: press, order: order)
    }

    func remove(_ id: UUID) { entries[id] = nil }

    /// The blocks on screen for `session`, oldest first.
    public func blocks(session: String) -> [Entry] {
        entries.values.filter { $0.session == session }.sorted { $0.order < $1.order }
    }
}

/// A code block: the theme's ground, a header with the language and a copy button drawn at all
/// times, and the code scrolling or wrapping as `code.wrap` says.
struct ChatCodeBlockStyle: StructuredText.CodeBlockStyle {
    let look: ChatAppearance
    let session: String

    func makeBody(configuration: Configuration) -> some View {
        CodeBlockView(look: look, session: session, configuration: configuration)
    }
}

private struct CodeBlockView: View {
    let look: ChatAppearance
    let session: String
    let configuration: StructuredText.CodeBlockStyleConfiguration
    @State private var id = UUID()
    @State private var copied = false

    /// The block's code. Textual keeps it inside the configuration's proxy and offers no way to
    /// read it, only a copy that goes through its own formatter.
    private var code: String {
        for child in Mirror(reflecting: configuration.codeBlock).children {
            if let content = child.value as? AttributedSubstring { return String(content.characters) }
        }
        return ""
    }

    private var language: String? {
        configuration.languageHint.flatMap { $0.isEmpty ? nil : $0 }
    }

    var body: some View {
        let code = code
        let theme = look.codeTheme
        let lines = code.split(separator: "\n", omittingEmptySubsequences: false).count
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language ?? "")
                    .font(look.meta)
                    .foregroundStyle(theme.dim)
                Spacer()
                Button {
                    press(code)
                } label: {
                    Image(systemName: copied ? "checkmark" : "doc.on.doc")
                        .font(look.meta)
                        .frame(minWidth: 16)
                }
                .buttonStyle(.plain)
                .foregroundStyle(theme.dim)
                .accessibilityLabel("Copy code")
                .help("Copy this code block")
            }
            .padding(.horizontal, 12)
            .padding(.top, 6)
            .padding(.bottom, 2)
            Overflow {
                HStack(alignment: .top, spacing: 12) {
                    if look.lineNumbers && lines > 3 {
                        Text((1...lines).map(String.init).joined(separator: "\n"))
                            .font(look.codeFont(look.codeSize))
                            .lineSpacing(look.codeSize * 0.3)
                            .multilineTextAlignment(.trailing)
                            .foregroundStyle(theme.dim)
                    }
                    configuration.label
                        .font(look.codeFont(look.codeSize))
                        .textual.lineSpacing(.fontScaled(0.3))
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.horizontal, 12)
                .padding(.top, 4)
                .padding(.bottom, 10)
            }
        }
        .background(theme.background)
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .textual.blockSpacing(.fontScaled(top: 0.6, bottom: 0))
        .onAppear { register(code) }
        .onChange(of: code) { register(code) }
        .onDisappear { CodeBlockRegistry.shared.remove(id) }
    }

    private func register(_ code: String) {
        CodeBlockRegistry.shared.set(id, session: session, language: language, text: code) { press(code) }
    }

    private func press(_ code: String) {
        copyToPasteboard(code)
        copied = true
        Task {
            try? await Task.sleep(for: .milliseconds(1500))
            copied = false
        }
    }
}
