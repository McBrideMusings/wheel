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

/// An agent's logo from the asset catalog (`AgentLogo-<name>`), drawn in grey; an SF Symbol for an agent wheel
/// has no logo for.
struct UsageIcon: View {
    let agent: String

    private static let logos: Set<String> = ["claude", "codex", "pi"]

    var body: some View {
        Group {
            if Self.logos.contains(agent) {
                Image("AgentLogo-\(agent)").renderingMode(.template).resizable().scaledToFit()
                    .foregroundStyle(.secondary).frame(width: 14, height: 14)
            } else {
                Image(systemName: "cpu")
            }
        }
        .frame(width: 16, height: 16)
    }
}

/// One entry of the strip: an agent's note, or one of its accounts.
private enum StripItem: Identifiable {
    case note(UsageAgent, String)
    case account(UsageAgent, UsageAccount)

    var id: String {
        switch self {
        case let .note(agent, _): return "note-\(agent.id)"
        case let .account(agent, account): return "\(agent.id)-\(account.id)"
        }
    }

    var name: String {
        switch self {
        case let .note(agent, _): return agent.name
        case let .account(agent, account): return "\(agent.name) \(account.name)"
        }
    }
}

/// The window's bottom edge, always on screen: per shown agent, per account its mark, its name and
/// its pinned bars, then the dial that opens the Usage window. When the entries do not fit the
/// width, the ones that do fit stay and the rest collapse into a "+N" that opens the Usage window.
struct UsageStrip: View {
    @ObservedObject var usage: UsageModel
    @Environment(\.openWindow) private var openWindow

    private var items: [StripItem] {
        usage.board.shown.flatMap { agent -> [StripItem] in
            (agent.missingNote.map { [StripItem.note(agent, $0)] } ?? [])
                + agent.accounts.map { StripItem.account(agent, $0) }
        }
    }

    var body: some View {
        let items = items
        // The first candidate that fits the width wins: every entry, then one fewer with the "+N".
        ViewThatFits(in: .horizontal) {
            ForEach(Array((0...items.count).reversed()), id: \.self) { shown in
                row(items: items, shown: shown)
            }
        }
        .padding(.horizontal, 4)
        .onChange(of: usage.openRequest) { openWindow(id: "usage") }
    }

    private func row(items: [StripItem], shown: Int) -> some View {
        HStack(spacing: 18) {
            ForEach(items.prefix(shown)) { item in
                switch item {
                case let .note(agent, note):
                    HStack(spacing: 6) {
                        UsageIcon(agent: agent.id).foregroundStyle(.secondary)
                        Text("\(agent.name): \(note)").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    .help(note)
                    .accessibilityElement(children: .combine)
                case let .account(agent, account):
                    StripAccount(board: usage.board, agent: agent, account: account)
                }
            }
            if shown < items.count {
                let hidden = items.dropFirst(shown)
                Button { openWindow(id: "usage") } label: {
                    Text("+\(hidden.count)").font(.callout.monospacedDigit()).foregroundStyle(.secondary)
                }
                .buttonStyle(.borderless)
                .help("Not shown: \(hidden.map(\.name).joined(separator: ", ")). Open the Usage window")
                .accessibilityLabel("\(hidden.count) more in the Usage window")
            }
            Button { openWindow(id: "usage") } label: {
                Image(systemName: "gauge.with.dots.needle.50percent")
            }
            .buttonStyle(.borderless)
            .help("Open the Usage window")
            .accessibilityLabel("Usage")
        }
        .fixedSize(horizontal: true, vertical: false)
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
                // The reason's first clause ("signed out", "no login in the Keychain"); the hover has the rest.
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
                    Text(agent.name)
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

/// Settings' Agents block: each agent pane program, found with its path or missing with why and how to install it.
struct AgentsSettings: View {
    let agents: [AgentInfo]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Agents").font(ChatFont.body).fontWeight(.semibold)
            VStack(spacing: 0) {
                ForEach(Array(agents.enumerated()), id: \.element.name) { index, agent in
                    if index > 0 { Divider() }
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) { UsageIcon(agent: agent.name); Text(agent.title).font(ChatFont.body) }
                            Text(agent.command).font(ChatFont.line).foregroundStyle(.secondary)
                            Text(agent.home.map { "Home: \($0)" } ?? "Home: \(agent.defaultHome)").font(ChatFont.line).foregroundStyle(.secondary)
                        }
                        Spacer()
                        VStack(alignment: .trailing, spacing: 2) {
                            if agent.found {
                                Text("found").font(ChatFont.body).foregroundStyle(.green)
                                if let path = agent.path, path != agent.command {
                                    Text(path).font(ChatFont.line).foregroundStyle(.secondary)
                                }
                            } else {
                                Text("missing").font(ChatFont.body).foregroundStyle(.orange)
                                if let why = agent.why { Text(why).font(ChatFont.line).foregroundStyle(.secondary) }
                                if let install = agent.installLine { Text("Install: \(install)").font(ChatFont.line).foregroundStyle(.secondary) }
                            }
                        }
                        .multilineTextAlignment(.trailing)
                        .textSelection(.enabled)
                    }
                    .padding(.horizontal, 14).padding(.vertical, 10)
                }
                if agents.isEmpty {
                    Text("The daemon lists no agents.").font(ChatFont.line).foregroundStyle(.secondary).padding(14)
                }
            }
            .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
        }
    }
}
