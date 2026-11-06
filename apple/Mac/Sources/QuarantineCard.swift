import AppKit
import SwiftUI
import WheelKit

/// Stands in for a space or a pane that was being drawn when Wheel last quit unexpectedly, so the
/// rest of the app opens. "Open anyway" draws it for real: if it crashes again, the next launch
/// holds it out again.
struct QuarantineCard: View {
    let title: String
    let detail: String
    let open: () -> Void

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle")
                .font(.largeTitle)
                .foregroundStyle(.secondary)
            Text("Wheel quit unexpectedly while drawing \(title)")
                .font(.headline)
                .multilineTextAlignment(.center)
            Text(detail)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            HStack {
                if let report = Quarantine.shared.report {
                    Button("Show Crash Report") { NSWorkspace.shared.activateFileViewerSelecting([report]) }
                }
                Button("Open Anyway", action: open)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background)
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator, lineWidth: 1))
    }
}

/// A pane's view, or its card while the last crash names it.
struct PaneOrCard: View {
    var model: AppModel
    let paneId: String
    let space: String
    let kind: PaneKind
    let focused: Bool

    var body: some View {
        if Quarantine.shared.isolates(pane: paneId) {
            QuarantineCard(title: "pane \(paneId)", detail: "Its other panes and spaces opened as usual. This pane is a \(kind.name) pane.") {
                Quarantine.shared.release(pane: paneId)
            }
        } else {
            PaneView(model: model, paneId: paneId, space: space, kind: kind, focused: focused)
        }
    }
}
