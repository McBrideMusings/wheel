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
            Button { model.show(space: space.id, in: windowID) } label: {
                Text(space.name)
                    .fontWeight(active ? .semibold : .regular)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(space.cwd)
            .accessibilityAddTraits(active ? .isSelected : [])
            CloseSpaceButton(model: model, space: space)
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
        .background(active ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: RoundedRectangle(cornerRadius: 6))
    }
}
