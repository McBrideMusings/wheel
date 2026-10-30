import SwiftUI
import WheelKit
import WheelUI

/// An agent pane on the phone: the agent's conversation, the request it waits on, and a composer,
/// all from WheelUI and bound to the session the daemon holds.
struct AgentPane: View {
    let model: PhoneModel
    let agent: String
    @ObservedObject var chat: SessionChat

    var body: some View {
        let mine = model.perms.filter { $0.session == chat.name }
        VStack(spacing: 0) {
            TranscriptView(
                chat: chat,
                empty: "Give this agent a task.",
                permissions: PermissionQueue(
                    waiting: Set(mine.compactMap(\.toolCall)),
                    places: Dictionary(model.perms.enumerated().compactMap { index, perm in
                        perm.toolCall.map { ($0, index + 1) }
                    }, uniquingKeysWith: { first, _ in first }),
                    count: model.perms.count
                ),
                look: model.appearance(forSession: chat.name)
            )
            .environment(\.chatLinks, model.links)
            if let notice = chat.notice {
                Notice(text: notice) { model.dismissNotice(chat) }
                    .padding([.horizontal, .top], 10)
            }
            if let perm = mine.first {
                PermissionCard(perm: perm, count: mine.count, answer: model.answer)
                    .padding([.horizontal, .top], 10)
            }
            Composer(
                placeholder: "Message \(AgentInfo.title(of: agent))",
                draft: $chat.draft,
                error: chat.error,
                enabled: model.isConnected,
                send: { model.send(chat) },
                // A phone's photos are not on the Mac the agent runs on and the daemon takes no
                // uploads, so there is nothing a picker could hand the agent.
                pickFiles: nil,
                focusRequest: chat.composerFocusRequest,
                skills: chat.accepted(model.agents.first { $0.name == agent }?.skills ?? []),
                skillPrefix: SkillSearch.prefix(ofAgent: agent),
                busy: chat.isBusy,
                stop: { model.cancel(chat) },
                takeBack: { chat.takeBack() }
            )
            .padding(10)
        }
    }
}

/// A request waiting on a person: why the agent asks, what it asks, and its choices grouped at
/// the right.
struct PermissionCard: View {
    let perm: PermInfo
    let count: Int
    let answer: (PermInfo, String) -> Void
    /// Who is asking, when the asker is not an agent session: a web pane's refresh command.
    var asker: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "hand.raised.fill").foregroundStyle(.orange)
                Text(asker ?? "\(perm.session) is asking").font(.subheadline.weight(.semibold))
                Spacer()
                if count > 1 { Text("+\(count - 1) more").font(.caption).foregroundStyle(.secondary) }
            }
            PermissionWhy(perm: perm)
            Text(perm.title)
                .font(.callout.monospaced())
                .lineLimit(8)
                .fixedSize(horizontal: false, vertical: true)
            PermissionChoices(perm: perm, answer: answer, keys: false)
        }
        .padding(12)
        .background(.background, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Color.orange.opacity(0.6)))
        .onAppear {
            Log.info("permission card \(perm.id): reason=\(perm.reason ?? "-") decision=\(perm.decisionReason ?? "-") path=\(perm.blockedPath ?? "-") buttons=\(perm.options.map(\.name).joined(separator: ","))")
        }
    }
}

/// A dismissible status the daemon holds over the conversation.
private struct Notice: View {
    let text: String
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "info.circle").foregroundStyle(.secondary)
            Text(text).font(.footnote).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading)
            Button("Dismiss", systemImage: "xmark", action: dismiss).labelStyle(.iconOnly).buttonStyle(.borderless)
        }
        .padding(8)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}
