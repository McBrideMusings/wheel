import SwiftUI
import WheelKit
import WheelUI

/// The sidebar: every space, one row each. A click on a space shows it.
struct SpacesSidebar: View {
    @ObservedObject var model: AppModel
    let windowID: String

    private enum Item: Identifiable {
        case space(SpaceInfo)
        case gap
        var id: String {
            switch self {
            case let .space(space): space.id
            case .gap: "gap"
            }
        }
    }

    /// The rows as the drag leaves them: the dragged space gone from its place, and a gap where
    /// it would land when the pointer is over this sidebar.
    private var items: [Item] {
        let held = model.spaces(in: windowID)
        guard let drag = model.spaceDrag else { return held.map(Item.space) }
        var rows = held.filter { $0.id != drag.space }.map(Item.space)
        if case let .sidebar(window, index)? = drag.target, window == windowID {
            // The index counts rows before the dragged one left its place.
            let moved = held.firstIndex { $0.id == drag.space }.map { $0 < index } ?? false
            rows.insert(.gap, at: min(moved ? index - 1 : index, rows.count))
        }
        return rows
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(items) { item in
                        switch item {
                        case let .space(space):
                            SidebarSpace(model: model, windowID: windowID, space: space, active: space.id == model.active(in: windowID))
                                .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.rowFrames[windowID, default: [:]][space.id] = $0 }
                        case .gap:
                            RoundedRectangle(cornerRadius: 6)
                                .strokeBorder(Color.accentColor, style: StrokeStyle(lineWidth: 1.5, dash: [4, 3]))
                                .frame(height: 26)
                                .transition(.opacity)
                                .accessibilityLabel("Drop here")
                        }
                    }
                }
                .padding(6)
                .animation(.smooth(duration: 0.2), value: items.map(\.id))
            }
            Divider()
            Button { openSpace(model) } label: {
                Label("New Space", systemImage: "plus")
            }
            .buttonStyle(.borderless)
            .padding(8)
            .help("New space from a folder")
        }
        .background(.background, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.sidebarFrames[windowID] = $0 }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Spaces")
    }
}

struct SidebarSpace: View {
    @ObservedObject var model: AppModel
    let windowID: String
    let space: SpaceInfo
    let active: Bool

    var body: some View {
        HStack(spacing: 4) {
            // A slot is kept with no dot, so the names line up.
            Color.clear.frame(width: 12, height: 12)
                .overlay { StatusDot(status: space.status, shown: model.statusDots) }
            if model.renamingSpace == space.id {
                InlineName(
                    text: space.name, placeholder: "Space name", error: model.spaceRenameError[space.id],
                    commit: { model.renameSpace(space.id, to: $0) }, cancel: { model.endRenaming() }
                )
            } else {
                Text(space.name)
                    .fontWeight(active ? .semibold : .regular)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                    // Double-click renames in place; the single click shows the space.
                    .onTapGesture(count: 2) { model.beginRename(space: space.id) }
                    .onTapGesture { model.show(space: space.id, in: windowID) }
                    .help(space.cwd)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(space.name)
                    .accessibilityAddTraits(active ? [.isButton, .isSelected] : .isButton)
                    .accessibilityAction(named: "Rename Space…") { model.beginRename(space: space.id) }
                if let status = space.status, model.statusDots.contains(status.rank), let caption = status.caption {
                    Text(caption)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .fixedSize()
                }
                CloseSpaceButton(model: model, space: space)
            }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
        .background(active ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: RoundedRectangle(cornerRadius: 6))
        .contextMenu {
            Button("Rename Space…") { model.beginRename(space: space.id) }
        }
        .spaceDraggable(model, space: space.id)
    }
}

/// A name edited where it is shown: Return commits, Escape cancels, and a refusal reads under the
/// field. Used for a space's sidebar row and tab, and for a window's title.
struct InlineName: View {
    let text: String
    let placeholder: String
    var error: String?
    let commit: (String) -> Void
    let cancel: () -> Void
    @State private var draft = ""
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            TextField(placeholder, text: $draft)
                .textFieldStyle(.roundedBorder)
                .focused($focused)
                .onSubmit { commit(draft) }
                .onExitCommand { cancel() }
                .onAppear {
                    draft = text
                    focused = true
                }
                // Clicking away ends the edit as Escape does.
                .onChange(of: focused) { _, now in if !now { cancel() } }
            if let error {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption).foregroundStyle(.red).lineLimit(2)
            }
        }
    }
}
