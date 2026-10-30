import AppKit
import SwiftUI
import WheelKit

/// A destructive step waiting on a yes: closing a pane or a space that runs something, or
/// replacing what a pane runs. The window it was asked in shows it as a sheet.
struct Confirmation: Identifiable {
    let id = UUID()
    let window: String
    let title: String
    let message: String
    let button: String
    let run: @MainActor () -> Void
}

/// What the keyboard actions in `CommandActions` do: moving between panes and spaces, moving
/// dividers, and the confirmations a close asks. A pane header's buttons and the debug endpoint
/// call the same functions.
extension AppModel {
    /// Build the keymap again from the actions there are and the `keybind` setting.
    func rebuildKeymap() {
        let map = Keymap(defaults: CommandActions.defaults(self), setting: keybindSetting)
        guard map != keymap else { return }
        keymap = map
        Log.info("keymap: \(map.chords.count) bound, keybind=\(keybindSetting.isEmpty ? "-" : keybindSetting)")
        for problem in map.problems { Log.info("keymap problem: \(problem)") }
        refreshCommandPalette()
    }

    /// The spaces of the focused window, in its order.
    var windowSpaces: [String] {
        layout.focusedWindow.map { spaces(in: $0.id).map(\.id) } ?? []
    }

    // MARK: Panes

    /// ⌘[ and ⌘]: the pane before or after the focused one, wrapping.
    func cycleFocus(by step: Int) {
        guard let space = activeSpace, let next = PaneNavigation.cycle(from: space.focus, in: space.root, by: step) else { return }
        focusByKeyboard(next)
    }

    /// ⌥⌘ and an arrow: the pane beside the focused one that way, as it is drawn.
    func moveFocus(toward dir: Dir) {
        guard let space = activeSpace, let layout = baseLayouts[space.id] else { return }
        guard let next = PaneNavigation.neighbor(of: space.focus, in: layout, toward: dir) else {
            Log.info("focus \(dir.rawValue): no pane beside \(space.focus)")
            return
        }
        focusByKeyboard(next)
    }

    /// Focus `pane` and move the keyboard into it, as a click inside it does. A terminal takes it
    /// when the daemon's answer arrives; an empty pane takes it when it sees itself focused.
    func focusByKeyboard(_ pane: String) {
        guard let kind = kind(of: pane) else { return }
        Log.info("focus by keyboard: \(pane) (\(kind.name))")
        focus(pane: pane)
        switch kind {
        case let .terminal(term, _): terminals[term]?.view.requestFocus()
        case let .agent(session, _, _): chats[session]?.composerFocusRequest += 1
        case .empty: break
        default:
            // A page has no keyboard of its own to give; the shell that had it must not keep it.
            NSApp.wheelWindow?.makeFirstResponder(nil)
        }
    }

    /// ⌃⌘ and an arrow: move the divider nearest the focused pane across that way by 5%.
    func resizeFocused(toward dir: Dir) {
        guard let space = activeSpace, let (split, ratio) = PaneNavigation.resize(space.focus, in: space.root, toward: dir, step: 0.05) else { return }
        Log.info("resize \(dir.rawValue): split \(split) -> \(ratio)")
        setRatio(split, ratio)
    }

    /// ⌃⌘=: give every pane along each split the same share.
    func equalizeSplits() {
        guard let space = activeSpace else { return }
        for (split, ratio) in PaneNavigation.equalized(space.root) {
            Log.info("equalize: split \(split) -> \(ratio)")
            setRatio(split, ratio)
        }
    }

    // MARK: Spaces

    /// ⇧⌘[ and ⇧⌘]: the space before or after the shown one in this window, wrapping.
    func cycleSpace(by step: Int) {
        let held = windowSpaces
        guard held.count > 1, let active, let i = held.firstIndex(of: active) else { return }
        showSpace(at: ((i + step) % held.count + held.count) % held.count)
    }

    /// ⌘1 to ⌘9: the window's space at `index`.
    func showSpace(at index: Int) {
        let held = windowSpaces
        guard held.indices.contains(index), let window = layout.focusedWindow?.id else { return }
        show(space: held[index], in: window)
    }

    // MARK: Shortcut sheet

    /// ⌘/: show the shortcut sheet over the window, or hide it.
    func toggleShortcuts() {
        guard shortcutsOpen else {
            shortcutsOpen = true
            return Log.info("shortcut sheet: opened")
        }
        closeShortcuts()
    }

    func closeShortcuts() {
        guard shortcutsOpen else { return }
        shortcutsOpen = false
        Log.info("shortcut sheet: closed")
    }

    // MARK: Asking first

    /// Ask in the focused window; the sheet runs `run` on its button.
    func confirm(_ title: String, _ message: String, button: String, run: @escaping @MainActor () -> Void) {
        // The window the sheet shows in; with none open, nothing could answer it.
        guard let window = layout.focusedWindow?.id else {
            Log.info("confirmation not asked, no window to ask in: \(title)")
            return
        }
        confirmation = Confirmation(window: window, title: title, message: message, button: button, run: run)
        Log.info("confirmation asked: \(title)")
    }

    /// The sheet's answer: run the step on a yes, drop it either way.
    func answerConfirmation(_ yes: Bool) {
        guard let pending = confirmation else { return }
        confirmation = nil
        Log.info("confirmation \(yes ? "accepted" : "declined"): \(pending.title)")
        if yes { pending.run() }
    }

    func kind(of pane: String) -> PaneKind? {
        space(holding: pane)?.root.panes.first { $0.id == pane }?.kind
    }

    /// The home directory text for the agent pane running `session`: the home that pane saved, else
    /// the agent's setting or default.
    func shownHome(session: String, agent: String) -> String {
        AgentInfo.shownHome(own: AgentPanes.home(ofSession: session, in: spaces), of: agent, in: agents)
    }

    /// The line that opens a transcript when `chat.session.home` is `transcript`: the agent and its home.
    func homeIntro(session: String, agent: String) -> String {
        "\(agent == "claude" ? "Claude Code" : AgentInfo.title(of: agent)) · \(shownHome(session: session, agent: agent))"
    }

    /// The question a pane that runs a shell or an agent asks before it stops running it.
    private func endingQuestion(_ kind: PaneKind) -> (title: String, message: String)? {
        switch kind {
        case .terminal: ("Kill this pane's shell?", "Whatever is running in it stops.")
        case .agent: ("End this pane's agent session?", "Its conversation and anything it is doing stop.")
        default: nil
        }
    }

    /// ⌘W and a header's ✕: close `pane`, asking first when it runs a shell or an agent.
    func askClosePane(_ pane: String) {
        guard let kind = kind(of: pane) else { return }
        guard let ask = endingQuestion(kind) else { return closePane(pane) }
        confirm(ask.title, ask.message, button: "Close Pane") { [weak self] in self?.closePane(pane) }
    }

    /// ⌥⌘W and a space's ✕: close `space`, asking first when it runs a shell or an agent.
    func askCloseSpace(_ id: String) {
        guard let space = spaces.first(where: { $0.id == id }) else { return }
        var shells = 0, agents = 0
        for pane in space.root.panes {
            if case .terminal = pane.kind { shells += 1 }
            if case .agent = pane.kind { agents += 1 }
        }
        guard shells + agents > 0 else { return closeSpace(id) }
        var parts: [String] = []
        if shells > 0 { parts.append(shells == 1 ? "its terminal's shell is killed" : "its \(shells) terminals' shells are killed") }
        if agents > 0 { parts.append(agents == 1 ? "its agent's session ends" : "its \(agents) agents' sessions end") }
        let text = parts.joined(separator: " and ")
        confirm("Close “\(space.name)”?", text.prefix(1).uppercased() + text.dropFirst() + ".", button: "Close Space") { [weak self] in
            self?.closeSpace(id)
        }
    }

    /// Show a terminal, a browser or the empty picker in `pane`, asking first when that ends what it runs.
    func askSetKind(_ pane: String, _ new: NewPaneKind) {
        guard let kind = kind(of: pane) else { return }
        let run: @MainActor () -> Void = { [weak self] in self?.setKind(pane, new) }
        guard let ask = endingQuestion(kind) else { return run() }
        if new == .empty {
            return confirm(ask.title, "\(ask.message) The pane then shows the empty-pane picker.", button: "Empty Pane", run: run)
        }
        let what = new == .terminal ? "Terminal" : "Browser"
        confirm(ask.title, "\(ask.message) The pane then shows a \(what.lowercased()).", button: "Show \(what)", run: run)
    }

    /// Start agent `agent` in `pane`, asking first when that ends what it runs.
    func askStartAgent(_ pane: String, _ agent: String) {
        guard let kind = kind(of: pane) else { return }
        let run: @MainActor () -> Void = { [weak self] in self?.startPane(pane, .agent, agent: agent) }
        guard let ask = endingQuestion(kind) else { return run() }
        confirm(ask.title, "\(ask.message) The pane then runs \(AgentInfo.title(of: agent)).", button: "Start \(AgentInfo.title(of: agent))", run: run)
    }
}
