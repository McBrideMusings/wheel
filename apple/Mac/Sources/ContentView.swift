import SwiftUI
import WheelKit

/// The Floating layout (DESIGN.md): the lead as a card on the left with a gap on every side,
/// the pane grid as a separate surface beside it. Stock macOS styling.
struct ContentView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var settings: SettingsModel

    var body: some View {
        HStack(spacing: 10) {
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
            .frame(width: 360)
            SpacesView(model: model)
        }
        .padding(10)
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
