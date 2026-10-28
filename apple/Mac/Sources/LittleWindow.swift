import AppKit
import SwiftUI
import WheelKit
import WheelUI

extension AppModel {
    /// A tab or row draws dimmed while its space is in a little window, and while it is being
    /// dragged over the desktop, where it is about to become one.
    func isDimmed(space: String) -> Bool {
        layout.little(showing: space) != nil || (spaceDrag?.space == space && spaceDrag?.target == .desktop)
    }

    /// What the back icon says: the window it returns the space to.
    func backHelp(window id: String?) -> String {
        guard let id else { return "Back to the background" }
        guard let record = layout.window(id) else { return "Back into its window" }
        return "Back into the \(title(of: record)) window"
    }
}

/// The back icon on a dimmed tab or row: closes the little window, and the tab shows the grid again.
struct BackFromLittleButton: View {
    @ObservedObject var model: AppModel
    let space: String
    let windowID: String

    var body: some View {
        if model.layout.little(showing: space) != nil {
            Button { model.bringBack(space: space) } label: {
                Image(systemName: "arrow.uturn.backward").font(.caption2)
            }
            .buttonStyle(.borderless)
            .help(model.backHelp(window: windowID))
            .accessibilityLabel("Bring \(model.spaceName(space) ?? "space") back")
        }
    }
}

/// What a dimmed tab shows in place of the grid: the space is in a little window.
struct LittlePlaceholder: View {
    @ObservedObject var model: AppModel
    let space: SpaceInfo

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "square.on.square").font(.system(size: 44)).foregroundStyle(.secondary)
            Text(space.name).font(.headline)
            Text("This space is open in a little window.").foregroundStyle(.secondary)
            HStack {
                Button("Go to Little Window") {
                    if let little = model.layout.little(showing: space.id) { model.goToLittle(little.id) }
                }
                .keyboardShortcut(.defaultAction)
                Button("Bring Back") { model.bringBack(space: space.id) }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(space.name) is open in a little window")
    }
}

/// A little window's content: a 30 pt header strip over one space's pane grid.
struct LittleWindowView: View {
    @ObservedObject var model: AppModel
    let littleID: String

    static let headerHeight: CGFloat = 30

    var body: some View {
        let little = model.layout.little(littleID)
        let space = little.flatMap { l in model.spaces.first { $0.id == l.space } }
        VStack(spacing: 0) {
            LittleHeader(model: model, little: little, space: space)
                .frame(height: Self.headerHeight)
            Divider()
            if let space {
                SpaceGrid(model: model, space: space).padding(8).clipped()
            } else {
                Spacer()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        // The header strip is the title bar's height: the traffic lights sit in it.
        .ignoresSafeArea(.container, edges: .top)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

private struct LittleHeader: View {
    @ObservedObject var model: AppModel
    let little: LittleWindow?
    let space: SpaceInfo?

    var body: some View {
        HStack(spacing: 8) {
            // The traffic lights sit over this gap.
            Color.clear.frame(width: 64, height: 1)
            if let space {
                Text(space.name).fontWeight(.semibold).lineLimit(1)
                Text("\(space.root.panes.count)").font(.caption).foregroundStyle(.secondary)
                    .accessibilityLabel("\(space.root.panes.count) panes")
                SpaceDots(space: space, shown: model.statusDots)
            }
            Spacer(minLength: 8)
            // Pinning is drawn here and works in wheel-2wmu.
            Button {} label: { Image(systemName: "pin") }
                .buttonStyle(.borderless)
                .disabled(true)
                .help("Pin on top")
                .accessibilityLabel("Pin")
            Button { if let little { model.closeLittle(little.id) } } label: { Image(systemName: "arrow.uturn.backward") }
                .buttonStyle(.borderless)
                .help(model.backHelp(window: little?.owner))
                .accessibilityLabel("Back")
        }
        .padding(.horizontal, 10)
        .frame(maxWidth: .infinity)
    }
}
