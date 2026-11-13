import SwiftUI
import WheelKit

/// `● running 40:12` after a transcript line whose call started a background task that still runs:
/// green, amber once its output has been quiet five minutes. Draws nothing for any other line.
struct RunningMark: View {
    @Environment(\.chatAppearance) private var look
    @ObservedObject var chat: AgentChat
    /// The calls the line stands for: one for a tool line, every call for a group line.
    let calls: [String]

    var body: some View {
        if let task = chat.footer?.tasks.first(where: { task in task.call.map(calls.contains) ?? false }) {
            TimelineView(.periodic(from: .now, by: 1)) { timeline in
                let quiet = task.quiet(at: timeline.date) != nil
                HStack(spacing: 4) {
                    Circle().fill(quiet ? Color.orange : Color.green).frame(width: 6, height: 6)
                    Text("running \(RunningTask.clock(task.elapsed(at: timeline.date)))")
                        .font(look.meta.monospacedDigit())
                        .foregroundStyle(quiet ? Color.orange : Color.green)
                }
                .fixedSize()
                .help(quiet ? "Running; no output for over five minutes" : "Running in the background")
            }
        }
    }
}

extension AgentChat {
    /// The transcript item holding the tool call `call`, top-level or inside a group.
    public func item(holdingCall call: String) -> Int? {
        for item in items {
            switch item {
            case let .row(row) where row.row.toolCall == call:
                return item.id
            case let .tools(group), let .work(group):
                if group.rows.contains(where: { $0.row.toolCall == call }) { return item.id }
            default:
                continue
            }
        }
        return nil
    }
}
