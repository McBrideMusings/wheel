import SwiftUI
import WheelKit

/// The Floating layout (DESIGN.md): the lead as a card on the left with a gap on every side,
/// the pane grid as a separate surface beside it. Stock macOS styling.
struct ContentView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var settings: SettingsModel
    @ObservedObject var sidebar: SidebarLayout

    var body: some View {
        GeometryReader { geo in
            let total = geo.size.width - 20
            let split = sidebar.splitWidth(in: total)
            let gap: Double = sidebar.mode == .hidden ? 0 : 10
            let leadWidth: Double = switch sidebar.mode {
            case .split: split
            case .hidden: 0
            case .full: total
            }
            // The grid keeps its split-layout size while the sidebar fills the window, so its
            // terminals are not resized to nothing; it slides out of the window instead.
            let gridWidth = sidebar.mode == .hidden ? total : total - split - 10
            HStack(spacing: 0) {
                VStack(spacing: 6) {
                    LeadCard(model: model, chat: model.lead)
                        .overlay {
                            if model.highlightedManager == .lead {
                                RoundedRectangle(cornerRadius: 10).strokeBorder(Color.accentColor, lineWidth: 3).allowsHitTesting(false)
                            }
                        }
                    ConnectionBadge(connection: model.connection)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 4)
                }
                .frame(width: sidebar.mode == .full ? total : split)
                .frame(width: leadWidth, alignment: .leading)
                .clipped()
                .allowsHitTesting(sidebar.mode != .hidden)
                .accessibilityHidden(sidebar.mode == .hidden)
                if sidebar.mode == .split {
                    SidebarHandle(sidebar: sidebar)
                } else {
                    Color.clear.frame(width: gap)
                }
                SpacesView(model: model)
                    .frame(width: gridWidth)
                    .opacity(sidebar.mode == .full ? 0 : 1)
                    .allowsHitTesting(sidebar.mode != .full)
                    .accessibilityHidden(sidebar.mode == .full)
            }
            .frame(width: total, alignment: .leading)
            .clipped()
            .padding(10)
        }
        .toolbar {
            ToolbarItemGroup {
                Button { sidebar.toggleHidden(animated: true) } label: {
                    Label(sidebar.mode == .split ? "Hide Lead" : (sidebar.mode == .full ? "Show Spaces" : "Show Lead"), systemImage: "sidebar.leading")
                }
                .help(sidebar.mode == .split ? "Hide the lead sidebar" : (sidebar.mode == .full ? "Show the spaces beside the lead" : "Show the lead sidebar"))
                Button { sidebar.toggleFull(animated: true) } label: {
                    Label(sidebar.mode == .full ? "Restore Lead" : "Expand Lead",
                          systemImage: sidebar.mode == .full ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right")
                }
                .help(sidebar.mode == .full ? "Show the spaces beside the lead" : "Expand the lead to fill the window")
            }
        }
        .frame(minWidth: 900, minHeight: 560)
        .background(Color(nsColor: .windowBackgroundColor))
        .sheet(isPresented: Binding(get: { model.unlockPrompt != nil }, set: { if !$0 { model.cancelUnlock() } })) {
            if let pane = model.unlockPrompt { UnlockSheet(model: model, pane: pane) }
        }
        // The debug endpoint's stand-in for the app menu's Settings…
        .onChange(of: settings.openRequest) { openSettings() }
    }

    @Environment(\.openSettings) private var openSettings
}

struct ConnectionBadge: View {
    let connection: DaemonClient.Connection

    var body: some View {
        switch connection {
        case .connected:
            Label("Connected to wheeld", systemImage: "checkmark.circle")
                .font(.caption).foregroundStyle(.secondary)
        case .connecting:
            Label("Connecting to wheeld…", systemImage: "arrow.triangle.2.circlepath")
                .font(.caption).foregroundStyle(.secondary)
        case let .unreachable(reason):
            Label("wheeld unreachable: \(reason). Retrying every 2 seconds.", systemImage: "exclamationmark.triangle")
                .font(.caption).foregroundStyle(.orange)
        }
    }
}
