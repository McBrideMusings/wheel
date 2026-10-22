import SwiftUI
import WheelKit
import WheelUI

/// The coding agents' usage under the top bar: per shown agent, per account its name and its pinned
/// bars, from the same board the Mac's strip draws. It scrolls sideways when the entries do not fit.
struct UsageStrip: View {
    let model: PhoneModel

    var body: some View {
        let board = model.usage
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 16) {
                if board.shown.isEmpty {
                    Text(model.usageLoaded ? "No coding agent shows usage" : "Asking for usage…")
                        .font(.caption).foregroundStyle(.secondary)
                }
                ForEach(board.shown) { agent in
                    if let note = agent.missingNote {
                        Text("\(agent.name): \(note)").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    ForEach(agent.accounts) { account in
                        StripAccount(board: board, agent: agent, account: account)
                    }
                }
            }
            .padding(.horizontal, 16)
        }
        .padding(.bottom, 8)
        .accessibilityIdentifier("usage-strip")
    }
}

/// One account in the strip: its name, then its pinned bars, or why it has none.
private struct StripAccount: View {
    let board: UsageBoard
    let agent: UsageAgent
    let account: UsageAccount

    var body: some View {
        HStack(spacing: 8) {
            Text("\(agent.name) \(account.name)").font(.caption).foregroundStyle(.secondary).lineLimit(1)
            if let note = account.missingNote {
                Text(note.split(separator: ":").first.map(String.init) ?? note)
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            } else {
                ForEach(board.stripRows(for: account, of: agent)) { row in
                    HStack(spacing: 5) {
                        UsageBar(pct: row.pct, stale: account.isStale).frame(width: 56, height: 6)
                        Text(UsageText.percent(row.pct)).font(.caption.monospacedDigit())
                        Text(row.tag).font(.caption).foregroundStyle(.secondary)
                    }
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel("\(row.label) \(UsageText.percent(row.pct))\(account.isStale ? ", stale" : "")")
                }
                if account.isStale { Text("stale").font(.caption).foregroundStyle(.orange) }
            }
        }
    }
}
