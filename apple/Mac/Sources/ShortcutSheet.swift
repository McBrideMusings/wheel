import SwiftUI
import WheelKit

/// The shortcut sheet (⌘/) over the main window: every action that has keys, by group, each with
/// the key the keymap gives it now. Stock macOS styling.
struct ShortcutSheet: View {
    var model: AppModel

    @Environment(\.colorScheme) private var scheme

    private func groupView(_ group: (name: String, rows: [CommandActions.ShortcutRow])) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(group.name).font(.subheadline.weight(.semibold)).foregroundStyle(.secondary)
            ForEach(group.rows, id: \.id) { row in
                HStack {
                    Text(row.title).lineLimit(1)
                    Spacer(minLength: 8)
                    Text(row.keys).font(.system(.callout, design: .monospaced)).foregroundStyle(.secondary)
                }
                .padding(.vertical, 1)
            }
        }
    }

    var body: some View {
        ZStack {
            Color.black.opacity(scheme == .dark ? 0.55 : 0.30)
                .contentShape(Rectangle())
                .onTapGesture { model.closeShortcuts() }
            VStack(spacing: 0) {
                Text("Keyboard Shortcuts")
                    .font(.headline)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                Divider()
                // Three columns so every group shows at the window's minimum size: Panes, Spaces,
                // then Windows over Agent.
                let groups = CommandActions.shortcutSheet(model)
                ScrollView {
                    HStack(alignment: .top, spacing: 24) {
                        ForEach([[0], [1], [2, 3]], id: \.self) { column in
                            VStack(alignment: .leading, spacing: 14) {
                                ForEach(column.filter { groups.indices.contains($0) }, id: \.self) { index in
                                    groupView(groups[index])
                                }
                            }
                            .frame(maxWidth: .infinity, alignment: .topLeading)
                        }
                    }
                    .padding(12)
                }
            }
            .frame(maxWidth: 820, maxHeight: 520)
            .background(.background, in: RoundedRectangle(cornerRadius: 10))
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator, lineWidth: 0.5))
            .shadow(color: .black.opacity(0.28), radius: 14, y: 4)
            .padding(16)
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Keyboard shortcuts")
        }
    }
}
