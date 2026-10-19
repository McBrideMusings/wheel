import SwiftUI
import WheelKit

/// A pane the lead or a supervisor manages is locked (ADR-0010): it shows a dashboard, and its
/// transcript read-only on request, and takes no input: no composer is drawn and a page or
/// browser shows without taking a click or a key. The one answer a locked worker pane still takes
/// is a permission request outside its supervisor's policy, because the worker stalls until
/// someone answers it. The person unlocks it behind a confirm sheet and can hand it back. `PaneView` wraps every pane in this; a pane the person made passes
/// through untouched.
struct ManagedPaneView<Content: View>: View {
    @ObservedObject var model: AppModel
    let paneId: String
    let kind: PaneKind
    let createdBy: CreatedBy
    let prior: CreatedBy?
    @ViewBuilder let content: Content

    var body: some View {
        switch ManagedPane.lock(createdBy: createdBy, prior: prior) {
        case .yours:
            content
        case .locked:
            VStack(spacing: 0) {
                ManagedHeader(model: model, paneId: paneId, kind: kind, createdBy: createdBy, prior: prior, lock: .locked)
                Divider()
                lockedBody
            }
        case .unlocked:
            VStack(spacing: 0) {
                ManagedHeader(model: model, paneId: paneId, kind: kind, createdBy: createdBy, prior: prior, lock: .unlocked)
                ManagedBanner(text: ManagedPane.unlockedBanner(at: model.unlockedAt[paneId].map { $0.formatted(date: .omitted, time: .shortened) }))
                unlockedBody
            }
        }
    }

    private var transcriptOn: Bool { model.transcriptPanes.contains(paneId) }

    @ViewBuilder private var lockedBody: some View {
        let manager = ManagedPane.managerName(createdBy, supervisors: model.supervisorsById)
        switch (kind, model.lockedShowing(paneId)) {
        case let (.worker(session), .transcript):
            if let chat = model.chats[session] {
                LockedTranscript(model: model, chat: chat, manager: manager, isWorker: true, empty: "This worker has not said anything yet.") {
                    WorkerDashboardHost(model: model, session: session, layout: .trafficOnly)
                }
            }
        case let (.worker(session), .dashboard):
            WorkerDashboardHost(model: model, session: session, layout: .full)
        case let (.supervisor(supervisor), .transcript):
            if let chat = model.chats[supervisor.session] {
                LockedTranscript(model: model, chat: chat, manager: manager, isWorker: false,
                                 empty: "The supervisor has not said anything yet.", showsSide: false) { EmptyView() }
            }
        case let (.supervisor(supervisor), .dashboard):
            if let board = model.supervisorPanes[paneId], let chat = model.chats[supervisor.session] {
                SupervisorDashboard(model: model, pane: board, chat: chat)
            }
        case let (.agent(session), .transcript):
            if let chat = model.chats[session] {
                LockedTranscript(model: model, chat: chat, manager: manager, isWorker: false,
                                 empty: "This agent has not said anything yet.", showsSide: false) { EmptyView() }
            }
        case (_, .content):
            // A terminal, a page, a browser or an empty pane shows as it is but takes no click or
            // key (a terminal's keystrokes are also dropped in `AppModel.typed`).
            VStack(spacing: 0) {
                if case .terminal = kind { ReadOnlyStrip(manager: manager, isWorker: false) }
                content
                    .disabled(true)
                    .allowsHitTesting(false)
            }
        default:
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    @ViewBuilder private var unlockedBody: some View {
        if case let .worker(session) = kind, let chat = model.chats[session] {
            // The conversation with a live composer, and the dashboard and traffic beside it.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 0) {
                    WorkerConversation(model: model, paneId: paneId, chat: chat, bead: model.workerBead(session: session) ?? session)
                        .frame(minWidth: 280)
                    Divider()
                    WorkerDashboardHost(model: model, session: session, layout: .side).frame(width: 210)
                }
                WorkerConversation(model: model, paneId: paneId, chat: chat, bead: model.workerBead(session: session) ?? session)
            }
        } else {
            content
        }
    }
}

// MARK: Header

struct ManagedHeader: View {
    @ObservedObject var model: AppModel
    let paneId: String
    let kind: PaneKind
    let createdBy: CreatedBy
    let prior: CreatedBy?
    let lock: ManagedPane.Lock

    private var supervisors: [String: Supervisor] { model.supervisorsById }

    /// The transcript toggle: a worker and a supervisor have a dashboard to toggle against.
    private var hasDashboard: Bool { model.lockedHasDashboard(kind) }

    var body: some View {
        // One row when the pane is wide enough for every label, the controls under the title when not.
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 8) {
                title
                Spacer(minLength: 4)
                controls
            }
            VStack(alignment: .leading, spacing: 6) {
                title
                HStack(spacing: 8) {
                    controls
                    Spacer(minLength: 0)
                }
            }
        }
        .lineLimit(1)
        .padding(.horizontal, 10).padding(.vertical, 6)
    }

    @ViewBuilder private var controls: some View {
        switch lock {
        case .locked:
            badge
            if hasDashboard {
                Toggle(ManagedPane.transcriptText, isOn: Binding(get: { model.transcriptPanes.contains(paneId) }, set: { _ in model.toggleTranscript(paneId) }))
                    .toggleStyle(.button).controlSize(.small)
                    .accessibilityLabel("Show the transcript of \(paneName)")
            }
            Button(ManagedPane.unlockText) { model.requestUnlock(paneId) }
                .controlSize(.small)
                .disabled(model.connection != .connected)
                .accessibilityLabel("Unlock \(paneName)")
        case .unlocked:
            Pill(text: "\(ManagedPane.yoursBadge) · created_by person", systemImage: "person.fill", tint: .accentColor)
            if let prior, ManagedPane.canHandBack(prior: prior, supervisors: supervisors) {
                Button(ManagedPane.handBackText(prior, supervisors: supervisors)) { model.handBack(paneId) }
                    .controlSize(.small)
                    .disabled(model.connection != .connected)
            }
        case .yours:
            EmptyView()
        }
    }

    private var paneName: String {
        switch kind {
        case let .worker(session): model.workerBead(session: session) ?? "worker"
        case .supervisor: "the supervisor"
        default: kind.name + " pane"
        }
    }

    private var badge: some View {
        Button { model.focus(manager: createdBy) } label: {
            Pill(text: ManagedPane.badge(createdBy, supervisors: supervisors), systemImage: "lock.fill", tint: .secondary)
        }
        .buttonStyle(.plain)
        .onHover { model.highlight(manager: $0 ? createdBy : nil) }
        .onDisappear { model.highlight(manager: nil) }
        .help("Focus who manages this pane")
        .accessibilityLabel(ManagedPane.badge(createdBy, supervisors: supervisors))
    }

    @ViewBuilder private var title: some View {
        switch kind {
        case let .worker(session):
            let dashboard = model.workerDashboard(session: session)
            HStack(spacing: 8) {
                Image(systemName: "hammer").foregroundStyle(.secondary)
                Text(dashboard?.bead ?? "Worker").font(.callout.monospaced()).fontWeight(.semibold)
                if let dashboard { Pill(text: dashboard.statusWord, tint: dashboard.tone.color) }
            }
        case let .supervisor(supervisor):
            HStack(spacing: 8) {
                Image(systemName: "person.3.sequence").foregroundStyle(.secondary)
                Text("Supervisor · \(supervisor.scope)").font(.callout).fontWeight(.semibold)
                Pill(text: SupervisorStyle.stateName(supervisor.state), tint: SupervisorStyle.color(supervisor.state))
            }
        default:
            HStack(spacing: 8) {
                Image(systemName: "rectangle.on.rectangle").foregroundStyle(.secondary)
                Text(kind.name.capitalized).font(.callout).fontWeight(.semibold)
            }
        }
    }
}

extension WorkerDashboard.Tone {
    var color: Color {
        switch self {
        case .working: .accentColor
        case .waiting: .orange
        case .stalled: .red
        case .quiet: .secondary
        }
    }
}

/// A rounded label: a status word, or who manages a pane.
struct Pill: View {
    let text: String
    var systemImage: String?
    let tint: Color

    var body: some View {
        HStack(spacing: 4) {
            if let systemImage { Image(systemName: systemImage).font(.caption2) }
            Text(text).font(.caption).fontWeight(.medium)
        }
        .padding(.horizontal, 8).padding(.vertical, 2)
        .foregroundStyle(tint)
        .background(tint.opacity(0.14), in: Capsule())
        .lineLimit(1)
    }
}

/// The light-blue strip under an unlocked pane's header.
struct ManagedBanner: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.callout)
            .foregroundStyle(Color.accentColor)
            .padding(.horizontal, 12).padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.accentColor.opacity(0.12))
            .accessibilityAddTraits(.isStaticText)
    }
}

// MARK: Locked bodies

/// The transcript with no composer: a notice above, the lock where the composer was, and the
/// traffic column beside it when the pane is wide enough.
struct LockedTranscript<Side: View>: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat
    let manager: String
    let isWorker: Bool
    let empty: String
    var showsSide = true
    @ViewBuilder let side: Side

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 0) {
                main.frame(minWidth: 280)
                if showsSide {
                    Divider()
                    side.frame(width: 190)
                }
            }
            main
        }
    }

    private var main: some View {
        VStack(spacing: 0) {
            ReadOnlyStrip(manager: manager, isWorker: isWorker)
            TranscriptView(model: model, chat: chat, empty: empty)
                .opacity(0.7)
            Label(ManagedPane.lockedComposerHint, systemImage: "lock.fill")
                .font(.caption).foregroundStyle(.secondary)
                .padding(.horizontal, 10).padding(.vertical, 3)
                .background(.quaternary, in: Capsule())
                .padding(.top, 6)
            HStack {
                Text(ManagedPane.lockedComposerPlaceholder).foregroundStyle(.secondary)
                Spacer()
                Text("Managed pane").font(.caption).foregroundStyle(.secondary)
                Image(systemName: "arrow.up.circle.fill").foregroundStyle(.quaternary)
            }
            .padding(10)
            .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 10))
            .padding(10)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Composer, locked")
        }
    }
}

/// The line under a locked pane's header that says who manages it.
struct ReadOnlyStrip: View {
    let manager: String
    let isWorker: Bool

    var body: some View {
        Label(ManagedPane.readOnlyNotice(manager, isWorker: isWorker), systemImage: "lock.fill")
            .font(.caption).foregroundStyle(.secondary)
            .padding(.horizontal, 12).padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.quaternary.opacity(0.5))
    }
}

// MARK: Worker dashboard

struct WorkerDashboardHost: View {
    enum Layout { case full, side, trafficOnly }

    @ObservedObject var model: AppModel
    let session: String
    let layout: Layout

    var body: some View {
        if let (_, board) = model.supervisorPane(of: session), let chat = model.chats[session] {
            WorkerDashboardLive(model: model, session: session, layout: layout, board: board, chat: chat)
        } else {
            VStack(spacing: 6) {
                Image(systemName: "checkmark.circle").font(.title2).foregroundStyle(.secondary)
                Text("Worker finished").foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

struct WorkerDashboardLive: View {
    @ObservedObject var model: AppModel
    let session: String
    let layout: WorkerDashboardHost.Layout
    @ObservedObject var board: SupervisorPaneModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        if let d = model.workerDashboard(session: session, now: board.now) {
            switch layout {
            case .full: full(d)
            case .side: side(d)
            case .trafficOnly: trafficColumn(d)
            }
        } else {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func full(_ d: WorkerDashboard) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(d.bead).font(.caption.monospaced()).foregroundStyle(.secondary)
                    Text(d.title).font(.title3.weight(.semibold))
                }
                if let title = d.waitingTitle, let detail = d.waitingDetail {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(title).fontWeight(.semibold)
                        Text(detail)
                    }
                    .font(.callout)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.orange.opacity(0.14), in: RoundedRectangle(cornerRadius: 8))
                }
                ForEach(WorkerRequest.pending(model.perms, session: chat.name)) { perm in
                    WorkerRequestCard(model: model, perm: perm, bead: d.bead, more: 0, shortcut: false)
                }
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 100), alignment: .topLeading)], alignment: .leading, spacing: 10) {
                    Stat("Stage") {
                        Text(d.stageText.capitalized)
                        StageSegments(filled: d.stageFilled, count: d.stageCount, tint: d.tone.color)
                    }
                    Stat("Elapsed") { Text(d.elapsed ?? "—").monospacedDigit() }
                    Stat("Continues") { Text(d.continuesText) }
                    Stat("Restarts") { Text(d.restartsText) }
                    Stat("Model") { Text(d.model).lineLimit(1).truncationMode(.middle) }
                    Stat("Branch") { Text(d.branch).font(.callout.monospaced()).lineLimit(1).truncationMode(.middle) }
                }
                SectionTitle("Latest actions")
                if d.actions.isEmpty {
                    Text("No tool calls yet.").font(.callout).foregroundStyle(.secondary)
                }
                ForEach(d.actions) { action in
                    HStack(spacing: 8) {
                        Text(action.title).font(.callout.monospaced()).lineLimit(1).truncationMode(.middle)
                            .foregroundStyle(action.running ? Color.accentColor : .primary)
                        Spacer(minLength: 4)
                        Text(action.age).font(.caption).foregroundStyle(.secondary)
                    }
                }
                SectionTitle("Inbox traffic · up and down")
                TrafficList(rows: d.traffic)
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func side(_ d: WorkerDashboard) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                SectionTitle("Dashboard")
                Text(d.bead).font(.callout.monospaced()).fontWeight(.semibold)
                Text("\(d.statusWord) · \(d.elapsed ?? "—")").font(.callout)
                StageSegments(filled: d.stageFilled, count: d.stageCount, tint: d.tone.color)
                SectionTitle("Inbox traffic").padding(.top, 6)
                TrafficList(rows: d.traffic)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func trafficColumn(_ d: WorkerDashboard) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                SectionTitle("Inbox traffic")
                TrafficList(rows: d.traffic)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct SectionTitle: View {
    let text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text.uppercased()).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
    }
}

struct Stat<Value: View>: View {
    let label: String
    @ViewBuilder let value: Value

    init(_ label: String, @ViewBuilder value: () -> Value) {
        self.label = label
        self.value = value()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            SectionTitle(label)
            value.font(.callout)
        }
        .accessibilityElement(children: .combine)
    }
}

/// The stage as a row of segments with those up to the stage filled.
struct StageSegments: View {
    let filled: Int
    let count: Int
    let tint: Color

    var body: some View {
        HStack(spacing: 2) {
            ForEach(0..<count, id: \.self) { i in
                Capsule().fill(i < filled ? AnyShapeStyle(tint) : AnyShapeStyle(.quaternary)).frame(width: 18, height: 4)
            }
        }
        .accessibilityHidden(true)
    }
}

struct TrafficList: View {
    let rows: [TrafficRow]

    var body: some View {
        if rows.isEmpty {
            Text("Nothing sent yet.").font(.callout).foregroundStyle(.secondary)
        }
        ForEach(rows) { row in
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Image(systemName: row.direction == .up ? "arrow.up" : "arrow.down")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(row.direction == .up && row.isNew ? Color.orange : Color.accentColor)
                        .accessibilityLabel(row.direction == .up ? "up" : "down")
                    Text(row.route).font(.caption.weight(.medium)).lineLimit(2)
                    Spacer(minLength: 2)
                    Text(row.at.formatted(date: .omitted, time: .shortened)).font(.caption2).foregroundStyle(.secondary).fixedSize()
                }
                HStack(spacing: 4) {
                    Pill(text: row.kind, tint: .secondary)
                    if row.isNew { Pill(text: "new", tint: .orange) }
                }
                if !row.detail.isEmpty {
                    Text(row.detail).font(.caption).foregroundStyle(.secondary).lineLimit(3)
                }
            }
            .accessibilityElement(children: .combine)
        }
    }
}

// MARK: Supervisor dashboard

struct SupervisorDashboard: View {
    @ObservedObject var model: AppModel
    @ObservedObject var pane: SupervisorPaneModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                if let board = pane.board {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(board.supervisor.scope).font(.title3.weight(.semibold))
                        Text("\(board.info.running.count) of \(board.supervisor.workers) slots taken").font(.caption).foregroundStyle(.secondary)
                    }
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 100), alignment: .topLeading)], alignment: .leading, spacing: 10) {
                        Stat("Loop") { Text(SupervisorStyle.stateName(board.supervisor.state)) }
                        Stat("Usage cap") { Text(board.supervisor.cap.map { "Holds at \($0)%" } ?? "Not holding") }
                        Stat("Inbox") { Text(board.forYou == 0 ? "Empty" : "\(board.forYou) waiting") }
                        Stat("Landed") { Text("\(board.landed.count)") }
                        Stat("Agent") { Text(model.stateLabel(chat)) }
                    }
                    SectionTitle("Workers")
                    if board.lines(now: pane.now).isEmpty { Text("No workers running.").font(.callout).foregroundStyle(.secondary) }
                    ForEach(board.lines(now: pane.now)) { line in
                        let d = WorkerDashboard(worker: line.worker, board: board, rows: [], supervisorName: board.supervisor.scope, now: pane.now)
                        HStack(spacing: 8) {
                            Text(line.worker.bead).font(.caption.monospaced())
                            Text(line.worker.title).lineLimit(1).truncationMode(.tail)
                            Spacer(minLength: 4)
                            StageSegments(filled: d.stageFilled, count: d.stageCount, tint: d.tone.color)
                            Pill(text: d.elapsed.map { "\(d.statusWord) · \($0)" } ?? d.statusWord, tint: d.tone.color)
                        }
                        .font(.callout)
                    }
                    SectionTitle("Traffic with the lead and the workers")
                    TrafficList(rows: traffic(board))
                } else if let error = pane.supervisor.planError {
                    Label(error, systemImage: "exclamationmark.triangle").font(.caption).foregroundStyle(.orange)
                } else {
                    ProgressView()
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func traffic(_ board: SupervisorBoard) -> [TrafficRow] {
        let name = board.supervisor.scope
        var rows = board.events.suffix(8).map { TrafficRow(event: $0, worker: nil, supervisor: name) }
        for item in board.inbox {
            rows.removeAll { $0.id == "e\(item.kind.name)-\(item.at)" }
            rows.append(TrafficRow(id: "i\(item.id)", direction: .up, route: "\(item.bead) to \(name)", kind: item.kind.name,
                                   at: Date(timeIntervalSince1970: Double(item.at) / 1000), detail: item.detail, isNew: true))
        }
        return rows.sorted { $0.at > $1.at }
    }
}

// MARK: Unlock sheet

/// The confirm sheet: what changes when the person takes the pane over.
struct UnlockSheet: View {
    @ObservedObject var model: AppModel
    let pane: String

    private var kind: PaneKind? {
        for space in model.spaces {
            if let found = space.root.panes.first(where: { $0.id == pane }) { return found.kind }
        }
        return nil
    }

    private var isWorker: Bool { if case .worker = kind { true } else { false } }

    private var what: String {
        switch kind {
        case let .worker(session): model.workerBead(session: session) ?? "this worker"
        case .supervisor: "the supervisor"
        case let .some(kind): "this \(kind.name) pane"
        case .none: "this pane"
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(ManagedPane.confirmTitle(what)).font(.headline)
            Text(ManagedPane.confirmBody)
            VStack(alignment: .leading, spacing: 6) {
                ForEach(ManagedPane.confirmBullets(isWorker: isWorker), id: \.self) { line in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text("•")
                        Text(line)
                    }
                    .font(.callout)
                }
            }
            HStack {
                Spacer()
                Button("Cancel") { model.cancelUnlock() }.keyboardShortcut(.cancelAction)
                Button(ManagedPane.confirmButton) { model.confirmUnlock() }.keyboardShortcut(.defaultAction).buttonStyle(.borderedProminent)
            }
            .padding(.top, 4)
        }
        .padding(20)
        .frame(width: 340)
    }
}
