import SwiftUI
import WheelKit

/// What one supervisor pane keeps: the supervisor as the layout holds it, and what the
/// `supervisor_*` replies last said about its workers, decisions and inbox. The board is worked
/// out from those alone (ADR-0001: the daemon owns the state, this only draws it).
@MainActor
final class SupervisorPaneModel: ObservableObject {
    let pane: String
    @Published var supervisor: Supervisor
    @Published var info: SupervisorInfo?
    @Published var events: [LoopEvent] = [] {
        didSet {
            // A landing stays counted after the daemon's bounded event list drops it.
            var seen = Set(landedSeen.map(\.bead))
            for event in events where event.kind == .landed {
                if let bead = event.bead, seen.insert(bead).inserted { landedSeen.append(LandedBead(bead: bead, detail: event.detail)) }
            }
        }
    }
    private var landedSeen: [LandedBead] = []
    /// When the board last asked the daemon; the "Stalled 6m" minutes count from it.
    @Published var now = Date()
    @Published var inbox: [SupervisorInboxItem] = []
    /// The full board is open; typing in the composer closes it.
    @Published var expanded = false
    /// The worker session the pane shows in place of the supervisor, if one is open.
    @Published var openWorker: String?
    /// The daemon's refusal of the last control the person pressed.
    @Published var error: String?

    init(pane: String, supervisor: Supervisor) {
        self.pane = pane
        self.supervisor = supervisor
    }

    /// The board, once the daemon has told us about the loop. The layout's supervisor is newer
    /// than the poll's copy of it, so its state and queue are the ones drawn.
    var board: SupervisorBoard? {
        guard let info else { return nil }
        return SupervisorBoard(
            info: SupervisorInfo(space: info.space, pane: info.pane, supervisor: supervisor, agent: info.agent, running: info.running, inbox: info.inbox),
            events: events,
            landedBefore: landedSeen,
            inbox: inbox
        )
    }
}

/// The pane title: a dot for the state, "Supervisor", the goal, and play or pause.
struct SupervisorTitle: View {
    @ObservedObject var model: AppModel
    @ObservedObject var pane: SupervisorPaneModel

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: SupervisorStyle.dot(pane.supervisor.state)).foregroundStyle(SupervisorStyle.color(pane.supervisor.state))
                .accessibilityLabel(SupervisorStyle.stateName(pane.supervisor.state))
            Text("Supervisor")
            Text(pane.supervisor.scope).font(.caption.monospaced()).foregroundStyle(.secondary)
            PlayPause(model: model, pane: pane)
        }
    }
}

enum SupervisorStyle {
    static func dot(_ state: SupervisorRun) -> String {
        switch state {
        case .running: "circle.fill"
        case .paused, .held: "pause.circle.fill"
        case .stopped: "stop.circle.fill"
        }
    }

    static func color(_ state: SupervisorRun) -> Color {
        switch state {
        case .running: .green
        case .paused: .secondary
        case .held: .orange
        case .stopped: .secondary
        }
    }

    static func stateName(_ state: SupervisorRun) -> String {
        switch state {
        case .running: "Running"
        case .paused: "Paused"
        case .held: "Held at the usage cap"
        case .stopped: "Stopped"
        }
    }
}

/// ❚❚ moves running to paused, ▶ moves back; a held supervisor resumes the same way.
struct PlayPause: View {
    @ObservedObject var model: AppModel
    @ObservedObject var pane: SupervisorPaneModel

    var body: some View {
        let state = pane.supervisor.state
        Button {
            if state == .running { model.pauseSupervisor(pane.pane) } else { model.resumeSupervisor(pane.pane) }
        } label: {
            Image(systemName: state == .running ? "pause.fill" : "play.fill")
        }
        .buttonStyle(.borderless)
        .disabled(state == .stopped || model.connection != .connected)
        .help(state == .running ? "Pause: workers finish their turn, nothing new starts" : "Resume")
        .accessibilityLabel(state == .running ? "Pause supervisor" : "Resume supervisor")
    }
}

/// A supervisor pane: the board above the supervisor's own chat.
struct SupervisorBody: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @ObservedObject var pane: SupervisorPaneModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        // The supervisor stays in the view while a worker is open over it, so ← Supervisor comes
        // back to the same scroll position and the same draft.
        ZStack {
            supervisor
                .opacity(pane.openWorker == nil ? 1 : 0)
                .allowsHitTesting(pane.openWorker == nil)
                .accessibilityHidden(pane.openWorker != nil)
            if let session = pane.openWorker {
                WorkerDrillIn(model: model, paneId: paneId, pane: pane, session: session)
                    .background(.background)
                    .id(session)
            }
        }
    }

    private var supervisor: some View {
        VStack(spacing: 0) {
            if let board = pane.board {
                if pane.expanded {
                    ExpandedBoard(model: model, pane: pane, board: board)
                } else {
                    PinnedBoard(model: model, pane: pane, board: board)
                }
                Divider()
            } else if let error = pane.supervisor.planError {
                Label(error, systemImage: "exclamationmark.triangle").font(.caption).foregroundStyle(.orange).padding(10)
                Divider()
            }
            TranscriptView(model: model, chat: chat, empty: "Message the supervisor.")
            Composer(
                placeholder: "Message the supervisor",
                draft: Binding(get: { chat.draft }, set: { model.supervisorTyped(pane: paneId, draft: $0) }),
                error: chat.error,
                enabled: model.connection == .connected,
                send: { model.send(chat) },
                pickFiles: pickFiles,
                onFocus: { model.paneFocused(pane: paneId) },
                focusRequest: chat.composerFocusRequest
            )
            .padding(10)
        }
        .simultaneousGesture(TapGesture().onEnded { model.paneFocused(pane: paneId) })
    }
}

/// The week meter: the week's usage as a bar, with the supervisor's cap marked on it. The daemon
/// reports no reading of the usage yet, so without one no bar is filled and the cap is text.
/// Amber while held.
struct WeekMeter: View {
    let board: SupervisorBoard

    var body: some View {
        let held = board.supervisor.state == .held
        let capText = board.supervisor.cap.map { "Cap \($0)%" } ?? "No usage cap"
        if let usage = board.weekUsage {
            let percent = Int((usage * 100).rounded())
            HStack(spacing: 6) {
                Text("Week").font(.caption).foregroundStyle(.secondary)
                ProgressView(value: min(max(usage, 0), 1)).tint(held ? .orange : .accentColor).frame(width: 70)
                    .overlay(alignment: .leading) {
                        if let cap = board.weekCap {
                            Rectangle().fill(.primary).frame(width: 2, height: 10).offset(x: 70 * min(cap, 1) - 1)
                        }
                    }
                Text("\(percent)% · \(capText)").font(.caption.monospacedDigit())
                    .foregroundStyle(held ? .orange : .secondary)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Week usage \(percent) percent, \(capText)\(held ? ", held" : "")")
        } else {
            Text("Week usage unknown · \(capText)").font(.caption.monospacedDigit())
                .foregroundStyle(held ? .orange : .secondary)
                .accessibilityLabel("Week usage unknown, \(capText)\(held ? ", held" : "")")
        }
    }
}

struct WorkersMenu: View {
    @ObservedObject var model: AppModel
    @ObservedObject var pane: SupervisorPaneModel

    var body: some View {
        Menu {
            ForEach(1...8, id: \.self) { n in
                Button { model.setSupervisorWorkers(pane.pane, n) } label: {
                    if n == pane.supervisor.workers { Label("\(n)", systemImage: "checkmark") } else { Text("\(n)") }
                }
            }
        } label: {
            Text("Workers \(pane.supervisor.workers) ▾")
        }
        .menuStyle(.button)
        .menuIndicator(.hidden)
        .controlSize(.small)
        .fixedSize()
        .disabled(pane.supervisor.state == .stopped)
    }
}

struct CapMenu: View {
    @ObservedObject var model: AppModel
    @ObservedObject var pane: SupervisorPaneModel

    var body: some View {
        Menu {
            Button("No cap") { model.setSupervisorCap(pane.pane, nil) }
            ForEach([50, 60, 70, 80, 90, 100], id: \.self) { cap in
                Button { model.setSupervisorCap(pane.pane, cap) } label: {
                    if cap == pane.supervisor.cap { Label("\(cap)%", systemImage: "checkmark") } else { Text("\(cap)%") }
                }
            }
        } label: {
            Text("Cap \(pane.supervisor.cap.map { "\($0)%" } ?? "none") ▾")
        }
        .menuStyle(.button)
        .menuIndicator(.hidden)
        .controlSize(.small)
        .fixedSize()
        .disabled(pane.supervisor.state == .stopped)
    }
}

struct WorkerRow: View {
    let line: WorkerLine
    var note: String?
    /// Open the worker in the supervisor's pane; nil while the connection is down.
    var open: (() -> Void)?

    var body: some View {
        HStack(spacing: 8) {
            Text("L\(line.lane)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            Text(line.worker.bead).font(.caption.monospaced())
            Text(line.worker.title).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 4)
            if let note { Text(note).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
            Label(line.word, systemImage: line.glyph).font(.caption)
                .foregroundStyle(line.alert ? AnyShapeStyle(.red) : line.word == "Working" ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.secondary))
                .lineLimit(1).fixedSize()
            if line.inPane {
                Label(WorkerLine.inAPaneText, systemImage: "rectangle.on.rectangle").font(.caption).foregroundStyle(.secondary)
                    .lineLimit(1).fixedSize()
            } else {
                Button("Open") { open?() }.buttonStyle(.bordered).controlSize(.small).disabled(open == nil)
                    .accessibilityLabel("Open worker \(line.worker.bead)")
            }
        }
        .font(.callout)
        .accessibilityElement(children: .combine)
    }
}

/// One line per worker above the chat, with the progress bar and the way into the full board.
struct PinnedBoard: View {
    @ObservedObject var model: AppModel
    @ObservedObject var pane: SupervisorPaneModel
    let board: SupervisorBoard

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                ProgressView(value: board.progress).frame(width: 90)
                Text(board.progressText).font(.callout)
                Spacer(minLength: 4)
                WorkersMenu(model: model, pane: pane)
                WeekMeter(board: board)
            }
            if let hold = board.holdText { Text(hold).font(.caption).foregroundStyle(.orange) }
            ForEach(board.lines(now: pane.now)) { line in
                WorkerRow(line: line, open: model.connection == .connected ? { model.openWorker(pane.pane, session: line.worker.session) } : nil)
            }
            Button("▾ Expand board · queued · landed · events") { pane.expanded = true }
                .buttonStyle(.borderless).font(.caption)
            if let error = pane.error { Text(error).font(.caption).foregroundStyle(.red) }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The full board: goal, progress, controls, the week meter, Workers, Queued, Landed and Events.
struct ExpandedBoard: View {
    @ObservedObject var model: AppModel
    @ObservedObject var pane: SupervisorPaneModel
    let board: SupervisorBoard

    private func section(_ title: String, count: Int? = nil) -> some View {
        Text(count.map { "\(title) \($0)" } ?? title).font(.caption.weight(.semibold)).foregroundStyle(.secondary).padding(.top, 4)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        Text("Goal").font(.caption).foregroundStyle(.secondary)
                        Text(board.supervisor.scope).font(.callout.monospaced())
                        if board.forYou > 0 { Text("\(board.forYou) for you").font(.caption.weight(.semibold)).foregroundStyle(.orange) }
                    }
                    HStack(spacing: 10) {
                        ProgressView(value: board.progress).frame(width: 120)
                        Text(board.progressText).font(.callout)
                    }
                    HStack(spacing: 12) {
                        PlayPause(model: model, pane: pane)
                        WorkersMenu(model: model, pane: pane)
                        CapMenu(model: model, pane: pane)
                        WeekMeter(board: board)
                    }
                    if let hold = board.holdText { Text(hold).font(.caption).foregroundStyle(.orange) }
                    if let error = pane.error { Text(error).font(.caption).foregroundStyle(.red) }

                    section("Workers", count: board.info.running.count)
                    ForEach(board.lines(now: pane.now)) { line in
                        WorkerRow(line: line, note: "\(line.worker.model) · \(note(for: line.worker.bead))",
                                  open: model.connection == .connected ? { model.openWorker(pane.pane, session: line.worker.session) } : nil)
                    }

                    section("Queued", count: board.queued.count)
                    if board.queued.isEmpty {
                        Text(board.queueEmptyText).font(.callout).foregroundStyle(.secondary)
                    }
                    ForEach(board.queued, id: \.bead) { entry in
                        HStack(spacing: 8) {
                            Text(entry.bead).font(.caption.monospaced())
                            Text(entry.model).font(.caption).foregroundStyle(.secondary)
                            Spacer()
                            if board.notRefilling { Text("Not refilled").font(.caption).foregroundStyle(.secondary) }
                        }
                    }

                    section("Landed", count: board.landed.count)
                    ForEach(board.landed) { landed in
                        HStack(spacing: 8) {
                            Label(landed.bead, systemImage: "checkmark.circle").font(.caption.monospaced())
                            Text(landed.detail).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }

                    section("Events", count: pane.events.count)
                    ForEach(pane.events.suffix(40).reversed()) { event in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text(Date(timeIntervalSince1970: Double(event.at) / 1000), format: .dateTime.hour().minute().second())
                                .font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                            Text(event.kind.name).font(.caption.weight(.medium))
                            if let bead = event.bead { Text(bead).font(.caption.monospaced()) }
                            Text(event.detail).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                        }
                    }
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxHeight: 420)
            Button("▴ Collapse") { pane.expanded = false }
                .buttonStyle(.borderless).font(.caption)
                .padding(.horizontal, 10).padding(.bottom, 6)
        }
    }

    /// What the loop last said about the worker's bead.
    private func note(for bead: String) -> String {
        pane.events.last { $0.bead == bead }.map { "\($0.kind.name): \($0.detail)" } ?? "claimed"
    }
}

/// A worker in place of its supervisor: back, the worker's lane and bead and state, Push to pane,
/// its conversation, the requests its supervisor's policy left to the person, and a composer that
/// messages the worker.
struct WorkerDrillIn: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @ObservedObject var pane: SupervisorPaneModel
    let session: String

    var body: some View {
        let line = pane.board?.line(session: session, now: pane.now)
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Button("← Supervisor") { model.closeWorker(pane.pane) }
                    .buttonStyle(.bordered).controlSize(.small)
                if let line {
                    Text(line.laneText).fontWeight(.semibold)
                    Text(line.worker.bead).font(.caption.monospaced()).foregroundStyle(.secondary)
                    WorkerState(line: line)
                    WorkerStageBar(line: line)
                } else {
                    Text("Worker finished").foregroundStyle(.secondary)
                }
                Spacer(minLength: 4)
                if let line {
                    Button(WorkerLine.pushText) { model.pushWorker(pane.pane, session: line.worker.session) }
                        .buttonStyle(.bordered).controlSize(.small)
                        .disabled(model.connection != .connected || line.inPane)
                }
            }
            .font(.callout)
            .padding(.horizontal, 10).padding(.vertical, 6)
            if let error = pane.error { Text(error).font(.caption).foregroundStyle(.red).padding(.horizontal, 10) }
            Divider()
            if let chat = model.chats[session] {
                WorkerConversation(model: model, paneId: paneId, chat: chat, bead: line?.worker.bead ?? session)
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }
}

/// A worker's state word beside its glyph; a stall draws red.
struct WorkerState: View {
    let line: WorkerLine

    var body: some View {
        Label(line.word, systemImage: line.glyph).font(.caption)
            .foregroundStyle(line.alert ? AnyShapeStyle(.red) : line.word == "Working" ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.secondary))
            .lineLimit(1)
    }
}

/// The stage as a row of segments, those up to the worker's stage filled, and its word.
struct WorkerStageBar: View {
    let line: WorkerLine

    var body: some View {
        HStack(spacing: 4) {
            HStack(spacing: 2) {
                ForEach(0..<WorkerLine.stageCount, id: \.self) { i in
                    Capsule()
                        .fill(i < line.stageFilled ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.quaternary))
                        .frame(width: 10, height: 4)
                }
            }
            Text(line.stageText).font(.caption).foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Stage \(line.stageText), \(line.stageFilled) of \(WorkerLine.stageCount)")
    }
}

/// What both a drilled-in worker and a worker pane draw under their header: the conversation,
/// the worker's waiting requests as cards, and the composer.
struct WorkerConversation: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @ObservedObject var chat: SessionChat
    let bead: String

    var body: some View {
        VStack(spacing: 0) {
            TranscriptView(model: model, chat: chat, empty: "This worker has not said anything yet.")
            let waiting = WorkerRequest.pending(model.perms, session: chat.name)
            if let perm = waiting.first {
                WorkerRequestCard(model: model, perm: perm, bead: bead, more: waiting.count - 1, shortcut: model.paneIsFocused(paneId))
                    .padding(.horizontal, 10).padding(.top, 6)
            }
            Composer(
                placeholder: WorkerLine.composerPlaceholder,
                draft: $chat.draft,
                error: chat.error,
                enabled: model.connection == .connected,
                send: { model.send(chat) },
                pickFiles: pickFiles,
                onFocus: { model.paneFocused(pane: paneId) },
                focusRequest: chat.composerFocusRequest
            )
            .padding(10)
        }
        .simultaneousGesture(TapGesture().onEnded { model.paneFocused(pane: paneId) })
    }
}

/// A request outside the supervisor's policy: who wants to run what, and Deny, Always allow …
/// here, Allow once.
struct WorkerRequestCard: View {
    @ObservedObject var model: AppModel
    let perm: PermInfo
    let bead: String
    /// How many more wait behind this one.
    let more: Int
    /// Whether ⌘↩ answers this card: only the card of the focused pane takes it.
    let shortcut: Bool

    @ViewBuilder private func buttons(_ reject: PermOption?, _ always: PermOption?, _ once: PermOption?) -> some View {
        if let reject { Button("Deny") { model.answer(perm, reject.id) } }
        if let always {
            Button("Always allow \(WorkerRequest.subject(perm.title)) here") { model.answer(perm, always.id) }
                .lineLimit(1).fixedSize()
        }
        if let once {
            Button("Allow once ⌘↩") { model.answer(perm, once.id) }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(shortcut ? KeyboardShortcut(.return, modifiers: .command) : nil)
                .lineLimit(1).fixedSize()
        }
    }

    var body: some View {
        let reject = perm.options.first { $0.kind == "reject_once" } ?? perm.options.first { $0.kind.hasPrefix("reject") }
        let always = perm.options.first { $0.kind == "allow_always" }
        let once = perm.options.first { $0.kind == "allow_once" } ?? perm.options.first { !$0.kind.hasPrefix("reject") && $0.kind != "allow_always" }
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "hand.raised.fill").foregroundStyle(.orange)
                Text("\(bead) \(WorkerRequest.wantsToRun)").fontWeight(.semibold)
                Text("· \(WorkerRequest.outsidePolicy)").foregroundStyle(.secondary)
                Spacer(minLength: 4)
                if more > 0 { Text("+\(more) more").font(.caption).foregroundStyle(.secondary) }
            }
            .font(.callout)
            Text(perm.title)
                .font(ChatFont.code)
                .textSelection(.enabled)
                .lineLimit(8)
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
            // One row when the pane is wide enough for every label, a column when it is not.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) { Spacer(minLength: 4); buttons(reject, always, once) }
                VStack(alignment: .trailing, spacing: 6) { buttons(reject, always, once) }
            }
            .controlSize(.small)
        }
        .padding(10)
        .background(.background, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(.separator))
        .accessibilityElement(children: .contain)
    }
}

/// A pane the person pushed a worker to: the worker's header, then its conversation.
struct WorkerPaneBody: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @ObservedObject var chat: SessionChat

    var body: some View {
        let found = model.workerLine(session: chat.name)
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    if let line = found?.line {
                        Text(line.worker.title).lineLimit(1).truncationMode(.tail)
                        Spacer(minLength: 4)
                        WorkerState(line: line)
                        WorkerStageBar(line: line)
                    } else {
                        Text("Worker finished").foregroundStyle(.secondary)
                    }
                }
                .font(.callout)
                Text(WorkerLine.closingText).font(.caption).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 10).padding(.vertical, 6)
            Divider()
            WorkerConversation(model: model, paneId: paneId, chat: chat, bead: found?.line.worker.bead ?? chat.name)
        }
    }
}

/// The pane title of a pushed worker: its lane and bead.
struct WorkerPaneTitle: View {
    @ObservedObject var model: AppModel
    let session: String

    var body: some View {
        HStack(spacing: 6) {
            if let line = model.workerLine(session: session)?.line {
                Text(line.laneText)
                Text(line.worker.bead).font(.caption.monospaced()).foregroundStyle(.secondary)
            } else {
                Text("Worker \(session)")
            }
        }
    }
}
