import AppKit
import SwiftUI
import WheelKit

/// The lead header's `…` menu: New Conversation, and a Resume submenu of the conversations the
/// lead's agent holds for its directory. Built in AppKit rather than as a SwiftUI `Menu` so the
/// debug endpoint can open the very menu the button opens, for a window shot.
@MainActor
enum LeadMenu {
    /// The button the menu drops from, while the card is on screen.
    static weak var anchor: NSView?

    /// The longest title a row shows before it is cut: a title is the conversation's first prompt,
    /// which can be a whole paragraph.
    static let titleLimit = 60

    static func build(_ model: AppModel) -> NSMenu {
        let menu = NSMenu()
        menu.autoenablesItems = false
        let canSwitch = model.canSwitchLead
        let new = ActionItem("New Conversation") { model.newLeadConversation() }
        new.isEnabled = canSwitch
        if !canSwitch { new.toolTip = "The lead is working; stop its turn first." }
        menu.addItem(new)

        let resume = NSMenuItem(title: "Resume", action: nil, keyEquivalent: "")
        resume.submenu = resumeMenu(model, canSwitch: canSwitch)
        menu.addItem(resume)
        return menu
    }

    /// One row per conversation, newest first, each its title over how long ago it was updated;
    /// the one the lead is in carries a checkmark and choosing it changes nothing.
    static func resumeMenu(_ model: AppModel, canSwitch: Bool) -> NSMenu {
        let menu = NSMenu(title: "Resume")
        menu.autoenablesItems = false
        if let error = model.leadSessionsError {
            menu.addItem(note(error))
            return menu
        }
        if model.leadSessions.isEmpty {
            menu.addItem(note("No past conversations"))
            return menu
        }
        for conversation in model.leadSessions {
            let item = ActionItem(rowTitle(conversation)) {
                if !conversation.current { model.resumeLeadConversation(conversation.id) }
            }
            item.subtitle = conversation.age() ?? "no date"
            item.state = conversation.current ? .on : .off
            item.isEnabled = canSwitch
            item.toolTip = conversation.title
            menu.addItem(item)
        }
        menu.addItem(.separator())
        // The agent's store belongs to the directory, so a terminal Claude Code session there
        // lists here too; nothing in an entry says which program made it, so the menu says so.
        let cwd = model.leadSessions.first?.cwd ?? ""
        menu.addItem(note("Includes conversations started outside Wheel in \(cwd)"))
        return menu
    }

    static func rowTitle(_ conversation: ConversationInfo) -> String {
        let title = (conversation.title ?? "").split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
        if title.isEmpty { return "Untitled conversation" }
        return title.count > titleLimit ? String(title.prefix(titleLimit - 1)) + "…" : title
    }

    /// Every row as the menu draws it, for the state query: `✓` marks the current conversation.
    static func outline(_ model: AppModel) -> [Any] {
        build(model).items.map { item -> Any in
            guard let sub = item.submenu else { return text(item) }
            return [item.title: sub.items.filter { !$0.isSeparatorItem }.map(text)]
        }
    }

    private static func text(_ item: NSMenuItem) -> String {
        var s = (item.state == .on ? "✓ " : "") + item.title
        if let sub = item.subtitle { s += " · \(sub)" }
        if !item.isEnabled { s += " (disabled)" }
        return s
    }

    private static func note(_ text: String) -> NSMenuItem {
        let item = NSMenuItem(title: text, action: nil, keyEquivalent: "")
        item.isEnabled = false
        return item
    }

    /// Drop the menu from `view` — the whole menu, or only its Resume submenu, which a script
    /// cannot otherwise open. It reads the list as it stands and asks for a fresh one, which the
    /// next opening shows.
    static func show(_ model: AppModel, from view: NSView, resumeOnly: Bool = false) {
        model.refreshLeadSessions()
        let menu = resumeOnly ? resumeMenu(model, canSwitch: model.canSwitchLead) : build(model)
        Log.info("lead menu opened\(resumeOnly ? " (resume)" : ""), \(model.leadSessions.count) conversations")
        open = menu
        // Runs the menu's own event loop until it closes.
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: view.isFlipped ? view.bounds.maxY + 4 : -4), in: view)
        open = nil
    }

    /// The menu on screen now, so the debug endpoint can close it after a capture.
    static weak var open: NSMenu?
}

/// A menu item that runs a closure.
final class ActionItem: NSMenuItem {
    private let run: () -> Void

    init(_ title: String, run: @escaping () -> Void) {
        self.run = run
        super.init(title: title, action: #selector(fire), keyEquivalent: "")
        target = self
    }

    @available(*, unavailable)
    required init(coder: NSCoder) { fatalError("unused") }

    @objc private func fire() { run() }
}

/// The header's `…` button.
struct LeadMenuButton: NSViewRepresentable {
    let model: AppModel

    func makeNSView(context: Context) -> NSButton {
        let button = NSButton(image: NSImage(systemSymbolName: "ellipsis.circle", accessibilityDescription: "Conversations")!,
                              target: context.coordinator, action: #selector(Coordinator.open(_:)))
        button.isBordered = false
        button.contentTintColor = .secondaryLabelColor
        button.toolTip = "New conversation, or resume a past one"
        button.setContentHuggingPriority(.required, for: .horizontal)
        LeadMenu.anchor = button
        return button
    }

    func updateNSView(_ nsView: NSButton, context: Context) {
        context.coordinator.model = model
    }

    func makeCoordinator() -> Coordinator { Coordinator(model: model) }

    @MainActor
    final class Coordinator: NSObject {
        var model: AppModel

        init(model: AppModel) { self.model = model }

        @objc func open(_ sender: NSButton) { LeadMenu.show(model, from: sender) }
    }
}
