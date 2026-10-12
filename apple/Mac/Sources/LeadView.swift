import AppKit
import SwiftUI
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
            if !model.perms.isEmpty {
                WaitingList(model: model)
                    .padding([.horizontal, .top], 10)
            }
            TranscriptView(rows: chat.transcript.rows, empty: "Ask the lead about any of your projects.")
            Divider()
            Composer(
                placeholder: "Message the lead",
                draft: $chat.draft,
                error: chat.error,
                enabled: model.connection == .connected,
                send: { model.send(chat) },
                pickFiles: pickFiles
            )
            .padding(10)
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

/// Every permission request waiting on a person, from any session.
struct WaitingList: View {
    @ObservedObject var model: AppModel

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(model.perms) { perm in
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
                    if perm.id != model.perms.last?.id { Divider() }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } label: {
            Label("Waiting on you · \(model.perms.count)", systemImage: "hand.raised")
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
                    RowView(kind: row.kind)
                }
            }
            .padding(12)
        }
        .defaultScrollAnchor(.bottom)
    }
}

struct RowView: View {
    let kind: TranscriptRow.Kind

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
            Text(text.trimmingCharacters(in: .whitespacesAndNewlines))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        case let .tool(title, _, status, _):
            Label {
                Text(title).lineLimit(1).truncationMode(.middle)
                Text(status.replacingOccurrences(of: "_", with: " ")).foregroundStyle(.secondary)
            } icon: {
                Image(systemName: status == "completed" ? "checkmark.circle" : status == "failed" ? "xmark.circle" : "wrench.and.screwdriver")
            }
            .font(.system(.caption, design: .monospaced))
            .padding(6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))
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
