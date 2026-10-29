import AppKit
import SwiftUI
import WheelKit
import WheelUI

/// The keys that scroll a transcript. The unpin monitor and the debug server's `POST /agent/key`
/// both read this one table.
enum ScrollKey: String, CaseIterable {
    case pageUp, pageDown, home, end, up, down

    var keyCode: UInt16 {
        switch self {
        case .pageUp: 116
        case .pageDown: 121
        case .home: 115
        case .end: 119
        case .up: 126
        case .down: 125
        }
    }

    /// Whether the table view ignores the key, so the transcript moves its own scroll view.
    var tableIgnoresIt: Bool { self != .pageUp && self != .pageDown }

    /// The character a keyboard reports for the key.
    var characters: String {
        switch self {
        case .pageUp: "\u{F72C}"
        case .pageDown: "\u{F72D}"
        case .home: "\u{F729}"
        case .end: "\u{F72B}"
        case .up: "\u{F700}"
        case .down: "\u{F701}"
        }
    }
}

/// A dismissible status over a conversation: what the daemon says about it, not something the
/// conversation said.
struct NoticeBanner: View {
    let text: String
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "info.circle")
                .foregroundStyle(.secondary)
            Text(text)
                .font(ChatFont.meta)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button("Dismiss", systemImage: "xmark") { dismiss() }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help("Dismiss")
        }
        .padding(8)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

/// The controls the agent offers over its own session — model, effort level, permission mode —
/// as a status line under the composer. Each is its value as plain text: a row of bordered
/// pickers costs four lines of the card and reads as a form, when what a person wants is to see
/// what is in force and change it in one click.
struct ConfigRow: View {
    var model: AppModel
    @ObservedObject var chat: SessionChat
    let paneId: String
    let agent: String

    var body: some View {
        // Already in draw order: SessionChat.setConfig sorts once, so this and the state query
        // read the same list.
        HStack(spacing: 10) {
            if let context = chat.context {
                ContextMeter(model: model, paneId: paneId, context: context)
            }
            if model.appearance(forSession: chat.name).home == .row {
                HomeLabel(agent: agent, agents: model.agents)
            }
            Spacer(minLength: 0)
            ForEach(chat.config) { option in
                ConfigControl(option: option) { value in
                    model.setConfig(chat, option, to: value)
                }
            }
        }
    }
}

/// One option as a word you can click. The menu carries the option's name as its header, so the
/// row itself needs no label: `Sonnet` and `Auto` say what they are, and the header and the
/// tooltip name the control for the cases that don't.
struct ConfigControl: View {
    let option: ConfigOption
    let set: (String) -> Void

    var body: some View {
        switch option.kind {
        // A select the agent sent with no choices has nothing to pick from, so it reads as a
        // value rather than a menu that opens onto nothing.
        case let .select(current, choices) where !choices.isEmpty:
            menu(current: current, values: choices.map { ($0.value, $0.name) })
        case let .select(current, _):
            unsettable(current, why: "The agent offers no values for “\(option.name)”.")
        case let .boolean(current):
            menu(current: current ? "true" : "false", values: [("true", "On"), ("false", "Off")])
        case .unknown:
            unsettable(option.name, why: "This version of Wheel can't show a “\(option.name)” control.")
        }
    }

    /// A control shown but not settable: there is nothing to choose, so it does not pretend to
    /// open a menu.
    private func unsettable(_ text: String, why: String) -> some View {
        Text(text)
            .font(ChatFont.meta)
            .foregroundStyle(.tertiary)
            .lineLimit(1)
            .help(why)
    }

    /// The label reads from the agent's own list, so it always shows what the agent took — a pick
    /// the agent refuses or coerces snaps back when its list arrives.
    private func menu(current: String, values: [(value: String, name: String)]) -> some View {
        Menu {
            Section(option.name) {
                ForEach(values, id: \.value) { value in
                    Button {
                        if value.value != current { set(value.value) }
                    } label: {
                        // A real checkmark image, not spaces padded to look like a column:
                        // VoiceOver reads the padding out. An unselected row draws no icon at
                        // all rather than an empty symbol name, which renders as a blank box.
                        // The current value stays clickable, so re-picking it re-asserts it
                        // after the agent moved it itself.
                        Label {
                            Text(value.name)
                        } icon: {
                            if value.value == current { Image(systemName: "checkmark") }
                        }
                    }
                }
            }
        } label: {
            // One weight for the whole line: these are settings in force, not the message, and a
            // long model name in full-strength colour outshouts the composer above it.
            Text(values.first { $0.value == current }?.name ?? current)
                .font(ChatFont.meta)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(option.description.map { "\(option.name): \($0)" } ?? option.name)
    }
}

/// A request's choices, in the order a person reads them: the refusals on the left, the
/// allowances on the right with the one-time allow filled, as the default.
struct PermChoices: View {
    let perm: PermInfo
    let answer: (PermInfo, String) -> Void
    let shortcut: Bool

    var body: some View {
        let rejects = perm.options.filter { $0.kind.hasPrefix("reject") }
        let allows = perm.options.filter { !$0.kind.hasPrefix("reject") }
        let primary = allows.first { $0.kind == "allow_once" } ?? allows.first
        HStack(spacing: 8) {
            ForEach(rejects, id: \.id) { option in
                Button(option.name) { answer(perm, option.id) }
            }
            Spacer(minLength: 8)
            ForEach(allows.filter { $0.id != primary?.id }, id: \.id) { option in
                Button(option.name) { answer(perm, option.id) }
            }
            if let primary {
                Button(primary.name) { answer(perm, primary.id) }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(shortcut ? KeyboardShortcut(.return, modifiers: .command) : nil)
                    .help(shortcut ? "\(primary.name) (⌘↩)" : primary.name)
            }
        }
        .controlSize(.small)
    }
}

/// A request no tool line draws, one at a time above the composer: who asks, what for, the full
/// request, and its choices. `position` of `count` says how many more wait behind it.
struct PermissionCard: View {
    let model: AppModel
    let perm: PermInfo
    let position: Int
    let count: Int
    let shortcut: Bool
    /// Who is asking, when the asker is not an agent session: a web pane's refresh command.
    var asker: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "hand.raised.fill").foregroundStyle(.orange)
                Text(asker ?? "Agent \(perm.session) is asking").font(ChatFont.line).fontWeight(.semibold)
                Spacer()
                if count > 1 {
                    Text("\(position) of \(count)").font(ChatFont.meta).monospacedDigit().foregroundStyle(.secondary)
                }
            }
            // The asking session's own tool line, when its transcript holds one: the kind and
            // content that explain the request. Observed, so a tool_update redraws it.
            if let call = perm.toolCall,
               let chat = model.chats[perm.session] {
                PermissionTool(model: model, chat: chat, call: call, title: perm.title)
            } else {
                PermissionTitle(text: perm.title)
            }
            PermChoices(perm: perm, answer: model.answer, shortcut: shortcut)
        }
        .padding(12)
        .background(.background, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Color.orange.opacity(0.6)))
    }
}

private struct PermissionTitle: View {
    let text: String

    var body: some View {
        Text(text)
            .font(ChatFont.code)
            .textSelection(.enabled)
            .lineLimit(12)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// Finds the asking session's row for a tool call, redrawing when the session's row list changes
/// (the row arrives after the request, or is replaced).
private struct PermissionTool: View {
    let model: AppModel
    @ObservedObject var chat: SessionChat
    let call: String
    let title: String

    var body: some View {
        if let row = chat.rows.last(where: { $0.row.toolCall == call }) {
            PermissionToolRow(model: model, row: row, call: call, title: title)
        } else {
            PermissionTitle(text: title)
        }
    }
}

/// The tool line itself, observing its row so a `tool_update` that replaces the content redraws
/// the card.
private struct PermissionToolRow: View {
    let model: AppModel
    @ObservedObject var row: RowModel
    let call: String
    let title: String

    var body: some View {
        if case let .tool(_, kind, _, content) = row.row.kind {
            Text(action(kind)).font(ChatFont.line).foregroundStyle(.secondary)
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    PermissionTitle(text: title)
                    ForEach(Array(content.enumerated()), id: \.offset) { _, item in ContentItem(item: item) }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxHeight: 220)
            .fixedSize(horizontal: false, vertical: true)
            .onChange(of: content, initial: true) { _, now in model.recordRequestCard(call: call, content: now) }
            .onDisappear { model.clearRequestCard(call: call) }
        } else {
            PermissionTitle(text: title)
        }
    }
}

/// The Mac's file picker for the composer's paperclip.
@MainActor
func pickFiles() -> [String] {
    let panel = NSOpenPanel()
    panel.canChooseDirectories = true
    panel.allowsMultipleSelection = true
    return panel.runModal() == .OK ? panel.urls.map(\.path) : []
}
