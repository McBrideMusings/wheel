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

    /// Closing a session ends every terminal and agent in its spaces, so it asks first.
    func askCloseSession(_ session: SessionInfo) {
        let count = session.spaces.count
        confirm("Close the “\(session.name)” session?",
                "It closes \(count == 1 ? "1 space" : "\(count) spaces") and ends every terminal and agent in \(count == 1 ? "it" : "them"). Every window showing it closes.",
                button: "Close Session") { [weak self] in
            self?.closeSession(session.id)
        }
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
