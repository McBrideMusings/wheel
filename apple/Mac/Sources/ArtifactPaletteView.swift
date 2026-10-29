import SwiftUI
import WheelKit

/// The palette over one pane's body: the body dims, and a search field above a grouped list sits
/// inside the pane's bounds. Only that pane is covered; the other panes stay lit.
struct PaletteOverlay: View {
    var model: AppModel
    @ObservedObject var palette: PanePalette
    let paneId: String
    let kind: PaneKind

    @Environment(\.colorScheme) private var scheme

    private var overTerminal: Bool { if case .terminal = kind { true } else { false } }

    var body: some View {
        GeometryReader { geo in
            let narrow = geo.size.width < 420
            let compact = geo.size.height < 340
            let inset: CGFloat = narrow ? 8 : 10
            ZStack(alignment: .top) {
                Color.black.opacity(scheme == .dark || overTerminal ? 0.55 : 0.30)
                    .contentShape(Rectangle())
                    .onTapGesture { model.closePalette(paneId) }
                PaletteCard(model: model, palette: palette, paneId: paneId, narrow: narrow, compact: compact)
                    .frame(maxWidth: narrow ? .infinity : 560)
                    .frame(maxHeight: geo.size.height - inset * 2, alignment: .top)
                    .padding(.horizontal, inset)
                    .padding(.top, inset)
            }
        }
        .confirmationDialog(kind.isTerminal ? "Kill this pane's shell?" : "End this pane's agent session?",
                            isPresented: Binding(get: { palette.confirming != nil }, set: { if !$0 { palette.confirming = nil } }),
                            presenting: palette.confirming) { info in
            Button("Show an Artifact", role: .destructive) { model.showArtifact(paneId, info) }
        } message: { _ in
            Text(kind.isTerminal ? "Whatever is running in it stops." : "Its conversation and anything it is doing stop.")
        }
    }
}

/// A scroll view as tall as its content and no taller than the room it is given, so a short list
/// makes a short card.
private struct FitScroll<Content: View>: View {
    @ViewBuilder let content: Content
    @State private var height: CGFloat?

    var body: some View {
        ScrollView {
            content.onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
        }
        .frame(maxHeight: height ?? .infinity)
    }
}

private extension PaneKind {
    var isTerminal: Bool { if case .terminal = self { true } else { false } }
}

private struct PaletteCard: View {
    var model: AppModel
    @ObservedObject var palette: PanePalette
    let paneId: String
    let narrow: Bool
    let compact: Bool

    @FocusState private var fieldFocused: Bool
    @FocusState private var retryFocused: Bool
    @FocusState private var noteFocused: Bool

    private var state: ArtifactPalette { palette.state }

    var body: some View {
        VStack(spacing: 0) {
            searchField
            Divider()
            content
                .frame(maxWidth: .infinity)
            Divider()
            footer
        }
        .background(.background, in: RoundedRectangle(cornerRadius: 10))
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator, lineWidth: 0.5))
        .shadow(color: .black.opacity(0.28), radius: 14, y: 4)
        .onExitCommand { model.paletteEscaped(paneId) }
        .onAppear { focusInput() }
        .onChange(of: fieldFocused) { _, focused in palette.fieldFocused = focused }
        .onChange(of: state.failure) { _, _ in focusInput() }
        .onChange(of: state.searchable) { _, _ in focusInput() }
        .onChange(of: state.isLoading) { _, _ in focusInput() }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Artifact palette")
    }

    /// The field takes the keyboard when there is something to search; the retry button takes it
    /// after a failure, so Return lists again.
    private func focusInput() {
        DispatchQueue.main.async {
            if state.failure != nil {
                retryFocused = true
            } else if !state.searchable {
                noteFocused = true
            } else {
                fieldFocused = true
            }
        }
    }

    // MARK: Search field

    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            if state.searchable {
                TextField(state.fieldPlaceholder, text: Binding(get: { state.query }, set: { model.paletteTyped(paneId, $0) }))
                    .textFieldStyle(.plain)
                    .font(.system(size: 14))
                    .focused($fieldFocused)
                    .onKeyPress(.upArrow) { model.paletteMoved(paneId, by: -1); return .handled }
                    .onKeyPress(.downArrow) { model.paletteMoved(paneId, by: 1); return .handled }
                    .onSubmit { model.paletteEntered(paneId) }
                    .accessibilityLabel("Search artifacts")
            } else {
                // Nothing to search: the field says why, in the placeholder's colour.
                Text(state.fieldPlaceholder)
                    .font(.system(size: 14))
                    .foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    // Focusable so Esc still reaches the palette.
                    .focusable()
                    .focusEffectDisabled()
                    .focused($noteFocused)
            }
            if !state.query.isEmpty {
                Button { model.paletteTyped(paneId, "") } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 12)
        .frame(height: compact ? 34 : 40)
    }

    // MARK: Body

    @ViewBuilder private var content: some View {
        switch state.listing {
        case .loading:
            ProgressView().controlSize(.small).frame(maxWidth: .infinity, maxHeight: .infinity)
        case let .failed(message):
            FitScroll { failedNote(message) }
        case let .ready(list):
            if list.isEmpty {
                FitScroll { emptyNote }
            } else if state.rows.isEmpty {
                FitScroll { noMatchNote }
            } else {
                rowsList
            }
        }
    }

    private var rowsList: some View {
        ScrollViewReader { proxy in
            FitScroll {
                LazyVStack(spacing: 0, pinnedViews: .sectionHeaders) {
                    group(.project)
                    group(.machine)
                }
                .padding(.bottom, 4)
            }
            .onChange(of: state.selectedRow?.id) { _, id in
                if let id { proxy.scrollTo(id) }
            }
        }
    }

    @ViewBuilder private func group(_ scope: ArtifactScope) -> some View {
        let rows = state.rows.filter { $0.info.scope == scope }
        if !rows.isEmpty {
            Section {
                ForEach(rows) { row in
                    PaletteRow(row: row, selected: state.selectedRow?.id == row.id, compact: compact, narrow: narrow)
                        .id(row.id)
                        .onTapGesture { model.paletteChose(paneId, row.info) }
                }
            } header: {
                HStack {
                    Text(scope.rawValue)
                    Spacer()
                    Text("\(rows.count)")
                }
                .font(.system(size: 10.5, weight: .semibold))
                .textCase(.uppercase)
                .tracking(0.44)
                .foregroundStyle(.tertiary)
                .padding(.horizontal, 16)
                .padding(.top, compact ? 5 : 8)
                .padding(.bottom, compact ? 2 : 3)
                .frame(maxWidth: .infinity)
                .background(.background)
                .accessibilityAddTraits(.isHeader)
            }
        }
    }

    // MARK: Notes

    private var noMatchNote: some View {
        VStack(spacing: 6) {
            Text("No artifact matches “\(state.query)”").font(.system(size: 13, weight: .semibold))
            Text(state.noMatchDetail).font(.system(size: 12)).foregroundStyle(.secondary)
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity)
        .padding(compact ? 12 : 18)
    }

    private var emptyNote: some View {
        VStack(spacing: 6) {
            Image(systemName: "rectangle.dashed").font(.system(size: 20)).foregroundStyle(.secondary)
            Text(ArtifactPalette.emptyTitle).font(.system(size: 13, weight: .semibold))
            Text(ArtifactPalette.emptyHelp).font(.system(size: 12)).foregroundStyle(.secondary)
            // On one line when it fits, the command under the request when the pane is narrow.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 5) { emptyAsk; emptyCommand }
                VStack(spacing: 4) { emptyAsk; emptyCommand }
            }
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity)
        .padding(compact ? 12 : 20)
    }

    private var emptyAsk: some View {
        Text(ArtifactPalette.emptyAsk).font(.system(size: 12)).foregroundStyle(.secondary).fixedSize()
    }

    private var emptyCommand: some View {
        Text(ArtifactPalette.emptyCommand)
            .font(.system(size: 11.5, design: .monospaced))
            .padding(.horizontal, 5)
            .padding(.vertical, 1)
            .background(.quaternary, in: RoundedRectangle(cornerRadius: 4))
            .fixedSize()
    }

    private func failedNote(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Image(systemName: "exclamationmark.circle").font(.system(size: 18)).foregroundStyle(.red)
                Text(ArtifactPalette.failedTitle).font(.system(size: 13, weight: .semibold))
            }
            Text(ArtifactPalette.failedLead).font(.system(size: 12)).foregroundStyle(.secondary)
            Text(message)
                .font(.system(size: 11.5, design: .monospaced))
                .foregroundStyle(.red)
                .textSelection(.enabled)
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
            Button { model.askArtifacts(paneId) } label: {
                Text(ArtifactPalette.retry)
                    .font(.system(size: 12))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 3)
                    .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 5))
                    // The comp's focused button: an accent border inside a soft accent halo.
                    .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(Color.accentColor, lineWidth: 1))
                    .background(RoundedRectangle(cornerRadius: 8).fill(Color.accentColor.opacity(0.25)).padding(-3))
            }
            .buttonStyle(.plain)
            .focused($retryFocused)
            .focusEffectDisabled()
            .onKeyPress(.return) { model.askArtifacts(paneId); return .handled }
            .padding(.top, 2)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(compact ? 10 : 16)
    }

    // MARK: Footer

    private var footer: some View {
        HStack(spacing: 10) {
            Text(state.footer)
            Spacer(minLength: 4)
            HStack(spacing: 12) {
                ForEach(Array(state.keys.enumerated()), id: \.offset) { index, key in
                    // The narrow footer drops the move hint to keep the others on one line.
                    if !(narrow && index == 0 && key.symbol == "↑↓") {
                        HStack(spacing: 3) {
                            Text(key.symbol)
                                .font(.system(size: 10.5))
                                .padding(.horizontal, 4)
                                .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(.separator, lineWidth: 1))
                            Text(key.word)
                        }
                        .opacity(key.enabled ? 1 : 0.45)
                    }
                }
            }
            .lineLimit(1)
            .fixedSize()
        }
        .font(.system(size: 11))
        .foregroundStyle(.tertiary)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
        .background(.quaternary.opacity(0.5))
    }
}

/// One artifact: its title (bold) over its name (mono), and the daemon's problem line when it has
/// one. The characters the query matched are bold in the accent colour.
private struct PaletteRow: View {
    let row: ArtifactPalette.Row
    let selected: Bool
    let compact: Bool
    let narrow: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "doc.text")
                .font(.system(size: 13))
                .frame(width: 26, height: 26)
                .background(selected ? Color.white.opacity(0.22) : Color.primary.opacity(0.08), in: RoundedRectangle(cornerRadius: 6))
                .foregroundStyle(selected ? Color.white : Color.secondary)
                .padding(.top, 1)
            VStack(alignment: .leading, spacing: 1) {
                if row.info.hasTitle {
                    marked(row.info.title, row.titleMatch, mono: false)
                        .font(.system(size: 13, weight: .medium))
                        .lineLimit(narrow ? 1 : 2)
                    marked(row.info.name, row.nameMatch, mono: true)
                        .font(.system(size: 11.5, design: .monospaced))
                        .foregroundStyle(selected ? Color.white.opacity(0.85) : Color.secondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                } else {
                    marked(row.info.name, row.nameMatch, mono: true)
                        .font(.system(size: 13, weight: .medium, design: .monospaced))
                        .lineLimit(narrow ? 1 : 2)
                }
                ForEach(row.info.problems, id: \.self) { problem in
                    HStack(alignment: .top, spacing: 5) {
                        Image(systemName: "exclamationmark.triangle").font(.system(size: 11)).padding(.top, 1)
                        Text(Self.wrapAfterSlashes(problem))
                    }
                    .font(.system(size: 11.5))
                    .foregroundStyle(selected ? Color.white : Color.orange)
                    .padding(.top, 3)
                }
            }
            Spacer(minLength: 0)
        }
        .foregroundStyle(selected ? Color.white : Color.primary)
        .padding(.horizontal, 10)
        .padding(.vertical, compact ? 4 : 6)
        .background(selected ? Color.accentColor : Color.clear, in: RoundedRectangle(cornerRadius: 7))
        .padding(.horizontal, 6)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? [.isSelected, .isButton] : .isButton)
    }

    private func marked(_ text: String, _ hits: [Int], mono: Bool) -> Text {
        let hit = Set(hits)
        var out = Text("")
        var run = ""
        var inHit = false
        func flush() {
            guard !run.isEmpty else { return }
            if inHit {
                var piece = AttributedString(run)
                piece.font = mono ? .system(size: 11.5, weight: .bold, design: .monospaced) : .system(size: 13, weight: .bold)
                if selected { piece.underlineStyle = .single } else { piece.foregroundColor = .accentColor }
                out = out + Text(piece)
            } else {
                out = out + Text(run)
            }
            run = ""
        }
        for (i, ch) in text.enumerated() {
            if hit.contains(i) != inHit { flush(); inHit = hit.contains(i) }
            run.append(ch)
        }
        flush()
        return out
    }

    /// A problem line wraps only after a `/` of the path it names: the path's spaces stop
    /// breaking, and a break opportunity follows each slash.
    static func wrapAfterSlashes(_ problem: String) -> String {
        guard let slash = problem.firstIndex(of: "/"), problem[..<slash].hasSuffix(" ") || slash == problem.startIndex else { return problem }
        let path = problem[slash...].replacingOccurrences(of: " ", with: "\u{00A0}").replacingOccurrences(of: "/", with: "/\u{200B}")
        return String(problem[..<slash]) + path
    }
}
