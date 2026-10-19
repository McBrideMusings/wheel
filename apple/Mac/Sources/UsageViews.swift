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
        HStack(spacing: 4) {
            UsageBar(pct: row.pct, stale: stale).frame(width: 34)
            Text(UsageText.percent(row.pct)).font(.caption.monospacedDigit())
            Text(row.tag).font(.caption2).foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(row.label) \(UsageText.percent(row.pct))\(stale ? ", stale" : "")")
    }
}

struct UsageBar: View {
    let pct: Double
    let stale: Bool

    var body: some View {
        ProgressView(value: min(max(pct, 0), 100), total: 100)
            .progressViewStyle(.linear)
            .tint(stale ? .gray : (pct >= 95 ? .red : (pct >= 80 ? .orange : .accentColor)))
    }
}

/// An agent's icon: Claude's spark, Codex's brackets.
func usageIcon(_ agent: String) -> String {
    switch agent {
    case "claude": "sparkle"
    case "codex": "chevron.left.forwardslash.chevron.right"
    default: "cpu"
    }
}

/// The window's bottom-left corner: per agent shown its icon and its pinned bars, then the dial
/// that opens the Usage window.
struct UsageStrip: View {
    @ObservedObject var usage: UsageModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        HStack(spacing: 10) {
            ForEach(usage.board.shown) { agent in
                HStack(spacing: 6) {
                    Image(systemName: usageIcon(agent.id)).foregroundStyle(.secondary)
                        .accessibilityLabel(agent.name)
                    if let note = agent.missingNote {
                        Text(note).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    } else {
                        ForEach(usage.board.stripRows(for: agent)) { row in
                            StripBar(row: row, stale: agent.isStale)
                        }
                        if agent.isStale {
                            Text("stale \(UsageText.age(agent.usage?.ageSecs ?? 0))")
                                .font(.caption2).foregroundStyle(.orange)
                        }
                    }
                }
                .help(agent.isStale ? "Stale: \(agent.usage?.why ?? "")" : agent.name)
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

/// The Usage window: a card per coding agent shown, with refresh-all, settings and close in its toolbar.
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
                    UsageCard(model: model, usage: usage, agent: agent)
                }
            }
            .padding(16)
        }
        .frame(width: 400, height: 480)
        .background(Color(nsColor: .windowBackgroundColor))
        .toolbar {
            ToolbarItemGroup {
                Button { model.askUsage(dueOnly: false) } label: { Label("Refresh All", systemImage: "arrow.clockwise") }
                    .help("Read every agent's usage now")
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

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: usageIcon(agent.id))
                Text(agent.name).font(.headline)
                if let plan = agent.usage?.reading?.plan {
                    Text(plan).font(.caption).padding(.horizontal, 6).padding(.vertical, 1)
                        .background(.quaternary, in: Capsule())
                }
                Spacer()
                if usage.refreshing.contains(agent.id) {
                    ProgressView().controlSize(.small)
                } else {
                    Button { model.askUsage(dueOnly: false) } label: { Image(systemName: "arrow.clockwise") }
                        .buttonStyle(.borderless).help("Read every agent's usage now")
                        .accessibilityLabel("Refresh usage")
                }
            }
            if let note = agent.missingNote {
                Label(note, systemImage: "exclamationmark.triangle").font(.callout).foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                if agent.isStale {
                    Label("Stale, read \(UsageText.age(agent.usage?.ageSecs ?? 0)) ago: \(agent.usage?.why ?? "")",
                          systemImage: "clock.badge.exclamationmark").font(.caption).foregroundStyle(.orange)
                }
                ForEach(agent.rows) { row in
                    HStack(spacing: 8) {
                        Text(row.label).frame(width: 96, alignment: .leading)
                        UsageBar(pct: row.pct, stale: agent.isStale)
                        Text(UsageText.percent(row.pct)).font(.body.monospacedDigit()).frame(width: 40, alignment: .trailing)
                        Text(UsageText.resets(row.resetsAt) ?? "").font(.caption).foregroundStyle(.secondary)
                            .frame(width: 70, alignment: .leading)
                        Button { model.togglePin(row.id) } label: {
                            Image(systemName: usage.board.isPinned(row.id) ? "pin.fill" : "pin")
                        }
                        .buttonStyle(.borderless)
                        .help(usage.board.isPinned(row.id) ? "Take off the strip" : "Pin to the strip")
                        .accessibilityLabel("\(usage.board.isPinned(row.id) ? "Unpin" : "Pin") \(row.label)")
                    }
                }
                Divider()
                if let r = agent.usage?.reading {
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
