import SwiftUI
import WheelKit

/// The window: the active space's pane grid, with the connection and the usage strip under it.
/// Stock macOS styling.
struct ContentView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var settings: SettingsModel

    var body: some View {
        VStack(spacing: 6) {
            SpacesView(model: model)
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
            if let palette = model.commandPalette {
                CommandPaletteOverlay(model: model, palette: palette)
            }
        }
        .toolbar {
            ToolbarItem(placement: .navigation) {
                Button { CommandActions.run("toggle-sidebar", model) } label: {
                    Image(systemName: "sidebar.left")
                }
                .help(model.sidebarOpen ? "Hide the sidebar; spaces become tabs" : "Show the sidebar")
                .accessibilityLabel(model.sidebarOpen ? "Hide sidebar" : "Show sidebar")
            }
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
