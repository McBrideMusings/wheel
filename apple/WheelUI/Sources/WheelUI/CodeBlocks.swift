import SwiftUI
import Textual
import WheelKit
#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

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
        /// False when Textual's proxy gave no code, so `text` is empty because the read failed.
        public let readable: Bool
        /// How the block draws: `code`, `diff`, or `svg` (the image) / `svg-source` (the SVG did not
        /// parse as an image, so its source shows).
        public let drawn: String
        /// Presses the block's copy button.
        public let press: () -> Void
        fileprivate let id: UUID
    }

    private var entries: [UUID: Entry] = [:]
    /// Each block's top edge in window coordinates; document order is top to bottom. Kept apart
    /// from the entries because the first position can arrive before the block registers.
    private var tops: [UUID: CGFloat] = [:]

    func set(_ id: UUID, session: String, language: String?, text: String, readable: Bool, drawn: String,
             press: @escaping () -> Void) {
        entries[id] = Entry(session: session, language: language, text: text, readable: readable,
                            drawn: drawn, press: press, id: id)
    }

    func move(_ id: UUID, top: CGFloat) { tops[id] = top }

    func remove(_ id: UUID) {
        entries[id] = nil
        tops[id] = nil
    }

    /// The blocks on screen for `session`, in document order (top of the transcript first).
    public func blocks(session: String) -> [Entry] {
        entries.values.filter { $0.session == session }.sorted { (tops[$0.id] ?? 0) < (tops[$1.id] ?? 0) }
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
    @State private var labelWidth: CGFloat = 0
    @State private var heights: [Int: CGFloat] = [:]

    /// The block's code, or nil when the read failed. Textual keeps it inside the configuration's
    /// proxy and offers no way to read it, only a copy that goes through its own formatter.
    private var read: String? {
        for child in Mirror(reflecting: configuration.codeBlock).children {
            if let content = child.value as? AttributedSubstring { return String(content.characters) }
        }
        return nil
    }

    /// What the copy button puts on the pasteboard: the code with the final newline of the fenced
    /// block, which Textual strips from what it renders.
    private var code: String {
        guard let read, !read.isEmpty else { return "" }
        return read.hasSuffix("\n") ? read : read + "\n"
    }

    private var language: String? {
        configuration.languageHint.flatMap { $0.isEmpty ? nil : $0 }
    }

    /// The fence's lines as a diff, when its language says diff or patch.
    private var diff: [DiffFenceLine]? {
        guard DiffFence.handles(language), let read else { return nil }
        return DiffFence.parse(read)
    }

    /// The fence's SVG drawn as an image after scripts, handlers and external references are
    /// removed; nil when the language is not svg or the cleaned text does not load as an image.
    private var svgImage: Image? {
        guard SVGFence.handles(language), let read, let clean = SVGFence.sanitize(read),
              let data = clean.data(using: .utf8) else { return nil }
        #if canImport(AppKit)
        guard let image = NSImage(data: data), image.isValid, image.size.width > 0 else { return nil }
        return Image(nsImage: image)
        #else
        guard let image = UIImage(data: data) else { return nil }
        return Image(uiImage: image)
        #endif
    }

    /// `code`, `diff`, `svg`, or `svg-source` when an svg fence could not be drawn.
    private var drawn: String {
        if diff != nil { return "diff" }
        if SVGFence.handles(language) { return svgImage == nil ? "svg-source" : "svg" }
        return "code"
    }

    var body: some View {
        let code = code
        let theme = look.codeTheme
        let lineTexts = (read ?? "").split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        let lines = lineTexts.count
        let spacing = look.codeSize * 0.3
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
                    if look.lineNumbers && lines > 3 && drawn == "code" {
                        if look.codeWrap == .wrap {
                            // A wrapped line is taller than one row, so each number takes the
                            // height its line measures at the label's width (see `measuring`).
                            VStack(alignment: .trailing, spacing: 0) {
                                ForEach(0..<lines, id: \.self) { index in
                                    Text(String(index + 1))
                                        .font(look.codeFont(look.codeSize))
                                        .foregroundStyle(theme.dim)
                                        .frame(height: (heights[index] ?? 0) + spacing, alignment: .top)
                                }
                            }
                        } else {
                            Text((1...lines).map(String.init).joined(separator: "\n"))
                                .font(look.codeFont(look.codeSize))
                                .lineSpacing(spacing)
                                .multilineTextAlignment(.trailing)
                                .foregroundStyle(theme.dim)
                        }
                    }
                    if let diff {
                        DiffFenceView(lines: diff, look: look)
                    } else if let svgImage {
                        svgImage.resizable().scaledToFit().frame(maxWidth: .infinity, maxHeight: 320, alignment: .leading)
                    } else {
                        configuration.label
                            .font(look.codeFont(look.codeSize))
                            .textual.lineSpacing(.fontScaled(0.3))
                            .fixedSize(horizontal: false, vertical: true)
                            .onGeometryChange(for: CGFloat.self, of: \.size.width) { labelWidth = $0 }
                            .background(alignment: .topLeading) {
                                if look.lineNumbers && lines > 3 && look.codeWrap == .wrap {
                                    measuring(lineTexts, spacing: spacing)
                                }
                            }
                    }
                }
                .padding(.horizontal, 12)
                .padding(.top, 4)
                .padding(.bottom, 10)
            }
        }
        .background(theme.background)
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .textual.blockSpacing(.fontScaled(top: 0.6, bottom: 0))
        .onGeometryChange(for: CGFloat.self, of: { $0.frame(in: .global).minY }) {
            CodeBlockRegistry.shared.move(id, top: $0)
        }
        .onAppear { register(code) }
        .onChange(of: code) { register(code) }
        .onDisappear { CodeBlockRegistry.shared.remove(id) }
    }

    /// Each line laid out alone at the label's width, invisible, to learn how tall it wraps.
    private func measuring(_ lineTexts: [String], spacing: CGFloat) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(lineTexts.enumerated()), id: \.offset) { index, text in
                Text(text.isEmpty ? " " : text)
                    .font(look.codeFont(look.codeSize))
                    .lineSpacing(spacing)
                    .frame(width: max(labelWidth, 1), alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                    .onGeometryChange(for: CGFloat.self, of: \.size.height) { heights[index] = $0 }
                    .padding(.bottom, spacing)
            }
        }
        .opacity(0)
        .allowsHitTesting(false)
    }

    private func register(_ code: String) {
        let readable = read != nil
        if !readable {
            UILog.info("code block in \(session): Textual's CodeBlockProxy holds no AttributedSubstring, so its code cannot be read")
        }
        CodeBlockRegistry.shared.set(id, session: session, language: language, text: code, readable: readable, drawn: drawn) {
            press(code)
        }
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

/// A diff fence: added and removed lines on the tints the edits card uses, hunk and file header
/// lines dimmed.
private struct DiffFenceView: View {
    let lines: [DiffFenceLine]
    let look: ChatAppearance

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                Text(line.text.isEmpty ? " " : line.text)
                    .font(look.codeFont(look.codeSize))
                    .fontWeight(line.kind == .file ? .semibold : .regular)
                    .foregroundStyle(line.kind == .hunk || line.kind == .file ? AnyShapeStyle(look.codeTheme.dim) : AnyShapeStyle(.primary))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(tint(line.kind))
            }
        }
        .textSelection(.enabled)
    }

    private func tint(_ kind: DiffFenceLine.Kind) -> Color {
        switch kind {
        case .added: Color.green.opacity(0.14)
        case .removed: Color.red.opacity(0.14)
        default: Color.clear
        }
    }
}
