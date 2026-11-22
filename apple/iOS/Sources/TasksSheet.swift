import SwiftUI
import WheelKit

/// Orchestrator mode's Tasks sheet: each global orchestrator task, newest first, with its status, the inbox events
/// naming its agents under it, and Done while it is open. A push's tap opens it scrolled to its task.
struct TasksSheet: View {
    let model: PhoneModel

    var body: some View {
        ScrollViewReader { scroller in
            ScrollView {
                VStack(spacing: 10) {
                    if model.tasks.isEmpty {
                        Text("No tasks yet")
                            .font(.subheadline).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity).padding(.vertical, 24)
                            .accessibilityIdentifier("no tasks")
                    }
                    ForEach(model.tasks) { task in
                        TaskTile(model: model, task: task, focused: task.id == model.tasksFocus).id(task.id)
                    }
                }
                .padding(.horizontal, 16).padding(.vertical, 12)
            }
            .accessibilityIdentifier("tasks list")
            .onChange(of: model.tasks.map(\.id)) { scrollToFocus(scroller) }
            .onAppear { scrollToFocus(scroller) }
        }
    }

    private func scrollToFocus(_ scroller: ScrollViewProxy) {
        guard let focus = model.tasksFocus, model.tasks.contains(where: { $0.id == focus }) else { return }
        scroller.scrollTo(focus, anchor: .top)
    }
}

/// One task: what was asked, its status, its newest inbox events, and Done.
private struct TaskTile: View {
    let model: PhoneModel
    let task: OrchestratorTask
    let focused: Bool

    /// The inbox events a tile lists: the newest.
    private static let shown = 5

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(task.asked).font(.body.weight(.semibold)).lineLimit(2)
                Spacer(minLength: 8)
                Text(task.statusText).font(.subheadline)
                    .foregroundStyle(task.status == "needs-decision" ? Color.orange : Color.secondary)
                    .accessibilityIdentifier("task status \(task.id)")
            }
            ForEach(Array(task.events.suffix(Self.shown).enumerated()), id: \.offset) { _, event in
                Text(event.text).font(.footnote).foregroundStyle(.secondary).lineLimit(2)
            }
            if let error = task.pushError {
                Text("No push sent: \(error)").font(.footnote).foregroundStyle(.secondary).lineLimit(2)
            }
            if !task.closed && model.canMarkTaskDone {
                Button("Done") { model.markTaskDone(task.id) }
                    .buttonStyle(.bordered).controlSize(.small)
                    .accessibilityIdentifier("task done \(task.id)")
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12).fill(Color(.secondarySystemBackground)))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color.accentColor, lineWidth: focused ? 2 : 0))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("task \(task.id)")
    }
}
