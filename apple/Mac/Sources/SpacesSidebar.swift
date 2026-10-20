import SwiftUI
import WheelKit

/// The sidebar: every space, each folding open to its panes. A click on a space shows it; a click
/// on a pane shows its space and gives the pane focus.
struct SpacesSidebar: View {
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(model.spaces) { space in
                        SidebarSpace(model: model, space: space, active: space.id == model.active)
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
    let space: SpaceInfo
    let active: Bool

    private var expanded: Bool { !model.collapsedSpaces.contains(space.id) }

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 4) {
                Button { model.setSpace(space.id, expanded: !expanded) } label: {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .frame(width: 14, height: 18)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel(expanded ? "Fold \(space.name)" : "Unfold \(space.name)")
                Button { model.focus(space: space.id) } label: {
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
            if expanded {
                ForEach(space.root.panes, id: \.id) { pane in
                    SidebarPane(model: model, paneId: pane.id, kind: pane.kind,
                                focused: active && pane.id == space.focus)
                }
            }
        }
    }
}

struct SidebarPane: View {
    @ObservedObject var model: AppModel
    let paneId: String
    let kind: PaneKind
    let focused: Bool

    var body: some View {
        Button { model.select(pane: paneId) } label: {
            HStack(spacing: 6) {
                Image(systemName: kind.icon).foregroundStyle(.secondary).frame(width: 14)
                Text(model.paneTitle(paneId, kind))
                    .fontWeight(focused ? .semibold : .regular)
                    .lineLimit(1)
                Spacer(minLength: 4)
                if case let .agent(session) = kind, let chat = model.chats[session] {
                    AgentStatus(model: model, chat: chat)
                }
            }
            .font(.callout)
            .padding(.leading, 24)
            .padding(.trailing, 6)
            .padding(.vertical, 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(focused ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear), in: RoundedRectangle(cornerRadius: 5))
        .accessibilityAddTraits(focused ? .isSelected : [])
    }
}

/// An agent pane's state on its sidebar row: what it is doing, or that it waits on the person.
struct AgentStatus: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        let label = model.stateLabel(chat)
        Text(label)
            .font(.caption)
            .foregroundStyle(label.hasPrefix("Waiting on you") ? Color.orange : Color.secondary)
            .lineLimit(1)
    }
}
