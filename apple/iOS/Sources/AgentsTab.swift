import SwiftUI
import WheelKit
import WheelUI

/// The sheet's two tabs.
enum ThumbTab: Equatable { case spaces, agents }

/// Which agents the Agents tab lists.
enum AgentScope: Equatable { case thisSpace, all }

/// One agent of the shown session, as the Agents tab draws it.
struct AgentRow: Identifiable {
    let pane: String
    let agent: String
    let harness: String
    let space: SpaceInfo
    let status: PaneStatus?
    let perm: PermInfo?
    var id: String { pane }

    /// Needs you, then working, then idle, done and stopped.
    enum Group: Int, CaseIterable {
        case needsYou, working, rest

        var title: String {
            switch self {
            case .needsYou: "Needs you"
            case .working: "Working"
            case .rest: "Idle"
            }
        }
    }

    var group: Group {
        switch status {
        case .needsYou: .needsYou
        case .working: .working
        default: .rest
        }
    }
}

extension PhoneModel {
    /// Every agent pane of the shown session's spaces, in session order and layout order within a
    /// space; `scope` narrows it to the shown space. Each agent's status comes from its space, its
    /// oldest waiting request from `perms`.
    func agentRows(scope: AgentScope) -> [AgentRow] {
        let spaces = scope == .all ? sessionSpaces : sessionSpaces.filter { $0.id == active }
        return spaces.flatMap { space in
            space.allPanes.compactMap { pane -> AgentRow? in
                guard case let .agent(agent, harness, _) = pane.kind else { return nil }
                return AgentRow(pane: pane.id, agent: agent, harness: harness, space: space,
                                status: space.paneStatuses[pane.id], perm: perms.first { $0.agent == agent })
            }
        }
    }

    /// How many of the shown session's agents wait on the person.
    var sessionNeedsYou: Int { Self.needsYou(in: sessionSpaces) }
}

/// The Agents tab: the shown session's agents, the ones waiting for the person nearest the thumb
/// at the bottom, then working, then the rest above. A waiting request carries its answers inline.
struct AgentsList: View {
    let model: PhoneModel
    let scope: AgentScope
    /// Closes the sheet once a row has taken the phone to its pane.
    let done: () -> Void

    var body: some View {
        let rows = model.agentRows(scope: scope)
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if rows.isEmpty {
                    Text(scope == .all ? "No agents in this session" : "No agents in this space")
                        .font(.subheadline).foregroundStyle(.secondary).padding(16)
                }
                ForEach(AgentRow.Group.allCases.reversed(), id: \.self) { group in
                    let members = rows.filter { $0.group == group }
                    if !members.isEmpty {
                        Text(group.title.uppercased()).font(.caption).foregroundStyle(.secondary)
                            .padding(.horizontal, 16).padding(.top, 12).padding(.bottom, 4)
                        ForEach(members) { row in
                            AgentRowView(model: model, row: row, chat: model.chats[row.agent], done: done)
                            Divider().padding(.leading, 16)
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .defaultScrollAnchor(.bottom)
        .accessibilityIdentifier("agents list")
        .onAppear { Log.info("agents tab \(scope == .all ? "all" : "this space"): \(rows.map { "\($0.agent)=\($0.group.title)" }.joined(separator: ", "))") }
    }
}

private struct AgentRowView: View {
    let model: PhoneModel
    let row: AgentRow
    let chat: AgentChat?
    let done: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                Log.info("agents tab: open \(row.agent) in \(row.space.id)/\(row.pane)")
                model.focus(space: row.space.id, pane: row.pane)
                done()
            } label: {
                HStack(alignment: .top, spacing: 10) {
                    AgentLogo(agent: row.harness).frame(width: 20, height: 20)
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            StatusDot(status: row.status, shown: Set(StatusRank.allCases))
                            Text(row.agent).font(.body.weight(.semibold)).foregroundStyle(.primary)
                            Text(row.space.name).font(.subheadline).foregroundStyle(.secondary)
                            Spacer(minLength: 8)
                            if let chat { ContextPercent(chat: chat) }
                        }
                        .lineLimit(1)
                        if let perm = row.perm, row.group == .needsYou {
                            Text(perm.title).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(2)
                        } else if let chat {
                            LastLine(chat: chat)
                        }
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("agentrow \(row.agent)")
            if let perm = row.perm, row.group == .needsYou {
                PhonePermissionChoices(perm: perm, answer: model.answer)
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 8)
    }
}

/// The share of the context window in use, once the agent has reported it.
private struct ContextPercent: View {
    @ObservedObject var chat: AgentChat

    var body: some View {
        if let context = chat.context {
            Text("\(context.percent)%").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
        }
    }
}

/// The last line of the conversation: the agent's last words, its last tool call or what the
/// person last sent.
private struct LastLine: View {
    @ObservedObject var chat: AgentChat

    var body: some View {
        if let line = Self.line(of: chat.transcript.rows.last?.kind) {
            Text(line).font(.caption).foregroundStyle(.secondary).lineLimit(1)
        }
    }

    static func line(of kind: TranscriptRow.Kind?) -> String? {
        let text: String
        switch kind {
        case let .user(message, _): text = message
        case let .agent(message): text = message
        case let .thought(message): text = message
        case let .tool(title, _, _, _, _): text = title
        case let .notice(message), let .error(message): text = message
        case .plan, .mode, nil: return nil
        }
        return text.split(whereSeparator: \.isNewline).last.map(String.init)
    }
}
