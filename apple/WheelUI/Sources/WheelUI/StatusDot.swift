import SwiftUI
import WheelKit

/// A pane's or space's status as a mark: a 9pt dot (orange needs you, a hollow red ring stopped, green idle) or,
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
        case .stopped: Circle().strokeBorder(Color.red, lineWidth: 1.8).frame(width: 9, height: 9)
        case .idle: Circle().fill(Color.green).frame(width: 9, height: 9)
        }
    }
}

/// A space's agents as marks: one per agent pane, most urgent first, overlapping a little, at most
/// `SpaceInfo.agentDotLimit` and then `+N`. A space whose panes sent no statuses but which has one of
/// its own (a daemon that predates per-pane statuses) draws that one.
public struct SpaceDots: View {
    let space: SpaceInfo
    let shown: Set<StatusRank>

    public init(space: SpaceInfo, shown: Set<StatusRank>) {
        self.space = space
        self.shown = shown
    }

    public var body: some View {
        let agents = space.agentStatuses(shown: shown)
        if space.paneStatuses.isEmpty {
            StatusDot(status: space.status, shown: shown)
        } else if !agents.marks.isEmpty {
            HStack(spacing: 3) {
                // Each mark gets the arc's 12pt slot, so a dot sits centred in it and two arcs
                // touch rather than merge; the slots overlap by 2pt.
                HStack(spacing: -2) {
                    ForEach(Array(agents.marks.enumerated()), id: \.offset) { index, status in
                        StatusDot(status: status, shown: shown)
                            .frame(width: 12, height: 12)
                            .zIndex(Double(agents.marks.count - index))
                    }
                }
                if agents.more > 0 {
                    Text("+\(agents.more)")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Self.label(agents))
        }
    }

    /// What VoiceOver reads: "Needs you, Working, 2 more".
    static func label(_ agents: (marks: [PaneStatus], more: Int)) -> String {
        var parts = agents.marks.map { $0.label }
        if agents.more > 0 { parts.append("\(agents.more) more") }
        return parts.joined(separator: ", ")
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
