import SwiftUI
import WheelKit
import WheelUI

/// An agent pane on the phone: the agent's conversation, the request it waits on, and a composer,
/// all from WheelUI and bound to the agent the daemon holds. The orchestrator's chat in Orchestrator mode has
/// no header row and talks to it by name: the top bar already names it.
struct AgentPane: View {
    let model: PhoneModel
    let pane: String
    let agent: String
    let harness: String
    @ObservedObject var chat: AgentChat
    var orchestrator = false
    @State private var keyboardUp = false

    var body: some View {
        let mine = model.perms.filter { $0.agent == chat.name }
        let tray = model.trays[pane]
        VStack(spacing: 0) {
            if !orchestrator {
                AgentHeader(model: model, pane: pane, agent: agent, harness: harness, chat: chat)
            }
            TranscriptView(
                chat: chat,
                empty: orchestrator ? "Message the orchestrator." : "Give this agent a task.",
                permissions: PermissionQueue(
                    waiting: Set(mine.compactMap(\.toolCall)),
                    places: Dictionary(model.perms.enumerated().compactMap { index, perm in
                        perm.toolCall.map { ($0, index + 1) }
                    }, uniquingKeysWith: { first, _ in first }),
                    count: model.perms.count
                ),
                look: model.appearance(forAgent: chat.name),
                earlier: model.earlier(for: pane)
            )
            .environment(\.chatLinks, model.links)
            // While the keyboard is up, a tap or a drag on the conversation hides it (design §8.1). A layer over the
            // transcript takes them: the list's own scroll and its rows never see a touch in the empty space under a short
            // conversation. The pager is an ancestor, so its sideways swipe still changes page through the layer.
            .overlay {
                if keyboardUp {
                    Color.clear
                        .contentShape(Rectangle())
                        .onTapGesture { hideKeyboard("tap") }
                        .simultaneousGesture(DragGesture(minimumDistance: 8).onChanged { _ in hideKeyboard("drag") })
                        .accessibilityHidden(true)
                }
            }
            .overlay { TaskFullView(chat: chat) { model.readTaskOutput(chat, task: $0) } }
            RunningStrip(chat: chat)
                .padding([.horizontal, .top], 10)
            if let notice = chat.notice {
                Notice(text: notice) { model.dismissNotice(chat) }
                    .padding([.horizontal, .top], 10)
            }
            if let perm = mine.first {
                PermissionCard(perm: perm, count: mine.count, answer: model.answer)
                    .padding([.horizontal, .top], 10)
            }
            Composer(
                placeholder: orchestrator ? "Message" : "Message \(AgentInfo.title(of: agent))",
                draft: $chat.draft,
                error: chat.error,
                enabled: model.isConnected,
                send: { model.send(chat, attachments: tray) },
                // A phone's files are not on the Mac the agent runs on, so a pick is uploaded
                // there first and its Mac path goes into the message; a server too old to take
                // uploads gets no attach control.
                pickFiles: nil,
                attachMenu: tray.map {
                    AnyView(AttachMenu(tray: $0, canAttach: model.canAttach,
                                       clear: AgentPanes.canClear(harness: harness) ? { model.clearChat(chat) } : nil))
                },
                attachments: tray?.chips ?? [],
                removeAttachment: { tray?.remove($0) },
                retryAttachment: { tray?.retry($0) },
                focusRequest: chat.composerFocusRequest,
                skills: chat.accepted(model.agents.first { $0.name == agent }?.skills ?? [], agent: agent),
                skillPrefix: SkillSearch.prefix(ofAgent: agent),
                busy: chat.isBusy,
                stop: { model.cancel(chat) },
                takeBack: { chat.takeBack() },
                readAloud: $chat.readsAloud,
                accessory: AnyView(ModelMenu(chat: chat) { model.setConfig(chat, $0, to: $1) }),
                dictation: chat.name,
                voice: model.voiceOffered
                    ? { model.openVoice(chat, pane: pane, harness: harness, orchestrator: orchestrator) }
                    : nil
            )
            .padding(10)
        }
        // Voice mode covers the pane it was opened in; a pane swiped away from, or a button, leaves the pill.
        .overlay {
            if model.voice.agent == chat.name, model.voice.stage != .off, model.voice.layout == .layer {
                VoiceLayer(model: model, pane: pane, chat: chat, tray: tray, harness: harness, orchestrator: orchestrator)
            }
        }
        .onDisappear {
            if model.voice.agent == chat.name, model.voice.layout == .layer { model.voice.shrink() }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(orchestrator ? "orchestrator chat" : "agent pane \(pane)")
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillShowNotification)) { _ in keyboardUp = true }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) { _ in keyboardUp = false }
    }

    private func hideKeyboard(_ how: String) {
        guard keyboardUp else { return }
        keyboardUp = false
        let hidden = UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
        Log.info("transcript \(how): keyboard \(hidden ? "hidden" : "was not up")")
    }
}

/// A request waiting on a person: why the agent asks, what it asks, and its choices grouped at
/// the right.
struct PermissionCard: View {
    let perm: PermInfo
    let count: Int
    let answer: (PermInfo, String) -> Void
    /// Who is asking, when the asker is not an agent: a web pane's refresh command.
    var asker: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "hand.raised.fill").foregroundStyle(.orange)
                Text(asker ?? "\(perm.agent) is asking").font(.subheadline.weight(.semibold))
                Spacer()
                if count > 1 { Text("+\(count - 1) more").font(.caption).foregroundStyle(.secondary) }
            }
            PermissionWhy(perm: perm)
            if perm.isQuestion {
                Text(perm.title).font(.callout).fixedSize(horizontal: false, vertical: true)
                QuestionChoices(perm: perm, answer: answer).controlSize(.large)
            } else {
                Text(perm.title)
                    .font(.callout.monospaced())
                    .lineLimit(8)
                    .fixedSize(horizontal: false, vertical: true)
                PhonePermissionChoices(perm: perm, answer: answer)
            }
        }
        .padding(12)
        .background(.background, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Color.orange.opacity(0.6)))
        .onAppear {
            Log.info("permission card \(perm.id): reason=\(perm.reason ?? "-") decision=\(perm.decisionReason ?? "-") path=\(perm.blockedPath ?? "-") buttons=\(perm.options.map(\.name).joined(separator: ","))")
        }
    }
}

/// A request's choices for a thumb: Deny and a filled Allow, 44 pt tall and equal, and a ⋯ menu
/// holding every other allowance (Always allow), a button's width from Allow so a mis-tap on
/// Allow cannot grant a rule that lasts.
struct PhonePermissionChoices: View {
    let perm: PermInfo
    let answer: (PermInfo, String) -> Void

    var body: some View {
        let allow = PermissionKey.allow.option(in: perm)
        let rejects = perm.options.filter { $0.kind.hasPrefix("reject") }
        let more = perm.options.filter { !$0.kind.hasPrefix("reject") && $0.id != allow?.id }
        HStack(spacing: 10) {
            ForEach(rejects, id: \.id) { option in
                Button { answer(perm, option.id) } label: { Text(option.name).frame(maxWidth: .infinity, minHeight: 44) }
                    .buttonStyle(.bordered)
            }
            if let allow {
                Button { answer(perm, allow.id) } label: { Text(allow.name).frame(maxWidth: .infinity, minHeight: 44) }
                    .buttonStyle(.borderedProminent)
            }
            if !more.isEmpty {
                Menu {
                    ForEach(more, id: \.id) { option in
                        Button(option.name) { answer(perm, option.id) }
                    }
                } label: {
                    Image(systemName: "ellipsis").frame(width: 44, height: 44)
                }
                .buttonStyle(.bordered)
                .accessibilityLabel("More answers")
            }
        }
        .controlSize(.large)
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
