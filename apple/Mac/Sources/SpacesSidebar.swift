import SwiftUI
import WheelKit
import WheelUI

/// The sidebar: every space, one row each. A click anywhere on a row but its close button shows it.
struct SpacesSidebar: View {
    @ObservedObject var model: AppModel
    let windowID: String

    /// Where the dragged space would land in this sidebar: the number of rows above the insertion
    /// bar, counted in the list as it stands. No row moves while the drag runs.
    private var insertion: Int? {
        if let bar = model.spaceDragInsertion, bar.window == windowID { bar.index } else { nil }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    let held = model.spaces(in: windowID)
                    ForEach(Array(held.enumerated()), id: \.element.id) { offset, space in
                        SidebarSpace(model: model, windowID: windowID, space: space, active: space.id == model.active(in: windowID))
                            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.rowFrames[windowID, default: [:]][space.id] = $0 }
                            .overlay(alignment: .top) { if insertion == offset { InsertionBar().offset(y: -2) } }
                            .overlay(alignment: .bottom) { if offset == held.count - 1, insertion == held.count { InsertionBar().offset(y: 2) } }
                    }
                }
                .padding(6)
                .animation(model.slide, value: model.spaces(in: windowID).map(\.id))
            }
            Divider()
            Button { model.openNewSpacePalette() } label: {
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
            PaneStack(space: space)
                .foregroundStyle(active ? .primary : .secondary)
            // A slot is kept with no dot, so the names line up.
            SpaceDots(space: space, shown: model.statusDots)
                .frame(minWidth: 12, minHeight: 12)
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
                    .help(space.cwd)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(space.name)
                    .accessibilityAddTraits(active ? [.isButton, .isSelected] : .isButton)
                    .accessibilityAction { model.tapSpace(space.id, in: windowID) }
                    .accessibilityAction(named: "Rename Space…") { model.beginRename(space: space.id) }
                BackFromLittleButton(model: model, space: space.id, windowID: windowID)
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
        .opacity(model.isDimmed(space: space.id) ? 0.5 : 1)
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
        .background(active ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: RoundedRectangle(cornerRadius: 6))
        .opacity(model.isDragged(space.id) ? 0.4 : 1)
        .spacePressTarget(model, windowID: windowID, space: space.id)
        .contextMenu { SpaceMenuItems(model: model, space: space.id, window: windowID) }
    }
}

/// The accent line between two rows where the dragged space would land.
struct InsertionBar: View {
    var body: some View {
        Capsule()
            .fill(Color.accentColor)
            .frame(height: 3)
            .accessibilityLabel("Drop here")
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
