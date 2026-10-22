import Foundation
import GhosttyTerminal
import Observation
import Security
import WheelKit

/// What the phone knows: the pairing, the daemon connection and the latest layout the daemon sent.
/// The daemon owns the layout (ADR-0001); this model renders it and changes it only by sending ops.
@MainActor @Observable
final class PhoneModel {
    private(set) var pairing: Pairing?
    private(set) var connection: DaemonClient.Connection = .connecting
    /// Set when the Mac presented a certificate other than the one the pairing pins.
    private(set) var certificateMismatch = false
    private(set) var spaces: [SpaceInfo] = []
    private(set) var active: String?
    /// A pairing being tried; it is saved to the Keychain only once the daemon answers.
    private var trying: Pairing?
    private var client: DaemonClient?
    private var session: URLSession?
    /// Bumped for each connection, so a late callback from a replaced one is ignored.
    private var generation = 0
    /// Why the last pairing was not kept.
    private(set) var pairProblem: String?
    /// One Ghostty surface per terminal pane the phone has shown, keyed by the term id the tree names.
    private(set) var terminals: [String: PhoneTerminal] = [:]
    /// The terminal the visible page shows: the only one attached, so the phone's grid size is the
    /// size the shell has while the page is up.
    private var shownTerm: String?
    /// Ghostty's default light/dark theme pair, shared by every surface, with the Mac's padding.
    private let terminalController = TerminalController {
        $0.withWindowPaddingX(10)
        $0.withWindowPaddingY(8)
    }

    var isTrying: Bool { trying != nil }

    var isPaired: Bool { pairing != nil }
    var activeSpace: SpaceInfo? { spaces.first { $0.id == active } ?? spaces.first }

    func start() {
        #if DEBUG
        if let json = ProcessInfo.processInfo.environment["WHEEL_PAIR_JSON"], pairing == nil {
            Log.info("pairing from WHEEL_PAIR_JSON")
            pair(json: json)
            return
        }
        #endif
        if let saved = PairingStore.load(), client == nil {
            pairing = saved
            open(saved)
        }
    }

    /// Tries `json` (a pairing code's text). Returns why it is not one; otherwise connects and, on
    /// the daemon's first answer, keeps it.
    @discardableResult
    func pair(json: String) -> String? {
        let parsed: Pairing
        do { parsed = try Pairing.parse(json) } catch {
            Log.info("pairing refused: \(error.localizedDescription)")
            return error.localizedDescription
        }
        close()
        certificateMismatch = false
        pairProblem = nil
        trying = parsed
        open(parsed)
        return nil
    }

    func unpair() {
        close()
        PairingStore.clear()
        pairing = nil
        trying = nil
        spaces = []
        active = nil
        terminals = [:]
        shownTerm = nil
        Log.info("unpaired")
    }

    private func close() {
        generation += 1
        client?.stop()
        client = nil
        session?.invalidateAndCancel()
        session = nil
    }

    private func open(_ p: Pairing) {
        generation += 1
        let mine = generation
        let session = PinnedSessionDelegate.session(for: p) { [weak self] in
            Task { @MainActor in
                if let self, self.generation == mine { self.pinFailed() }
            }
        }
        self.session = session
        let client = DaemonClient(endpoint: DaemonEndpoint(url: p.url, token: DaemonEndpoint.token(p.token)), session: session)
        client.log = { Log.info($0) }
        client.onConnection = { [weak self] state in
            guard let self else { return }
            connection = state
            if state != .connected { for terminal in terminals.values { terminal.attached = false } }
            if case .connected = state, let trying {
                self.trying = nil
                let status = PairingStore.save(trying)
                if status == errSecSuccess {
                    pairing = trying
                    Log.info("paired with \(trying.url.absoluteString)")
                } else {
                    Log.info("keychain refused the pairing: status \(status)")
                    pairProblem = "Could not keep the pairing in the Keychain (status \(status))."
                    close()
                }
            }
        }
        client.onConnect = { [weak self] status in self?.handle(status) }
        client.onMessage = { [weak self] msg in self?.handle(msg) }
        self.client = client
        client.start()
    }

    private func pinFailed() {
        Log.info("certificate does not match the pinned fingerprint")
        certificateMismatch = true
        close()
        trying = nil
    }

    private func handle(_ msg: ServerMsg) {
        switch msg {
        case let .status(_, _, _, _, _, _, spaces, active, _, _, _), let .spaces(spaces, active):
            self.spaces = spaces
            self.active = active
            dropGoneTerminals()
            // A fresh connection starts with its first status: attach the page that is showing again.
            if case .status = msg, let shownTerm { attach(shownTerm) }
            Log.info("layout: \(spaces.count) spaces, active \(active ?? "none"), panes \(activeSpace?.root.readingOrder.map(\.id) ?? [])")
        case let .paneUrl(pane, url):
            spaces = spaces.map { space in
                var space = space
                space.root = space.root.replacing(pane: pane, with: .browser(url: url))
                return space
            }
        case let .termAttached(_, id, replayBytes):
            guard let terminal = terminals[id] else { return }
            // Every attach (and every resync) replays from scratch: reset the screen first.
            terminal.session.receive(Data("\u{1b}c".utf8))
            terminal.attached = true
            Log.info("attached to \(id), replay \(replayBytes) bytes")
        case let .termOutput(id, data):
            terminals[id]?.session.receive(data)
        case let .termExited(id, code):
            guard let terminal = terminals[id] else { return }
            Log.info("terminal \(id) exited with \(code.map(String.init) ?? "?")")
            terminal.session.receive(Data("\r\n[shell exited — close this pane and split for a new one]\r\n".utf8))
        case let .error(_, _, message):
            Log.info("daemon error: \(message)")
        default:
            break
        }
    }

    // MARK: Ops. Each one asks the daemon; the layout it broadcasts back is what changes the screen.

    func focus(space: String, pane: String) {
        Log.info("focus pane \(pane) in \(space)")
        client?.send(.focus(req: nil, space: space, pane: pane))
    }

    func select(space: String) {
        Log.info("switch to space \(space)")
        client?.send(.focus(req: nil, space: space, pane: nil))
    }

    func createSpace(path: String) {
        let cwd = path.trimmingCharacters(in: .whitespaces)
        guard !cwd.isEmpty, let client else { return }
        Log.info("new space at \(cwd)")
        client.send(.spaceCreate(req: client.req(), cwd: cwd))
    }

    // MARK: Terminals

    /// The surface for `term`, made the first time a page asks.
    func terminal(_ term: String) -> PhoneTerminal {
        if let existing = terminals[term] { return existing }
        let terminal = PhoneTerminal(term: term, controller: terminalController, model: self)
        terminals[term] = terminal
        return terminal
    }

    /// The page showing `pane` came up: attach its terminal, when it is one, and detach the one
    /// the previous page showed.
    func show(pane: String?) {
        let term = activeSpace?.root.readingOrder.first { $0.id == pane }.flatMap { entry -> String? in
            if case let .terminal(term, _) = entry.kind { term } else { nil }
        }
        guard term != shownTerm else { return }
        if let old = shownTerm {
            client?.send(.termDetach(id: old))
            terminals[old]?.attached = false
            Log.info("detached from \(old)")
        }
        shownTerm = term
        if let term {
            attach(term)
            // The keyboard and its key bar come up with the page.
            self.terminal(term).view.requestFocus()
        }
    }

    #if DEBUG
    /// Checks type through the surface as the keyboard would: `text` as a paste, then Return.
    func typeIntoShown(_ text: String) {
        guard let shownTerm, let terminal = terminals[shownTerm] else { return }
        let pasted = terminal.view.paste(text: text), entered = terminal.view.sendKey(.enter)
        Log.info("typed into \(shownTerm): paste \(pasted), return \(entered)")
    }
    #endif

    private func attach(_ term: String) {
        let terminal = self.terminal(term)
        terminal.attached = false
        terminal.size = nil
        Log.info("attaching to \(term)")
        client?.send(.termAttach(req: client?.req(), id: term))
        settle(terminal)
    }

    private func dropGoneTerminals() {
        let held = Set(spaces.flatMap { $0.root.panes.compactMap { pane -> String? in
            if case let .terminal(term, _) = pane.kind { term } else { nil }
        } })
        for term in terminals.keys where !held.contains(term) {
            terminals[term] = nil
            if shownTerm == term { shownTerm = nil }
            Log.info("terminal \(term) left the layout")
        }
    }

    func typed(_ term: String, _ data: Data) {
        guard terminals[term]?.attached == true else { return }
        client?.send(.termInput(req: nil, id: term, data: data))
    }

    func resized(_ term: String, _ vp: InMemoryTerminalViewport) {
        guard vp.columns > 0, vp.rows > 0, let terminal = terminals[term] else { return }
        terminal.reported = vp
        settle(terminal)
    }

    /// Send the surface's grid to the daemon once it fits the page's laid-out size, and only for
    /// the terminal the page shows. A new surface first reports a default frame before SwiftUI
    /// lays it out, which would resize the shell to a grid that is not the phone's.
    /// Called when the surface reports, when the page's layout changes and when it is attached,
    /// whichever comes last.
    func settle(_ terminal: PhoneTerminal) {
        guard terminal.term == shownTerm, terminal.attached || terminal.size == nil, let vp = terminal.reported else { return }
        let cols = Int(vp.columns), rows = Int(vp.rows)
        guard let layout = terminal.layout, vp.widthPixels > 0, vp.heightPixels > 0 else {
            Log.info("surface \(terminal.term) \(cols)x\(rows): held until the page is laid out")
            return
        }
        let slack = PhoneTerminal.paddingPoints * layout.scale
        let dw = layout.size.width * layout.scale - CGFloat(vp.widthPixels)
        let dh = layout.size.height * layout.scale - CGFloat(vp.heightPixels)
        // The surface reports no cell size on iOS (0x0), so the cell is the grid's pixels over its count.
        let cellW = CGFloat(vp.widthPixels) / CGFloat(cols), cellH = CGFloat(vp.heightPixels) / CGFloat(rows)
        let fits = (0...slack + cellW).contains(dw) && (0...slack + cellH).contains(dh)
        Log.info("surface \(terminal.term) \(cols)x\(rows) at \(vp.widthPixels)x\(vp.heightPixels)px in page \(layout.size.width)x\(layout.size.height)pt @\(layout.scale)x: \(fits ? "sent" : "held until laid out")")
        guard fits else { return }
        if let size = terminal.size, size == (cols, rows) { return }
        terminal.size = (cols, rows)
        client?.send(.termResize(req: nil, id: terminal.term, cols: cols, rows: rows))
    }
}
