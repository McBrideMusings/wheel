import SwiftUI
import WheelKit

/// The window: the active space's pane grid, with the connection and the usage strip under it.
/// Stock macOS styling.
struct ContentView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var settings: SettingsModel
    /// The window this view fills (its record in `model.layout`).
    let windowID: String

    var body: some View {
        VStack(spacing: 6) {
            SpacesView(model: model, windowID: windowID)
            HStack(spacing: 12) {
                ConnectionBadge(connection: model.connection)
                UsageStrip(usage: model.usage)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(10)
        .frame(minWidth: 900, minHeight: 560)
        .background(Color(nsColor: .windowBackgroundColor))
        .overlay {
            if let palette = model.commandPalette, model.layout.focused == windowID {
                CommandPaletteOverlay(model: model, palette: palette)
            }
        }
        .toolbar {
            ToolbarItem(placement: .principal) {
                WindowTitle(model: model, windowID: windowID)
            }
            ToolbarItem(placement: .navigation) {
                let open = model.sidebarOpen(in: windowID)
                Button { model.setSidebar(open: !open, in: windowID) } label: {
                    Image(systemName: "sidebar.left")
                }
                .help(open ? "Hide the sidebar; spaces become tabs" : "Show the sidebar")
                .accessibilityLabel(open ? "Hide sidebar" : "Show sidebar")
            }
        }
        // The debug endpoint's stand-in for the app menu's Settings…
        .onChange(of: settings.openRequest) { if model.layout.focused == windowID { openSettings() } }
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

/// The window's title in the title bar: its name in bold with the active space beside it, or the
/// space's name alone when the window has none. Double-clicking edits the name; an empty name
/// clears it.
struct WindowTitle: View {
    @ObservedObject var model: AppModel
    let windowID: String

    var body: some View {
        let record = model.layout.window(windowID)
        let space = record?.active.flatMap(model.spaceName)
        if model.renamingWindow == windowID {
            InlineName(
                text: record?.name ?? "", placeholder: space ?? "Window name", error: nil,
                commit: { model.commitRename(window: windowID, to: $0) }, cancel: { model.endRenaming() }
            )
            .frame(width: 220)
        } else {
            HStack(spacing: 6) {
                if let name = record?.name {
                    Text(name).fontWeight(.bold)
                    if let space { Text(space).foregroundStyle(.secondary) }
                } else {
                    Text(space ?? "Wheel").fontWeight(.semibold)
                }
            }
            .lineLimit(1)
            .contentShape(Rectangle())
            .onTapGesture(count: 2) { model.beginRename(window: windowID) }
            .help("Double-click to rename the window")
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Window \(model.layout.window(windowID).map { model.title(of: $0) } ?? "")")
            .accessibilityAction(named: "Rename Window…") { model.beginRename(window: windowID) }
        }
    }
}
