import SwiftUI
import WheelKit

/// The sidebar: every space, one row each. A click on a space shows it.
struct SpacesSidebar: View {
    @ObservedObject var model: AppModel
    let windowID: String

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(model.spaces(in: windowID)) { space in
                        SidebarSpace(model: model, windowID: windowID, space: space, active: space.id == model.active(in: windowID))
                    }
                }
                .padding(6)
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
                CloseSpaceButton(model: model, space: space)
            }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
        .background(active ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: RoundedRectangle(cornerRadius: 6))
        .contextMenu {
            Button("Rename Space…") { model.beginRename(space: space.id) }
        }
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
