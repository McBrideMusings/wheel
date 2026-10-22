import SwiftUI
import WheelKit

/// A pane's or space's status as a mark: a 9pt dot (orange needs you, red stopped, green idle) or,
/// while a turn runs, a 12pt blue arc turning once a second. It draws nothing for no status or a
/// rank `status.dots` leaves out.
public struct StatusDot: View {
    let status: PaneStatus?
    let shown: Set<StatusRank>

    public init(status: PaneStatus?, shown: Set<StatusRank>) {
        self.status = status
        self.shown = shown
    }

    public var body: some View {
        if let status, shown.contains(status.rank) {
            mark(status)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(status.label)
        }
    }

    @ViewBuilder
    private func mark(_ status: PaneStatus) -> some View {
        switch status {
        case .working: WorkingArc()
        case .needsYou: Circle().fill(Color.orange).frame(width: 9, height: 9)
        case .stopped: Circle().fill(Color.red).frame(width: 9, height: 9)
        case .idle: Circle().fill(Color.green).frame(width: 9, height: 9)
        }
    }
}

/// The 3/4 arc, turning once a second. Its own view so each time a pane starts working the arc
/// starts from rest, and it stands still under Reduce Motion.
private struct WorkingArc: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var turning = false

    var body: some View {
        Circle()
            .trim(from: 0, to: 0.75)
            .stroke(Color.blue, style: StrokeStyle(lineWidth: 1.8, lineCap: .round))
            .frame(width: 12, height: 12)
            .rotationEffect(.degrees(turning && !reduceMotion ? 360 : 0))
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.linear(duration: 1).repeatForever(autoreverses: false)) { turning = true }
            }
    }
}
