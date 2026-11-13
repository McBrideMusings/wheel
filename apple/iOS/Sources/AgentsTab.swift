import Combine
import SwiftUI
import WheelKit
import WheelUI

/// The sheet's two tabs.
enum ThumbTab: Equatable { case spaces, agents }

/// Which agents the Agents tab lists.
enum AgentScope: Equatable { case thisSpace, all }

/// One agent of the shown session, as the Agents tab draws it: an agent pane, or a terminal pane
/// running an agent the daemon has seen.
struct AgentRow: Identifiable {
    let pane: String
    let agent: String
    let harness: String
    let space: SpaceInfo
    let status: PaneStatus?
    let perm: PermInfo?
    var id: String { pane }

    /// Where the row sorts: waiting on a permission, then stopped, working, done, idle.
    var rank: StatusRank { status?.rank ?? .idle }
}

extension PhoneModel {
    /// Every agent in the shown session's spaces, most in need first: a waiting request, then
    /// stopped, working, done, idle; session order and layout order within a rank. `scope` narrows
    /// it to the shown space. Each agent's status comes from its space, its oldest waiting request
    /// from `perms`.
    func agentRows(scope: AgentScope) -> [AgentRow] {
        let spaces = scope == .all ? sessionSpaces : sessionSpaces.filter { $0.id == active }
        let rows = spaces.flatMap { space in
            space.allPanes.compactMap { pane -> AgentRow? in
                let agent: String, harness: String
                switch pane.kind {
                case let .agent(name, program, _): (agent, harness) = (name, program)
                case let .terminal(_, found?): (agent, harness) = (found.name, found.name)
                default: return nil
                }
                return AgentRow(pane: pane.id, agent: agent, harness: harness, space: space,
                                status: space.paneStatuses[pane.id], perm: perms.first { $0.agent == agent })
            }
        }
        return StatusRank.allCases.flatMap { rank in rows.filter { $0.rank == rank } }
    }

    /// How many of the shown session's agents wait on the person.
    var sessionNeedsYou: Int { Self.needsYou(in: sessionSpaces) }
}

/// Every agent of the shown session as one flat column of full-width tiles, most in need first.
/// The eye button's sheet shows it for the whole session; the title menu's Agents tab shows it too,
/// narrowed by its filter. A tile carries no status dot and nothing an agent declares through the
/// person's own skills: only its pane's status, its conversation's title, its chat and its request.
struct AgentsList: View {
    let model: PhoneModel
    let scope: AgentScope
    /// Where the list shows, for phone.log: `agents sheet` or `agents tab`.
    let place: String
    /// Closes the sheet once a tile has taken the phone to its pane.
    let done: () -> Void

    var body: some View {
        let rows = model.agentRows(scope: scope)
        ScrollView {
            VStack(spacing: 10) {
                if rows.isEmpty {
                    Text(scope == .all ? "No agents in this session" : "No agents in this space")
                        .font(.subheadline).foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity).padding(.vertical, 24)
                        .accessibilityIdentifier("no agents")
                }
                ForEach(rows) { row in
                    AgentTile(model: model, row: row, chat: model.chats[row.agent], place: place, done: done)
                }
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
        }
        .accessibilityIdentifier("agents list")
        .onAppear { Log.info("\(place) \(scope == .all ? "all" : "this space"): \(rows.map { "\($0.agent)=\($0.rank.title)" }.joined(separator: ", "))") }
    }
}

/// One agent: its program's logo (orange when it needs the person), its conversation's title, the
/// system spinner while it works and its space at the right; then its last message to the person,
/// two lines, three while it waits. A waiting request adds the command and Deny, Allow and an
/// open control. A tap anywhere else opens the agent's pane.
private struct AgentTile: View {
    let model: PhoneModel
    let row: AgentRow
    let chat: AgentChat?
    let place: String
    let done: () -> Void

    private var needsYou: Bool { row.rank == .needsYou }
    private var waiting: PermInfo? { needsYou ? row.perm : nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button(action: open) {
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 8) {
                        AgentLogo(agent: row.harness, tint: needsYou ? .orange : nil).frame(width: 18, height: 18)
                        if let chat { ChatTitle(chat: chat, fallback: row.agent) } else { Text(row.agent).font(.body.weight(.semibold)) }
                        Spacer(minLength: 8)
                        if row.rank == .working {
                            ProgressView().controlSize(.small).accessibilityIdentifier("working spinner")
                        }
                        Label(row.space.name, systemImage: "folder")
                            .font(.caption).foregroundStyle(.secondary)
                            .labelStyle(SpaceLabelStyle())
                    }
                    .lineLimit(1)
                    if let chat { LastMessage(chat: chat, lines: needsYou ? 3 : 2) }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("agentrow \(row.agent)")
            if let perm = waiting {
                if perm.isQuestion {
                    Text(perm.title).font(.subheadline).lineLimit(3)
                } else {
                    Text(perm.title).font(.footnote.monospaced()).foregroundStyle(.orange).lineLimit(3)
                        .accessibilityIdentifier("tile command")
                }
                TileChoices(perm: perm, answer: model.answer, open: open)
            }
        }
        .padding(14)
        .background(needsYou ? AnyShapeStyle(Color.orange.opacity(0.08)) : AnyShapeStyle(.background.secondary),
                    in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .strokeBorder(needsYou ? AnyShapeStyle(Color.orange.opacity(0.7)) : AnyShapeStyle(.quaternary)))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("agent tile \(row.agent)")
    }

    private func open() {
        Log.info("\(place): open \(row.agent) in \(row.space.id)/\(row.pane)")
        model.focus(space: row.space.id, pane: row.pane)
        done()
    }
}

/// The space at a tile's right: the folder icon before the name, both muted.
private struct SpaceLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 4) { configuration.icon; configuration.title }
    }
}

/// The conversation's title, held by wheeld; the agent's name until it has a first message.
private struct ChatTitle: View {
    @ObservedObject var chat: AgentChat
    let fallback: String

    var body: some View {
        Text(chat.title ?? fallback).font(.body.weight(.semibold)).foregroundStyle(.primary)
    }
}

/// A waiting request's answers on a tile: Deny, a filled Allow and an open control, equal and 44 pt
/// tall. A question has no Deny or Allow: the open control takes the person to its choices.
private struct TileChoices: View {
    let perm: PermInfo
    let answer: (PermInfo, String) -> Void
    let open: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            if !perm.isQuestion {
                if let deny = PermissionKey.deny.option(in: perm) {
                    Button { answer(perm, deny.id) } label: { Text("Deny").frame(maxWidth: .infinity, minHeight: 44) }
                        .buttonStyle(.bordered)
                        .accessibilityIdentifier("tile deny")
                }
                if let allow = PermissionKey.allow.option(in: perm) {
                    Button { answer(perm, allow.id) } label: { Text("Allow").frame(maxWidth: .infinity, minHeight: 44) }
                        .buttonStyle(.borderedProminent)
                        .accessibilityIdentifier("tile allow")
                }
            }
            Button(action: open) { Image(systemName: "eye").frame(maxWidth: .infinity, minHeight: 44) }
                .buttonStyle(.bordered)
                .accessibilityLabel("Open")
                .accessibilityIdentifier("tile open")
        }
        .controlSize(.large)
    }
}

/// The agent's last message to the person: its latest reply in the transcript, never a tool call,
/// a thought or what the person sent. Read again only when a row is added or removed or the agent's
/// state changes (a turn ends), never per streamed chunk or draft keystroke.
private struct LastMessage: View {
    let chat: AgentChat
    let lines: Int
    @State private var text: String?

    var body: some View {
        Group {
            if let text {
                Text(text).font(.subheadline).foregroundStyle(.secondary).lineLimit(lines)
                    .multilineTextAlignment(.leading)
                    .accessibilityIdentifier("last message")
            }
        }
        .onReceive(chat.$rows.combineLatest(chat.$state)) { _ in
            let latest = Self.text(in: chat.transcript.rows)
            if latest != text { text = latest }
        }
    }

    static func text(in rows: [TranscriptRow]) -> String? {
        for row in rows.reversed() {
            if case let .agent(message) = row.kind {
                let flat = message.split(whereSeparator: \.isNewline).joined(separator: " ").trimmingCharacters(in: .whitespaces)
                if !flat.isEmpty { return flat }
            }
        }
        return nil
    }
}
