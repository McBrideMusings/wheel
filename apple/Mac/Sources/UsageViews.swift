import SwiftUI
import WheelKit

/// What the usage strip and the Usage window show: the daemon's board, which it sends on connect,
/// after each reading and whenever the agents or pins settings change.
@MainActor
final class UsageModel: ObservableObject {
    @Published private(set) var board = UsageBoard()
    /// A board has arrived since the last connect.
    @Published private(set) var loaded = false
    /// Agent ids whose refresh is on its way.
    @Published private(set) var refreshing: Set<String> = []
    /// Counts requests to show the Usage window; the debug endpoint sends them.
    @Published var openRequest = 0
    /// The Usage window is on screen.
    @Published var open = false

    func received(_ board: UsageBoard) {
        self.board = board
        loaded = true
        refreshing = []
    }

    func refreshStarted() { refreshing = Set(board.agents.map(\.id)) }

    func disconnected() { loaded = false }
}

/// A bar, a percent and a tag: one pinned row in the strip.
private struct StripBar: View {
    let row: UsageRow
    let stale: Bool

    var body: some View {
        HStack(spacing: 6) {
            UsageBar(pct: row.pct, stale: stale).frame(width: 104, height: 7)
            Text(UsageText.percent(row.pct)).font(.callout.monospacedDigit())
                .frame(width: 36, alignment: .trailing)
            Text(row.tag).font(.callout).foregroundStyle(.secondary)
        }
        .fixedSize()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(row.label) \(UsageText.percent(row.pct))\(stale ? ", stale" : "")")
    }
}

/// A rounded bar filled to `pct`: neutral, orange from 80%, red from 95%, track-coloured when stale.
struct UsageBar: View {
    let pct: Double
    let stale: Bool

    private var fill: Color {
        if stale { return .primary.opacity(0.12) }
        if pct >= 95 { return .red }
        if pct >= 80 { return .orange }
        return .primary.opacity(0.55)
    }

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(.primary.opacity(0.12))
                Capsule().fill(fill)
                    .frame(width: max(geo.size.height, geo.size.width * min(max(pct, 0), 100) / 100))
            }
        }
    }
}

/// Claude Code's starburst: twelve rays of uneven length from one centre. SF Symbols has no
/// Claude mark, so wheel draws it.
struct ClaudeMark: Shape {
    private static let lengths: [CGFloat] = [0.95, 0.78, 0.9, 0.7, 0.98, 0.82, 0.88, 0.72, 0.96, 0.8, 0.92, 0.75]

    func path(in rect: CGRect) -> Path {
        let centre = CGPoint(x: rect.midX, y: rect.midY)
        let radius = min(rect.width, rect.height) / 2
        var path = Path()
        for (i, length) in Self.lengths.enumerated() {
            let angle = CGFloat(i) / CGFloat(Self.lengths.count) * 2 * .pi + 0.08 * sin(CGFloat(i) * 2.3)
            path.move(to: centre)
            path.addLine(to: CGPoint(x: centre.x + cos(angle) * radius * length,
                                     y: centre.y + sin(angle) * radius * length))
        }
        return path
    }
}

/// An agent's mark: Claude's starburst, an SF Symbol for the others.
struct UsageIcon: View {
    let agent: String

    var body: some View {
        Group {
            switch agent {
            case "claude":
                ClaudeMark().stroke(style: StrokeStyle(lineWidth: 1.5, lineCap: .round)).frame(width: 13, height: 13)
            case "codex": Image(systemName: "chevron.left.forwardslash.chevron.right")
            default: Image(systemName: "cpu")
            }
        }
        .frame(width: 16, height: 16)
    }
}

/// The window's bottom edge, always on screen: per shown agent, per account its mark, its name and
/// its pinned bars, then the dial that opens the Usage window.
struct UsageStrip: View {
    @ObservedObject var usage: UsageModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        HStack(spacing: 18) {
            ForEach(usage.board.shown) { agent in
                if let note = agent.missingNote {
                    HStack(spacing: 6) {
                        UsageIcon(agent: agent.id).foregroundStyle(.secondary)
                        Text("\(agent.name): \(note)").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    .help(note)
                    .accessibilityElement(children: .combine)
                }
                ForEach(agent.accounts) { account in
                    StripAccount(board: usage.board, agent: agent, account: account)
                }
            }
            Button { openWindow(id: "usage") } label: {
                Image(systemName: "gauge.with.dots.needle.50percent")
            }
            .buttonStyle(.borderless)
            .help("Open the Usage window")
            .accessibilityLabel("Usage")
        }
        .padding(.horizontal, 4)
        .onChange(of: usage.openRequest) { openWindow(id: "usage") }
    }
}

/// One account in the strip: mark, name, pinned bars, and why when it has none or they are stale.
private struct StripAccount: View {
    let board: UsageBoard
    let agent: UsageAgent
    let account: UsageAccount

    var body: some View {
        HStack(spacing: 8) {
            UsageIcon(agent: agent.id).foregroundStyle(.secondary)
                .accessibilityLabel(agent.name)
            Text(account.name).font(.callout).foregroundStyle(.secondary).lineLimit(1)
            if let note = account.missingNote {
                // The reason's first clause ("signed out", "lead is not logged in"); the hover has the rest.
                Text(note.split(separator: ":").first.map(String.init) ?? note)
                    .font(.callout).foregroundStyle(.secondary).lineLimit(1)
            } else {
                ForEach(board.stripRows(for: account, of: agent)) { row in
                    StripBar(row: row, stale: account.isStale)
                }
                if account.isStale {
                    Text("stale").font(.caption).foregroundStyle(.orange)
                }
            }
        }
        .help(help)
    }

    private var help: String {
        if let note = account.missingNote { return "\(agent.name) \(account.name): \(note)" }
        if account.isStale {
            return "Stale, read \(UsageText.age(account.usage.ageSecs ?? 0)) ago: \(account.usage.why ?? "")"
        }
        return [agent.name, account.name, account.usage.reading?.plan].compactMap { $0 }.joined(separator: " · ")
    }
}

/// The Usage window: a card per account of each coding agent shown, with refresh-all, settings and
/// close in its toolbar.
struct UsageWindow: View {
    @ObservedObject var model: AppModel
    @ObservedObject var usage: UsageModel
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if usage.board.shown.isEmpty {
                    Text(usage.loaded ? "No coding agent is set to show usage. Turn one on in Settings." : "Asking wheeld for usage…")
                        .foregroundStyle(.secondary)
                }
                ForEach(usage.board.shown) { agent in
                    if let note = agent.missingNote {
                        Label("\(agent.name): \(note)", systemImage: "exclamationmark.triangle")
                            .font(.callout).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    ForEach(agent.accounts) { account in
                        UsageCard(model: model, usage: usage, agent: agent, account: account)
                    }
                }
            }
            .padding(16)
        }
        .frame(width: 400, height: 480)
        .background(Color(nsColor: .windowBackgroundColor))
        .toolbar {
            ToolbarItemGroup {
                Button { model.askUsage(dueOnly: false) } label: { Label("Refresh All", systemImage: "arrow.clockwise") }
                    .help("Read every account's usage now")
                Button { openSettings() } label: { Label("Settings", systemImage: "gearshape") }
                    .help("Choose which agents show usage")
                Button { dismiss() } label: { Label("Close", systemImage: "xmark") }
                    .help("Close the Usage window")
            }
        }
        .onAppear { usage.open = true }
        .onDisappear { usage.open = false }
    }
}

private struct UsageCard: View {
    @ObservedObject var model: AppModel
    @ObservedObject var usage: UsageModel
    let agent: UsageAgent
    let account: UsageAccount

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                UsageIcon(agent: agent.id)
                VStack(alignment: .leading, spacing: 1) {
                    Text(account.name).font(.headline)
                    Text(account.lead ? "\(agent.name) · the lead's account" : agent.name)
                        .font(.caption).foregroundStyle(.secondary)
                }
                if let plan = account.usage.reading?.plan {
                    Text(plan).font(.caption).padding(.horizontal, 6).padding(.vertical, 1)
                        .background(.quaternary, in: Capsule())
                }
                Spacer()
                if usage.refreshing.contains(agent.id) {
                    ProgressView().controlSize(.small)
                } else {
                    Button { model.askUsage(dueOnly: false) } label: { Image(systemName: "arrow.clockwise") }
                        .buttonStyle(.borderless).help("Read every account's usage now")
                        .accessibilityLabel("Refresh usage")
                }
            }
            if let note = account.missingNote {
                Label(note, systemImage: "exclamationmark.triangle").font(.callout).foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                if account.isStale {
                    Label("Stale, read \(UsageText.age(account.usage.ageSecs ?? 0)) ago: \(account.usage.why ?? "")",
                          systemImage: "clock.badge.exclamationmark").font(.caption).foregroundStyle(.orange)
                }
                ForEach(account.rows(agent: agent.id)) { row in
                    HStack(spacing: 8) {
                        Text(row.label).frame(width: 96, alignment: .leading)
                        UsageBar(pct: row.pct, stale: account.isStale).frame(height: 7)
                        Text(UsageText.percent(row.pct)).font(.body.monospacedDigit()).frame(width: 40, alignment: .trailing)
                        Text(UsageText.resets(row.resetsAt) ?? "").font(.caption).foregroundStyle(.secondary)
                            .frame(width: 70, alignment: .leading)
                        Button { model.togglePin(row.id) } label: {
                            Image(systemName: usage.board.isPinned(row.id) ? "pin.fill" : "pin")
                        }
                        .buttonStyle(.borderless)
                        .help(usage.board.isPinned(row.id) ? "Take off the strip, for every \(agent.name) account" : "Pin to the strip, for every \(agent.name) account")
                        .accessibilityLabel("\(usage.board.isPinned(row.id) ? "Unpin" : "Pin") \(row.label)")
                    }
                }
                Divider()
                if let r = account.usage.reading {
                    HStack {
                        Text("Extra usage").foregroundStyle(.secondary)
                        Spacer()
                        Text(r.extraUsage.map { $0 ? "On" : "Off" } ?? "Unknown")
                    }
                    if let email = r.email {
                        HStack { Text("Account").foregroundStyle(.secondary); Spacer(); Text(email) }
                    }
                }
            }
        }
        .padding(12)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator, lineWidth: 0.5))
    }
}

/// Settings' Usage block: which coding agents show usage, and why an agent cannot.
struct UsageAgentsSettings: View {
    @ObservedObject var model: AppModel
    @ObservedObject var usage: UsageModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Usage agents").font(ChatFont.body).fontWeight(.semibold)
            VStack(spacing: 0) {
                ForEach(Array(usage.board.agents.enumerated()), id: \.element.id) { index, agent in
                    if index > 0 { Divider() }
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        Toggle(agent.name, isOn: Binding(get: { agent.shown }, set: { _ in model.toggleUsageAgent(agent.id) }))
                            .disabled(!agent.readable && !agent.shown)
                        Spacer()
                        Text(agent.readable ? "readable" : (agent.why ?? ""))
                            .font(ChatFont.line).foregroundStyle(.secondary)
                            .multilineTextAlignment(.trailing)
                    }
                    .padding(.horizontal, 14).padding(.vertical, 10)
                }
            }
            .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
        }
    }
}
