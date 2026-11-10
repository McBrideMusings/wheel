import AppKit
import SwiftUI
import WheelKit

/// The session dropdown in a main window's title row (ADR-0014): the sessions wheeld holds, the
/// one this window shows checked, and the verbs on sessions. The window's tabs and sidebar list the
/// shown session's spaces.
struct SessionMenu: View {
    var model: AppModel
    let windowID: String

    var body: some View {
        let current = model.session(of: windowID)
        Menu {
            ForEach(model.sessions) { session in
                Button { model.show(session: session.id, in: windowID) } label: {
                    if session.id == current?.id {
                        Label(session.name, systemImage: "checkmark")
                    } else {
                        Text(session.name)
                    }
                }
            }
            Divider()
            Button("New Session") { model.newSession() }
            Button("Rename Session…") { if let current { model.promptRename(session: current) } }
                .disabled(current == nil)
            Button("Close Session…", role: .destructive) { if let current { model.askCloseSession(current) } }
                .disabled(current == nil)
        } label: {
            Image(systemName: "rectangle.stack")
        }
        .menuStyle(.button)
        .menuIndicator(.hidden)
        .fixedSize()
        .help("Session: \(current?.name ?? "none"). Click to switch, make or close one.")
        .accessibilityLabel("Session")
        .accessibilityValue(current?.name ?? "none")
        .titleButtonFrame(model, window: windowID, id: "session")
    }
}

extension AppModel {
    func promptRename(session: SessionInfo) {
        Self.askName(title: "Rename session", message: "", initial: session.name, button: "Rename") { [weak self] name in
            self?.renameSession(session.id, to: name)
        }
    }

    /// What closing `session` ends, as a sentence: its spaces, panes, shells and agents.
    func closeWarning(of session: SessionInfo) -> String {
        let held = spaces(inSession: session.id)
        guard !held.isEmpty else { return "It holds no spaces." }
        let panes = held.flatMap(\.allPanes)
        let shells = panes.filter { if case .terminal = $0.kind { true } else { false } }.count
        let agents = panes.filter { if case .agent = $0.kind { true } else { false } }.count
        func count(_ n: Int, _ noun: String) -> String { "\(n) \(noun)\(n == 1 ? "" : "s")" }
        var text = "It closes \(count(held.count, "space")) and \(count(panes.count, "pane"))"
        var running: [String] = []
        if shells > 0 { running.append(count(shells, "terminal")) }
        if agents > 0 { running.append(count(agents, "agent")) }
        if !running.isEmpty { text += ", and ends \(running.joined(separator: " and "))" }
        return text + ". Every window showing it closes."
    }

    /// Closing a session ends every terminal and agent in its spaces, so it asks first: in a
    /// sheet on the focused window, or in an alert when asked from the menu bar item, which
    /// may have no window open.
    func askCloseSession(_ session: SessionInfo, inAlert: Bool = false) {
        let title = "Close the “\(session.name)” session?"
        let message = closeWarning(of: session)
        if !inAlert, layout.focusedWindow != nil {
            confirm(title, message, button: "Close Session") { [weak self] in self?.closeSession(session.id) }
            return
        }
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Close Session")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate()
        if alert.runModal() == .alertFirstButtonReturn { closeSession(session.id) }
    }

    private static func askName(title: String, message: String, initial: String, button: String, _ done: @escaping (String) -> Void) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.addButton(withTitle: button)
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 240, height: 24))
        field.stringValue = initial
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        done(field.stringValue)
    }
}
