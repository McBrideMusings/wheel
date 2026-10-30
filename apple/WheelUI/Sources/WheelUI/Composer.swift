import SwiftUI
import WheelKit

// MARK: - Composer

/// The composer every chat shares (ADR-0002): Return sends, Shift-Return adds a line, the
/// paperclip inserts file paths, and the agent's skill prefix (`/` for Claude, `$` for Codex) opens
/// a fuzzy list of its skills that the arrow keys, Tab and Return drive. It knows nothing about who it talks
/// to: the caller owns the draft and supplies the send action, which clears the draft once the
/// message goes out.
public struct Composer: View {
    let placeholder: String
    @Binding var draft: String
    let error: String?
    let enabled: Bool
    let send: () -> Void
    /// The platform's file picker; returns the chosen paths. Nil hides the paperclip, for a
    /// platform whose files are not on the machine the agent runs on.
    let pickFiles: (() -> [String])?
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

    public init(
        placeholder: String,
        draft: Binding<String>,
        error: String?,
        enabled: Bool,
        send: @escaping () -> Void,
        pickFiles: (() -> [String])? = nil,
        onFocus: @escaping () -> Void = {},
        focusRequest: Int = 0,
        skills: [SkillInfo] = [],
        skillPrefix: Character? = nil,
        busy: Bool = false,
        stop: @escaping () -> Void = {},
        takeBack: @escaping () -> Bool = { false }
    ) {
        self.takeBack = takeBack
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
                        guard enabled else { return .ignored }
                        if draftIsEmpty, takeBack() { return .handled }
                        guard busy else { return .ignored }
                        stop()
                        return .handled
                    }
                    .onKeyPress(.return, phases: .down) { press in
                        if press.modifiers.contains(.shift) { return .ignored }
                        if chosen, acceptHighlighted() == .handled { return .handled }
                        submit()
                        return .handled
                    }
            }
            HStack(spacing: 6) {
                if let pickFiles {
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

    private var showsStop: Bool { busy && draftIsEmpty }

    private var canSend: Bool { enabled && !draftIsEmpty }

    /// The one send path for Return and the button.
    private func submit() {
        guard !draft.isEmpty else { return }
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
