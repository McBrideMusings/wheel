import SwiftUI
import WheelKit
import WheelUI

/// The line above an agent's chat: logo, name, the folder it works in, and how full its context
/// is. Tapping opens the card with the five values behind it: model and effort, account, folder,
/// login folder, context and the weekly limit of the pane's login.
struct AgentHeader: View {
    let model: PhoneModel
    let pane: String
    let agent: String
    let harness: String
    @ObservedObject var chat: AgentChat
    @State private var open = false

    private var info: PaneHeaderInfo? { model.logins[pane] }
    private var folder: String? { info?.folder }
    private var weekly: UsageReading? { info?.login?.account?.usage.reading }

    var body: some View {
        VStack(spacing: 0) {
            Button {
                open.toggle()
                if open { model.requestHeader(pane, refresh: true) }
            } label: {
                HStack(spacing: 8) {
                    AgentGlyph(agent: harness)
                    Text(AgentInfo.title(of: agent)).font(.subheadline.weight(.semibold))
                    if let folder {
                        Text(Self.leaf(folder)).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer(minLength: 8)
                    if let context = chat.context {
                        ContextRing(fraction: context.fraction, hot: context.percent > 80)
                            .frame(width: 16, height: 16)
                        Text(context.shortText).font(.caption.monospacedDigit())
                            .foregroundStyle(context.percent > 80 ? Color.orange : .secondary)
                    }
                    Image(systemName: "chevron.down").font(.caption2).foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(open ? 180 : 0))
                }
                .padding(.horizontal, 16).padding(.vertical, 8)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("agent-header")
            .accessibilityLabel("\(AgentInfo.title(of: agent)), \(folder.map(Self.leaf) ?? "no folder")\(chat.context.map { ", context \($0.percent)%" } ?? "")")
            if open { card }
            Divider()
        }
        .task { model.requestHeader(pane) }
    }

    private var card: some View {
        VStack(alignment: .leading, spacing: 8) {
            row("Model", value: Self.current(of: "model", in: chat.config) ?? "-")
            if let effort = Self.current(of: "thought_level", in: chat.config) { row("Effort", value: effort) }
            row("Account", value: info?.login?.account.map(Self.accountText) ?? "-")
            row("Folder", value: folder ?? "-")
            row("Login folder", value: info?.login?.dir ?? "-")
            if let context = chat.context {
                meter("Context", pct: Double(context.percent), fraction: context.fraction,
                      detail: "\(context.used.formatted()) of \(context.size.formatted()) tokens")
            } else {
                row("Context", value: "No figure yet")
            }
            if let weekly {
                meter("Weekly", pct: weekly.weeklyPct, fraction: weekly.weeklyPct / 100,
                      detail: UsageText.resets(weekly.weeklyResetsAt) ?? "")
            } else if let login = info?.login {
                row("Weekly", value: "No usage reading for \(login.dir)")
            }
        }
        .font(.footnote)
        .padding(.horizontal, 16).padding(.bottom, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityIdentifier("agent-card")
    }

    private func row(_ label: String, value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label).foregroundStyle(.secondary).frame(width: 92, alignment: .leading)
            Text(value).lineLimit(2).truncationMode(.middle)
            Spacer(minLength: 0)
        }
    }

    private func meter(_ label: String, pct: Double, fraction: Double, detail: String) -> some View {
        HStack(alignment: .center) {
            Text(label).foregroundStyle(.secondary).frame(width: 92, alignment: .leading)
            UsageBar(pct: pct, stale: false).frame(height: 6)
            Text(UsageText.percent(pct)).monospacedDigit()
                .foregroundStyle(pct > 80 ? Color.orange : .primary)
            if !detail.isEmpty { Text(detail).foregroundStyle(.secondary).lineLimit(1) }
        }
    }

    static func leaf(_ path: String) -> String {
        path.split(separator: "/").last.map(String.init) ?? path
    }

    private static func accountText(_ account: UsageAccount) -> String {
        [account.name, account.usage.reading?.plan].compactMap { $0 }.joined(separator: " · ")
    }

    /// The label of the choice the option in `category` has now.
    private static func current(of category: String, in options: [ConfigOption]) -> String? {
        guard let option = options.first(where: { $0.category == category }),
              case let .select(current, choices) = option.kind else { return nil }
        return choices.first { $0.value == current }?.name ?? current
    }
}

/// An agent's logo from the asset catalog in grey; a plain symbol for an agent with no logo.
private struct AgentGlyph: View {
    let agent: String

    var body: some View {
        Group {
            if ["claude", "codex", "pi"].contains(agent) {
                Image("AgentLogo-\(agent)").renderingMode(.template).resizable().scaledToFit()
                    .foregroundStyle(.secondary)
            } else {
                Image(systemName: "cpu").foregroundStyle(.secondary)
            }
        }
        .frame(width: 16, height: 16)
    }
}

/// A ring filled to `fraction`, orange when `hot`.
private struct ContextRing: View {
    let fraction: Double
    let hot: Bool

    var body: some View {
        ZStack {
            Circle().stroke(Color.primary.opacity(0.12), lineWidth: 2.5)
            Circle().trim(from: 0, to: fraction)
                .stroke(hot ? Color.orange : Color.primary.opacity(0.55), style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
    }
}
