import SwiftUI
import WheelKit

/// The header's `+` and history buttons: a new conversation, and the picker over the transcript.
struct LeadConversationButtons: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 6) {
            Button { model.newLeadConversation() } label: { Image(systemName: "plus") }
                .help(model.canSwitchLead ? "New conversation" : "The lead is working; stop its turn first.")
                .disabled(!model.canSwitchLead)
                .accessibilityLabel("New conversation")
            Button { model.toggleLeadPicker() } label: {
                Image(systemName: "clock.arrow.circlepath")
                    .foregroundStyle(model.leadPicker == nil ? Color.secondary : Color.accentColor)
            }
            .help(model.leadPicker == nil ? "Resume a past conversation" : "Back to the conversation")
            .accessibilityLabel("Past conversations")
        }
        .buttonStyle(.borderless)
    }
}

/// The picker that takes the lead card's body in place of the transcript: a search field, and the
/// agent's conversations newest first in a lazily drawn list.
struct LeadConversationPicker: View {
    @ObservedObject var model: AppModel
    let picker: ConversationPicker

    @FocusState private var fieldFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            searchBar
            Divider()
            content
            Divider()
            footer
        }
        .onExitCommand { model.closeLeadPicker() }
        .onAppear { fieldFocused = true }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Past conversations")
    }

    private var searchBar: some View {
        HStack(spacing: 8) {
            Button { model.closeLeadPicker() } label: {
                Label("Back", systemImage: "chevron.left").font(ChatFont.line)
            }
            .buttonStyle(.borderless)
            .help("Back to the conversation (Esc)")
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField(ConversationPicker.placeholder, text: Binding(get: { picker.query }, set: { model.leadPickerTyped($0) }))
                .textFieldStyle(.plain)
                .font(ChatFont.body)
                .focused($fieldFocused)
                .onKeyPress(.upArrow) { model.leadPickerMoved(by: -1); return .handled }
                .onKeyPress(.downArrow) { model.leadPickerMoved(by: 1); return .handled }
                .onSubmit { model.leadPickerEntered() }
                .accessibilityLabel("Search conversations")
            if !picker.query.isEmpty {
                Button { model.leadPickerTyped("") } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary) }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 40)
    }

    @ViewBuilder private var content: some View {
        if picker.all.isEmpty {
            if picker.isLoading {
                VStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(ConversationPicker.loadingNote).font(ChatFont.line).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let error = picker.error {
                note("Couldn’t list conversations", error, symbol: "exclamationmark.circle", tint: .red)
            } else {
                note(ConversationPicker.emptyNote, ConversationPicker.emptyDetail, symbol: "bubble.left.and.bubble.right", tint: .secondary)
            }
        } else if picker.rows.isEmpty {
            note("No conversation matches “\(picker.query)”", "Searched the titles and first messages of \(picker.all.count) conversations.",
                 symbol: "magnifyingglass", tint: .secondary)
        } else {
            list
        }
    }

    private func note(_ title: String, _ detail: String, symbol: String, tint: Color) -> some View {
        VStack(spacing: 6) {
            Image(systemName: symbol).font(.title2).foregroundStyle(tint)
            Text(title).font(ChatFont.line).fontWeight(.semibold)
            Text(detail).font(ChatFont.meta).foregroundStyle(.secondary).textSelection(.enabled)
        }
        .multilineTextAlignment(.center)
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var list: some View {
        ScrollViewReader { proxy in
            // A List draws only the rows on screen and asks for the rest as they scroll in, so 10,000
            // of them cost no more than a screenful. Rows are keyed by their index into the
            // listing, an Int, so there is no string hashing and no intermediate array.
            List {
                ForEach(picker.rows, id: \.self) { index in
                    let conversation = picker.all[index]
                    ConversationRow(conversation: conversation, selected: index == picker.selectedIndex,
                                    current: model.isLeadConversation(conversation.id), enabled: model.canSwitchLead)
                        .onTapGesture { model.leadPickerChose(conversation.id) }
                        .onAppear { if index == picker.rows.first { DispatchQueue.main.async { model.leadPickerPainted() } } }
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color.clear)
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .onChange(of: picker.selectedIndex) { _, index in
                if let index { proxy.scrollTo(index) }
            }
        }
    }

    private var footer: some View {
        let listingFailed = picker.error != nil && !picker.all.isEmpty
        return HStack(spacing: 10) {
            Text(listingFailed ? picker.error ?? "" : picker.footer)
                .foregroundStyle(listingFailed ? Color.red : Color.secondary)
            Spacer(minLength: 4)
            if !model.canSwitchLead { Text("The lead is working; stop its turn to resume one") }
            Text("↑↓ move   ↩ resume   esc back")
        }
        .font(ChatFont.meta)
        .foregroundStyle(.tertiary)
        .lineLimit(1)
        .padding(.horizontal, 14)
        .padding(.vertical, 5)
        .background(.quaternary.opacity(0.5))
    }
}

private struct ConversationRow: View {
    let conversation: ConversationInfo
    let selected: Bool
    let current: Bool
    let enabled: Bool

    var body: some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 1) {
                Text(conversation.rowTitle).font(ChatFont.line).lineLimit(1)
                Text(conversation.age() ?? "no date")
                    .font(ChatFont.meta)
                    .foregroundStyle(selected ? Color.white.opacity(0.85) : Color.secondary)
            }
            Spacer(minLength: 0)
            if current { Image(systemName: "checkmark").font(ChatFont.meta) }
        }
        .foregroundStyle(selected ? Color.white : Color.primary)
        .opacity(enabled || selected ? 1 : 0.55)
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(selected ? Color.accentColor : Color.clear, in: RoundedRectangle(cornerRadius: 7))
        .padding(.horizontal, 6)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? [.isSelected, .isButton] : .isButton)
    }
}
