import AppKit
import SwiftUI
import Textual
import WheelKit

/// The lead card (DESIGN.md): header, the Waiting-on-you list pinned above the conversation, the
/// transcript, and the composer every chat shares.
struct LeadCard: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(12)
            Divider()
            if !model.boxPerms.isEmpty {
                WaitingList(model: model)
                    .padding([.horizontal, .top], 10)
            }
            TranscriptView(model: model, session: chat.name, rows: chat.transcript.rows, empty: "Ask the lead about any of your projects.")
            Divider()
            Composer(
                placeholder: "Message the lead",
                draft: $chat.draft,
                error: chat.error,
                enabled: model.connection == .connected,
                send: { model.send(chat) },
                pickFiles: pickFiles
            )
            .padding([.horizontal, .top], 10)
            // Under the composer, where a person's eyes and hands already are when they are about
            // to send: what model answers and what it may do without asking belong beside the box
            // you type in, not in a header you read once.
            ConfigRow(model: model, chat: chat)
                .padding(.horizontal, 10)
                .padding(.top, 4)
                .padding(.bottom, 10)
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .background(.background, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.separator))
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "person.crop.circle")
                .font(.title2)
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text("Lead").font(.headline)
                Text(chat.state?.label ?? "Not running")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
            StopButton(model: model, chat: chat)
        }
    }
}

/// The controls the agent offers over its own session — model, effort level, permission mode —
/// as a status line under the composer. Each is its value as plain text: a row of bordered
/// pickers costs four lines of the card and reads as a form, when what a person wants is to see
/// what is in force and change it in one click.
struct ConfigRow: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        // Already in draw order: SessionChat.setConfig sorts once, so this and the state query
        // read the same list.
        HStack(spacing: 10) {
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
            .font(.caption)
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
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(option.description.map { "\(option.name): \($0)" } ?? option.name)
    }
}

/// Every permission request waiting on a person, from any session.
struct WaitingList: View {
    @ObservedObject var model: AppModel

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(model.boxPerms) { perm in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(perm.title)
                            .font(.system(.callout, design: .monospaced))
                            .lineLimit(2)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                        HStack {
                            Text(perm.session).font(.caption).foregroundStyle(.secondary)
                            Spacer()
                            Button("Deny") { model.answer(perm, "deny") }
                            Button("Approve") { model.answer(perm, "allow") }
                        }
                        .controlSize(.small)
                    }
                    if perm.id != model.boxPerms.last?.id { Divider() }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } label: {
            Label("Waiting on you · \(model.boxPerms.count)", systemImage: "hand.raised")
                .font(.caption.weight(.semibold))
        }
    }
}

/// Shown while a session is working; stops its current turn.
struct StopButton: View {
    @ObservedObject var model: AppModel
    @ObservedObject var chat: SessionChat

    var body: some View {
        if case .busy = chat.state {
            Button("Stop", systemImage: "stop.circle") { model.cancel(chat) }
                .labelStyle(.iconOnly)
                .help("Stop the current turn")
        }
    }
}

struct TranscriptView: View {
    @ObservedObject var model: AppModel
    /// The session these rows belong to; a waiting request attaches to its tool row by this and the call id.
    let session: String
    let rows: [TranscriptRow]
    /// What an empty conversation says.
    let empty: String

    var body: some View {
        // Anchored to the bottom: new output stays in view while you are at the end, and leaves
        // you alone while you scroll back through history.
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 10) {
                if rows.isEmpty {
                    Text(empty)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .center)
                        .padding(.top, 40)
                }
                ForEach(rows) { row in
                    RowView(kind: row.kind, perm: row.toolCall.flatMap { model.perm(session: session, toolCall: $0) }, answer: model.answer)
                }
            }
            .padding(12)
        }
        .defaultScrollAnchor(.bottom)
    }
}

struct RowView: View {
    let kind: TranscriptRow.Kind
    /// The request this tool row gates while it waits for a person.
    let perm: PermInfo?
    let answer: (PermInfo, String) -> Void

    var body: some View {
        switch kind {
        case let .user(text, queued):
            VStack(alignment: .trailing, spacing: 2) {
                Text(text)
                    .textSelection(.enabled)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(Color.accentColor.opacity(queued ? 0.07 : 0.15), in: RoundedRectangle(cornerRadius: 10))
                    .foregroundStyle(queued ? .secondary : .primary)
                if queued {
                    Text("Queued").font(.caption2).foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        case let .agent(text):
            // Agents write Markdown, so render it: headings, lists, tables, block quotes and
            // fenced code with its own highlighting. A half-received fence still renders as text,
            // which is what makes this safe to use on a row that grows chunk by chunk.
            StructuredText(markdown: text.trimmingCharacters(in: .whitespacesAndNewlines))
                .textual.textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        case let .tool(title, _, status, _):
            VStack(alignment: .leading, spacing: 6) {
                Label {
                    Text(title).lineLimit(1).truncationMode(.middle)
                    Text(perm == nil ? status.replacingOccurrences(of: "_", with: " ") : "needs permission").foregroundStyle(.secondary)
                } icon: {
                    Image(systemName: perm != nil ? "hand.raised" : status == "completed" ? "checkmark.circle" : status == "failed" ? "xmark.circle" : "wrench.and.screwdriver")
                }
                .font(.system(.caption, design: .monospaced))
                if let perm {
                    HStack {
                        Spacer()
                        Button("Deny") { answer(perm, "deny") }
                        Button("Approve") { answer(perm, "allow") }
                    }
                    .controlSize(.small)
                }
            }
            .padding(6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))
            .overlay { if perm != nil { RoundedRectangle(cornerRadius: 6).strokeBorder(Color.accentColor) } }
        case let .plan(entries):
            VStack(alignment: .leading, spacing: 2) {
                ForEach(Array(entries.enumerated()), id: \.offset) { _, entry in
                    Label(entry.content, systemImage: entry.status == "completed" ? "checkmark.square" : "square")
                        .font(.caption)
                }
            }
        case let .notice(text):
            Text(text).font(.caption).foregroundStyle(.secondary)
        case let .error(text):
            Label(text, systemImage: "exclamationmark.triangle").font(.caption).foregroundStyle(.red)
        }
    }
}

/// The composer every chat shares (ADR-0002): Return sends, Shift-Return adds a line, the
/// paperclip inserts file paths and `/` inserts a skill name. It knows nothing about who it talks
/// to: the caller owns the draft and supplies the send action, which clears the draft once the
/// message goes out.
struct Composer: View {
    let placeholder: String
    @Binding var draft: String
    let error: String?
    let enabled: Bool
    let send: () -> Void
    /// The platform's file picker; returns the chosen paths.
    let pickFiles: () -> [String]

    static let skills = [
        ("implement", "Plan, edit, verify and review one tracked item"),
        ("review", "Review the changes in a project"),
        ("spike", "Build a throwaway prototype"),
        ("issues", "File or pick the next bead"),
        ("diagnose", "Track down a bug"),
        ("wrap-up", "Commit and land finished work"),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ZStack(alignment: .topLeading) {
                if draft.isEmpty {
                    Text(placeholder)
                        .foregroundStyle(.tertiary)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .allowsHitTesting(false)
                }
                TextEditor(text: $draft)
                    .scrollContentBackground(.hidden)
                    .frame(minHeight: 36, maxHeight: 120)
                    .fixedSize(horizontal: false, vertical: true)
                    .onKeyPress(.return, phases: .down) { press in
                        if press.modifiers.contains(.shift) { return .ignored }
                        submit()
                        return .handled
                    }
            }
            HStack(spacing: 4) {
                Button("Attach a file", systemImage: "paperclip") {
                    let paths = pickFiles()
                    if !paths.isEmpty { insert(paths.joined(separator: " ") + " ") }
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help("Insert a file's path into the message")
                Menu {
                    ForEach(Self.skills, id: \.0) { skill in
                        Button("/\(skill.0) — \(skill.1)") { insert("/\(skill.0) ") }
                    }
                } label: {
                    Text("/").font(.system(.body, design: .monospaced).weight(.semibold))
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .help("Run a skill")
                if let error {
                    Text(error).font(.caption).foregroundStyle(.red).lineLimit(2)
                }
                Spacer()
                Button("Send", systemImage: "arrow.up.circle.fill", action: submit)
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .font(.title2)
                    .disabled(!canSend)
            }
        }
    }

    private var canSend: Bool {
        enabled && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// The one send path for Return and the button.
    private func submit() {
        guard !draft.isEmpty else { return }
        send()
    }

    private func insert(_ text: String) {
        draft += (draft.isEmpty || draft.hasSuffix(" ") || draft.hasSuffix("\n") ? "" : " ") + text
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
