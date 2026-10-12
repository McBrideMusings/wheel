import GhosttyTerminal
import SwiftUI
import WheelKit

/// The Floating layout (DESIGN.md): the lead as a card on the left with a gap on every side,
/// the pane grid as a separate surface beside it. Stock macOS styling.
struct ContentView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 10) {
            VStack(spacing: 6) {
                LeadCard(model: model)
                ConnectionBadge(connection: model.connection)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 4)
            }
            .frame(width: 360)
            TerminalPane(model: model)
        }
        .padding(10)
        .frame(minWidth: 900, minHeight: 560)
        .background(Color(nsColor: .windowBackgroundColor))
    }
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

struct TerminalPane: View {
    @ObservedObject var model: AppModel

    var body: some View {
        TerminalSurfaceView(context: model.terminal)
            .onAppear {
                model.terminal.configuration = TerminalSurfaceOptions(backend: .inMemory(model.session))
            }
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
            .overlay(alignment: .center) {
                if !model.attached {
                    ProgressView(model.connection == .connected ? "Attaching…" : "Waiting for wheeld…")
                        .padding()
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
                }
            }
    }
}
