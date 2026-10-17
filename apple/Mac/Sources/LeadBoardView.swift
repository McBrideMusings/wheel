import SwiftUI
import WheelKit

/// The lead card's header control: "▾ Board" opens Supervisors and For you, "▴ Collapse" closes.
struct BoardToggle: View {
    @ObservedObject var model: AppModel

    var body: some View {
        Button(model.leadBoardExpanded ? "▴ Collapse" : "▾ Board") {
            model.leadBoardExpanded.toggle()
        }
        .buttonStyle(.borderless)
        .font(ChatFont.meta)
        .help(model.leadBoardExpanded ? "Back to the conversation" : "Every supervisor, and what waits on you")
    }
}

/// "Inbox N": how many items in the lead's inbox are not closed.
struct InboxBadge: View {
    let count: Int

    var body: some View {
        Text("Inbox \(count)")
            .font(ChatFont.meta)
            .monospacedDigit()
            .foregroundStyle(count > 0 ? Color.accentColor : .secondary)
            .padding(.horizontal, 7)
            .padding(.vertical, 1)
            .overlay(Capsule().strokeBorder(count > 0 ? Color.accentColor.opacity(0.6) : Color.secondary.opacity(0.4)))
            .accessibilityLabel("Inbox, \(count) open")
    }
}

/// One line per supervisor above the chat: its dot, goal title, landed of total and a state word.
struct LeadPinned: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let rows = model.leadBoard.supervisors
        if !rows.isEmpty {
            VStack(alignment: .leading, spacing: 5) {
                ForEach(rows) { row in
                    if let pane = model.supervisorPanes[row.pane] {
                        PinnedSupervisorLine(summary: row, pane: pane)
                    }
                }
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            Divider()
        }
    }
}

/// What a summary says once its pane's board has answered: the pane model is observed so a
/// landing redraws the line, since `AppModel` itself does not change for it.
@MainActor private func counts(_ summary: SupervisorSummary, _ pane: SupervisorPaneModel) -> (landed: Int, total: Int) {
    pane.board.map { ($0.landed.count, $0.total) } ?? (summary.landed, summary.total)
}

struct PinnedSupervisorLine: View {
    let summary: SupervisorSummary
    @ObservedObject var pane: SupervisorPaneModel

    var body: some View {
        let state = pane.supervisor.state
        let c = counts(summary, pane)
        HStack(spacing: 6) {
            Image(systemName: SupervisorStyle.dot(state)).font(ChatFont.meta)
                .foregroundStyle(SupervisorStyle.color(state))
            Text(summary.title).font(ChatFont.line).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 4)
            Text("\(c.landed)/\(c.total)").font(ChatFont.meta).monospacedDigit().foregroundStyle(.secondary)
            Text(summary.stateWord).font(ChatFont.meta).foregroundStyle(.secondary).lineLimit(1).fixedSize()
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(summary.title), \(c.landed) of \(c.total) landed, \(summary.stateWord)")
    }
}

/// The full board in place of the transcript: a card per supervisor, then For you.
struct LeadExpandedBoard: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let board = model.leadBoard
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                Text("Supervisors").font(ChatFont.meta).fontWeight(.semibold).foregroundStyle(.secondary)
                if board.supervisors.isEmpty {
                    Text(LeadBoard.noSupervisors).font(ChatFont.line).foregroundStyle(.secondary)
                }
                ForEach(board.supervisors) { row in
                    if let pane = model.supervisorPanes[row.pane] {
                        SupervisorCard(summary: row, pane: pane)
                    }
                }
                Text("For you").font(ChatFont.meta).fontWeight(.semibold).foregroundStyle(.secondary).padding(.top, 6)
                if board.forYou.isEmpty {
                    Text("Nothing waits on you").font(ChatFont.line).foregroundStyle(.secondary)
                }
                ForEach(board.forYou) { item in
                    ForYouRow(item: item, spaceName: model.spaces.first { $0.id == item.source.space }?.name ?? item.source.space, supervisorTitle: board.supervisorTitle(for: item))
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct SupervisorCard: View {
    let summary: SupervisorSummary
    @ObservedObject var pane: SupervisorPaneModel

    var body: some View {
        let state = pane.supervisor.state
        let c = counts(summary, pane)
        let workers = pane.board?.info.running.count ?? summary.workers.count
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Image(systemName: SupervisorStyle.dot(state)).font(ChatFont.meta)
                    .foregroundStyle(SupervisorStyle.color(state))
                Text(summary.spaceName).font(ChatFont.meta).foregroundStyle(.secondary)
                Text(summary.title).font(ChatFont.line).fontWeight(.semibold).lineLimit(1).truncationMode(.tail)
                Spacer(minLength: 4)
                Text(summary.stateWord).font(ChatFont.meta).foregroundStyle(.secondary).fixedSize()
            }
            ProgressView(value: c.total == 0 ? 0 : Double(c.landed) / Double(c.total))
            HStack(spacing: 8) {
                Text("\(c.landed) of \(c.total) landed").font(ChatFont.meta).foregroundStyle(.secondary)
                Text("\(workers) of \(pane.supervisor.workers) workers").font(ChatFont.meta).foregroundStyle(.secondary)
                Spacer(minLength: 4)
                let inbox = pane.board?.forYou ?? summary.forYou
                if inbox > 0 {
                    Label("\(inbox) for you", systemImage: "diamond.fill").font(ChatFont.meta).foregroundStyle(.orange)
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .accessibilityElement(children: .combine)
    }
}

struct ForYouRow: View {
    let item: InboxItem
    let spaceName: String
    var supervisorTitle: String?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: item.glyph).font(ChatFont.meta)
                .foregroundStyle(item.kind == .turnEnded ? AnyShapeStyle(.secondary) : AnyShapeStyle(.orange))
                .frame(width: 14)
            VStack(alignment: .leading, spacing: 2) {
                Text("\(supervisorTitle ?? spaceName) · \(item.headline)").font(ChatFont.line).fontWeight(.semibold)
                if let detail = item.detail, !detail.isEmpty {
                    Text(detail).font(ChatFont.meta).foregroundStyle(.secondary).lineLimit(4)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}
