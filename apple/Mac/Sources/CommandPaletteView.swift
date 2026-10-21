import SwiftUI
import WheelKit

/// The command palette over the main window: the window dims, and a search field above the
/// matching rows sits near its top. Stock macOS styling.
struct CommandPaletteOverlay: View {
    @ObservedObject var model: AppModel
    let palette: CommandPalette

    @Environment(\.colorScheme) private var scheme
    @FocusState private var fieldFocused: Bool

    var body: some View {
        ZStack(alignment: .top) {
            Color.black.opacity(scheme == .dark ? 0.55 : 0.30)
                .contentShape(Rectangle())
                .onTapGesture { model.closeCommandPalette() }
            VStack(spacing: 0) {
                searchField
                Divider()
                rows
            }
            .frame(maxWidth: 560)
            .background(.background, in: RoundedRectangle(cornerRadius: 10))
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator, lineWidth: 0.5))
            .shadow(color: .black.opacity(0.28), radius: 14, y: 4)
            .padding(.horizontal, 16)
            .padding(.top, 48)
            .onExitCommand { model.closeCommandPalette() }
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Command palette")
        }
        .onAppear { DispatchQueue.main.async { fieldFocused = true } }
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField("Run an action, open a space or a folder",
                      text: Binding(get: { palette.query }, set: { model.commandPaletteTyped($0) }))
                .textFieldStyle(.plain)
                .font(.system(size: 14))
                .focused($fieldFocused)
                .onKeyPress(.upArrow) { model.commandPaletteMoved(by: -1); return .handled }
                .onKeyPress(.downArrow) { model.commandPaletteMoved(by: 1); return .handled }
                .onSubmit { model.commandPaletteRun() }
                .accessibilityLabel("Search commands")
        }
        .padding(.horizontal, 12)
        .frame(height: 40)
    }

    @ViewBuilder private var rows: some View {
        if palette.rows.isEmpty {
            Text("Nothing matches “\(palette.query)”")
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, minHeight: 60)
        } else {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(palette.rows.enumerated()), id: \.element.id) { index, row in
                            PaletteRow(row: row, selected: index == palette.selected)
                                .id(row.id)
                                .onTapGesture {
                                    model.commandPaletteSelect(index)
                                    model.commandPaletteRun()
                                }
                        }
                    }
                    .padding(4)
                }
                .frame(maxHeight: 320)
                .onChange(of: palette.selected) {
                    if palette.rows.indices.contains(palette.selected) { proxy.scrollTo(palette.rows[palette.selected].id) }
                }
            }
        }
    }
}

private struct PaletteRow: View {
    let row: CommandPalette.Row
    let selected: Bool

    private var icon: String {
        switch row.item {
        case .action: "command"
        case .space: "rectangle.3.group"
        case .recent: "clock.arrow.circlepath"
        case .openFolder: "folder.badge.plus"
        }
    }

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: icon).foregroundStyle(selected ? .primary : .secondary).frame(width: 18)
            Text(row.item.title).lineLimit(1)
            Spacer(minLength: 8)
            if !row.item.detail.isEmpty {
                Text(row.item.detail)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.head)
            }
        }
        .padding(.horizontal, 8)
        .frame(height: 28)
        .background(selected ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: RoundedRectangle(cornerRadius: 6))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
