import SwiftUI
import WheelKit

// MARK: - Composer

/// The composer every chat shares (ADR-0002): Return sends, Shift-Return adds a line, the
/// paperclip inserts file paths (or, where the files are not on the agent's machine, opens the
/// platform's attach menu and shows each upload as a chip above the text), and the agent's skill prefix (`/` for Claude, `$` for Codex) opens
/// a fuzzy list of its skills that the arrow keys, Tab and Return drive. It knows nothing about who it talks
/// to: the caller owns the draft and supplies the send action, which clears the draft once the
/// message goes out.
public struct Composer: View {
    let placeholder: String
    @Binding var draft: String
    let error: String?
    let enabled: Bool
    let send: () -> Void
    /// The platform's file picker; returns the chosen paths, which go into the draft. For a
    /// platform whose files are on the machine the agent runs on.
    let pickFiles: (() -> [String])?
    /// The attach control for a platform whose files must be uploaded first; shown in place of
    /// the paperclip `pickFiles` draws. Nil with no `pickFiles` shows no attach control.
    var attachMenu: AnyView?
    /// Files on their way to, or stored on, the agent's machine. Send waits until every one is
    /// ready, and a message of attachments alone may go with no text.
    var attachments: [ComposerAttachment] = []
    var removeAttachment: (ComposerAttachment.ID) -> Void = { _ in }
    var retryAttachment: (ComposerAttachment.ID) -> Void = { _ in }
    /// Called when the text box takes keyboard focus, however it got it (a click, Tab).
    var onFocus: () -> Void = {}
    /// Changes when the caller wants the keyboard in the text box.
    var focusRequest = 0
    /// The agent's skills and the character that starts one. Without both, no picker opens.
    var skills: [SkillInfo] = []
    var skillPrefix: Character?
    /// Whether a turn is running. With nothing typed, the send button becomes Stop and Escape stops
    /// the turn; with a draft, send queues the message behind the turn.
    var busy = false
    var stop: () -> Void = {}
    /// Takes back the message the agent has not answered, returning false when there is none. Escape
    /// and Up call it while nothing is typed.
    var takeBack: () -> Bool = { false }
    /// Set while a permission request waits on this conversation: Return with nothing typed allows
    /// it and Escape denies it. Returns whether the request took the key.
    var permissionKey: ((PermissionKey) -> Bool)?

    public init(
        placeholder: String,
        draft: Binding<String>,
        error: String?,
        enabled: Bool,
        send: @escaping () -> Void,
        pickFiles: (() -> [String])? = nil,
        attachMenu: AnyView? = nil,
        attachments: [ComposerAttachment] = [],
        removeAttachment: @escaping (ComposerAttachment.ID) -> Void = { _ in },
        retryAttachment: @escaping (ComposerAttachment.ID) -> Void = { _ in },
        onFocus: @escaping () -> Void = {},
        focusRequest: Int = 0,
        skills: [SkillInfo] = [],
        skillPrefix: Character? = nil,
        busy: Bool = false,
        stop: @escaping () -> Void = {},
        takeBack: @escaping () -> Bool = { false },
        permissionKey: ((PermissionKey) -> Bool)? = nil
    ) {
        self.takeBack = takeBack
        self.permissionKey = permissionKey
        self.busy = busy
        self.stop = stop
        self.skills = skills
        self.skillPrefix = skillPrefix
        self.placeholder = placeholder
        self._draft = draft
        self.error = error
        self.enabled = enabled
        self.send = send
        self.pickFiles = pickFiles
        self.attachMenu = attachMenu
        self.attachments = attachments
        self.removeAttachment = removeAttachment
        self.retryAttachment = retryAttachment
        self.onFocus = onFocus
        self.focusRequest = focusRequest
    }

    @FocusState private var focused: Bool

    @State private var highlighted = 0
    /// Whether the arrow keys chose the highlighted row. Return sends the draft as typed until
    /// they have, so a built-in such as `/clear` is never swapped for a skill; Tab always accepts.
    @State private var chosen = false
    /// A draft the person closed the list on with Escape; the list stays shut until it changes.
    @State private var dismissedDraft: String?

    /// The skills the draft's skill name so far matches, best first; empty when no picker is open.
    private var matches: [SkillInfo] {
        guard let skillPrefix, dismissedDraft != draft,
              let query = SkillSearch.query(in: draft, prefix: skillPrefix)
        else { return [] }
        return SkillSearch.rank(query, in: skills)
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            let matches = matches
            if !matches.isEmpty, let skillPrefix {
                SkillList(matches: matches, prefix: skillPrefix, highlighted: min(highlighted, matches.count - 1)) { accept($0) }
            }
            if !attachments.isEmpty {
                AttachmentChips(attachments: attachments, remove: removeAttachment, retry: retryAttachment)
            }
            ZStack(alignment: .topLeading) {
                if draft.isEmpty {
                    Text(placeholder)
                        .font(ChatFont.body)
                        .foregroundStyle(.tertiary)
                        .padding(.horizontal, 5)
                        .allowsHitTesting(false)
                }
                TextEditor(text: $draft)
                    .font(ChatFont.body)
                    .scrollContentBackground(.hidden)
                    .scrollIndicators(.never)
                    .frame(minHeight: 20, maxHeight: 120)
                    .fixedSize(horizontal: false, vertical: true)
                    .focused($focused)
                    .onChange(of: focused) { _, now in
                        if now { onFocus() }
                    }
                    .onChange(of: focusRequest) { focused = true }
                    .onChange(of: draft) {
                        highlighted = 0
                        chosen = false
                    }
                    .onKeyPress(.downArrow, phases: [.down, .repeat]) { _ in
                        let count = matches.count
                        guard count > 0 else { return .ignored }
                        highlighted = (min(highlighted, count - 1) + 1) % count
                        chosen = true
                        return .handled
                    }
                    .onKeyPress(.upArrow, phases: [.down, .repeat]) { _ in
                        let count = matches.count
                        guard count > 0 else {
                            return enabled && draftIsEmpty && takeBack() ? .handled : .ignored
                        }
                        highlighted = (min(highlighted, count - 1) + count - 1) % count
                        chosen = true
                        return .handled
                    }
                    .onKeyPress(.tab, phases: .down) { _ in acceptHighlighted() }
                    .onKeyPress(.escape, phases: .down) { _ in
                        if !matches.isEmpty {
                            dismissedDraft = draft
                            return .handled
                        }
                        if permissionKey?(.deny) == true { return .handled }
                        guard enabled else { return .ignored }
                        if draftIsEmpty, takeBack() { return .handled }
                        guard busy else { return .ignored }
                        stop()
                        return .handled
                    }
                    .onKeyPress(.return, phases: .down) { press in
                        if press.modifiers.contains(.shift) { return .ignored }
                        if press.modifiers.isEmpty, draft.allSatisfy(\.isWhitespace), attachments.isEmpty, permissionKey?(.allow) == true {
                            return .handled
                        }
                        if chosen, acceptHighlighted() == .handled { return .handled }
                        submit()
                        return .handled
                    }
            }
            HStack(spacing: 6) {
                if let attachMenu {
                    attachMenu
                } else if let pickFiles {
                    Button("Attach a file", systemImage: "paperclip") {
                        let paths = pickFiles()
                        if !paths.isEmpty { insert(paths.joined(separator: " ") + " ") }
                    }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .help("Insert a file's path into the message")
                }
                if let skillPrefix, !skills.isEmpty { Menu {
                    ForEach(skills, id: \.name) { skill in
                        Button("\(String(skillPrefix))\(skill.name) — \(skill.description)") { insert("\(String(skillPrefix))\(skill.name) ") }
                    }
                } label: {
                    Label("Run a skill", systemImage: "slash.circle")
                        .labelStyle(.iconOnly)
                }
                #if os(macOS)
                .menuStyle(.borderlessButton)
                #endif
                .menuIndicator(.hidden)
                .fixedSize()
                .help("Run a skill")
                }
                if let error {
                    Text(error).font(ChatFont.meta).foregroundStyle(.red).lineLimit(2)
                }
                Spacer()
                if showsStop {
                    #if os(macOS)
                    Text("esc").font(ChatFont.meta).foregroundStyle(.tertiary)
                    #endif
                    Button("Stop", systemImage: "stop.circle.fill", action: stop)
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                        .font(.title2)
                        .help("Stop the current turn (Esc)")
                        .disabled(!enabled)
                } else {
                    Button("Send", systemImage: "arrow.up.circle.fill", action: submit)
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                        .font(.title2)
                        .disabled(!canSend)
                        .help(busy ? "Queue: sends when this turn ends" : "Send")
                }
            }
            .foregroundStyle(.secondary)
        }
        .padding(8)
        .background(Color.primary.opacity(0.03), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Color.primary.opacity(focused ? 0.25 : 0.1))
        )
        .contentShape(Rectangle())
        .onTapGesture { focused = true }
    }

    private var draftIsEmpty: Bool {
        draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// Whether there is anything to send: text, or attachments alone.
    private var hasContent: Bool { !draftIsEmpty || !attachments.isEmpty }

    private var attachmentsReady: Bool {
        attachments.allSatisfy { $0.state == .ready }
    }

    private var showsStop: Bool { busy && !hasContent }

    private var canSend: Bool { enabled && hasContent && attachmentsReady }

    /// The one send path for Return and the button.
    private func submit() {
        guard hasContent, attachmentsReady else { return }
        send()
    }

    /// Replace the skill being typed with the highlighted match, or leave the key alone when no
    /// list is open.
    private func acceptHighlighted() -> KeyPress.Result {
        let matches = matches
        guard !matches.isEmpty else { return .ignored }
        accept(matches[min(highlighted, matches.count - 1)])
        return .handled
    }

    private func accept(_ skill: SkillInfo) {
        guard let skillPrefix else { return }
        draft = "\(String(skillPrefix))\(skill.name) "
    }

    private func insert(_ text: String) {
        draft += (draft.isEmpty || draft.hasSuffix(" ") || draft.hasSuffix("\n") ? "" : " ") + text
    }
}

/// The skills a typed prefix matches, best first, with the highlighted one shaded. A click picks
/// a row; the keys are the composer's.
private struct SkillList: View {
    let matches: [SkillInfo]
    let prefix: Character
    let highlighted: Int
    let pick: (SkillInfo) -> Void

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(matches.enumerated()), id: \.element.name) { index, skill in
                        HStack(spacing: 8) {
                            Text("\(String(prefix))\(skill.name)").font(ChatFont.body).fontWeight(.medium)
                            Text(skill.description).font(ChatFont.meta).foregroundStyle(.secondary).lineLimit(1)
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 6)
                        .padding(.vertical, 3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(index == highlighted ? Color.accentColor.opacity(0.18) : .clear, in: RoundedRectangle(cornerRadius: 5))
                        .contentShape(Rectangle())
                        .onTapGesture { pick(skill) }
                        .id(skill.name)
                    }
                }
            }
            .frame(maxHeight: 150)
            .fixedSize(horizontal: false, vertical: true)
            .onChange(of: highlighted) { _, now in
                if matches.indices.contains(now) { proxy.scrollTo(matches[now].name) }
            }
        }
    }
}

/// One file a composer holds for its next message: what the chip shows and where its upload is.
public struct ComposerAttachment: Identifiable, Equatable, Sendable {
    public enum State: Equatable, Sendable {
        /// On its way; the fraction sent when known.
        case uploading(Double?)
        case ready
        case failed(String)
    }

    public let id: UUID
    public var name: String
    /// The size, or a word on where the upload is.
    public var detail: String
    /// Encoded image bytes for the chip's thumbnail; nil shows a document icon.
    public var thumbnail: Data?
    public var state: State

    public init(id: UUID, name: String, detail: String, thumbnail: Data?, state: State) {
        self.id = id
        self.name = name
        self.detail = detail
        self.thumbnail = thumbnail
        self.state = state
    }
}

/// The attachments above the composer's text, in a row that scrolls sideways: a thumbnail or
/// icon, the name, its size or progress, and a button that takes it out. A failed one shows why
/// in red, and a tap on it tries again.
private struct AttachmentChips: View {
    let attachments: [ComposerAttachment]
    let remove: (ComposerAttachment.ID) -> Void
    let retry: (ComposerAttachment.ID) -> Void

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 6) {
                ForEach(attachments) { item in chip(item) }
            }
        }
        .scrollIndicators(.never)
    }

    private func chip(_ item: ComposerAttachment) -> some View {
        let failed: String? = if case let .failed(why) = item.state { why } else { nil }
        return HStack(spacing: 6) {
            ZStack {
                if let data = item.thumbnail, let image = PlatformImage(data: data) {
                    Image(platform: image).resizable().scaledToFill()
                } else {
                    Image(systemName: "doc").foregroundStyle(.secondary)
                }
                if case let .uploading(fraction) = item.state {
                    Color.black.opacity(0.35)
                    if let fraction {
                        ProgressView(value: fraction).progressViewStyle(.circular).tint(.white).controlSize(.small)
                    } else {
                        ProgressView().tint(.white).controlSize(.small)
                    }
                }
            }
            .frame(width: 32, height: 32)
            .background(Color.primary.opacity(0.08))
            .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
            VStack(alignment: .leading, spacing: 1) {
                Text(item.name).font(ChatFont.meta).lineLimit(1).truncationMode(.middle)
                Text(failed ?? item.detail)
                    .font(ChatFont.meta)
                    .foregroundStyle(failed == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(.red))
                    .lineLimit(1)
            }
            .frame(maxWidth: 160, alignment: .leading)
            Button("Remove \(item.name)", systemImage: "xmark") { remove(item.id) }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .font(ChatFont.meta)
                .foregroundStyle(.secondary)
        }
        .padding(.leading, 4)
        .padding(.trailing, 6)
        .padding(.vertical, 4)
        .background(
            (failed == nil ? Color.primary.opacity(0.08) : Color.red.opacity(0.12)),
            in: RoundedRectangle(cornerRadius: 8, style: .continuous)
        )
        .contentShape(Rectangle())
        .onTapGesture { if failed != nil { retry(item.id) } }
        .accessibilityElement(children: .combine)
    }
}
