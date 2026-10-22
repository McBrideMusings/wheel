import SwiftUI

// MARK: - Composer

/// The composer every chat shares (ADR-0002): Return sends, Shift-Return adds a line, the
/// paperclip inserts file paths and `/` inserts a skill name. It knows nothing about who it talks
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

    public init(
        placeholder: String,
        draft: Binding<String>,
        error: String?,
        enabled: Bool,
        send: @escaping () -> Void,
        pickFiles: (() -> [String])?,
        onFocus: @escaping () -> Void = {},
        focusRequest: Int = 0
    ) {
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

    static let skills = [
        ("implement", "Plan, edit, verify and review one tracked item"),
        ("review", "Review the changes in a project"),
        ("spike", "Build a throwaway prototype"),
        ("issues", "File or pick the next bead"),
        ("diagnose", "Track down a bug"),
        ("wrap-up", "Commit and land finished work"),
    ]

    public var body: some View {
        VStack(alignment: .leading, spacing: 6) {
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
                    .onKeyPress(.return, phases: .down) { press in
                        if press.modifiers.contains(.shift) { return .ignored }
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
                Menu {
                    ForEach(Self.skills, id: \.0) { skill in
                        Button("/\(skill.0) — \(skill.1)") { insert("/\(skill.0) ") }
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
                if let error {
                    Text(error).font(ChatFont.meta).foregroundStyle(.red).lineLimit(2)
                }
                Spacer()
                Button("Send", systemImage: "arrow.up.circle.fill", action: submit)
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .font(.title2)
                    .disabled(!canSend)
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
