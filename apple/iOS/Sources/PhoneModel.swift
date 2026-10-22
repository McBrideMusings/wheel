import Foundation
import GhosttyTerminal
import Observation
import Security
import UIKit
import WheelKit
import WheelUI

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
    /// The page of each web pane in the layout, kept across swipes and layout changes.
    private(set) var webPanes: [String: WebPane] = [:]
    private let pageDaemon = PageDaemon()
    /// One conversation per agent pane in the layout, keyed by session name.
    private(set) var chats: [String: SessionChat] = [:]
    /// Permission requests waiting on a person, from every session.
    private(set) var perms: [PermInfo] = []
    /// The agents the Mac can start, from `status`.
    private(set) var agents: [AgentInfo] = []
    /// The daemon's refusal of the last pane-menu op, shown as an alert until dismissed.
    var menuError: String?
    /// The artifacts a pane may show, as the last `artifact_ls` listed them; nil while it is out.
    private(set) var artifacts: [ArtifactInfo]?
    private var artifactReq: UInt64?
    /// Pane-menu ops sent and not yet answered, so only their refusals reach `menuError`.
    private var menuReqs: Set<UInt64> = []
    private var sessionReqs: [UInt64: String] = [:]
    /// The coding agents' usage as the daemon last sent it; the bar under the top bar draws it.
    private(set) var usage = UsageBoard()
    /// A board has arrived since the last connect.
    private(set) var usageLoaded = false
    /// The Settings screen: every setting resolved for `settingsSpace`, each carrying this machine's
    /// value and that project's, as the Mac's window reads them.
    private(set) var settings: [SettingInfo] = []
    private(set) var settingsSpace: String?
    private(set) var settingsLoaded = false
    /// The daemon's reason for refusing a key's last change, by layer and key.
    private(set) var settingRefusals: [SettingField: String] = [:]
    private var settingListReq: UInt64?
    private var settingChangeReqs: [UInt64: SettingField] = [:]
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
    var isConnected: Bool {
        if case .connected = connection { return true }
        return false
    }
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
        pageDaemon.url = nil
        pageDaemon.token = nil
        pageDaemon.pairing = nil
        syncWebPanes()
        chats = [:]
        perms = []
        agents = []
        artifacts = nil
        menuReqs = []
        sessionReqs = [:]
        usage = UsageBoard()
        usageLoaded = false
        settings = []
        settingsLoaded = false
        settingRefusals = [:]
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
        TLSProbe.run(p)
        let session = PinnedSessionDelegate.session(for: p, trace: { Log.info($0) }) { [weak self] in
            Task { @MainActor in
                if let self, self.generation == mine { self.pinFailed() }
            }
        }
        self.session = session
        pageDaemon.url = p.url
        pageDaemon.token = p.token
        pageDaemon.pairing = p
        // A pairing code's token is good once: the daemon spends it and answers with this device's
        // own credential (`.paired`), which every later connection presents.
        let endpoint = DaemonEndpoint(url: p.url, token: DaemonEndpoint.token(p.token), pairingAs: trying == nil ? nil : UIDevice.current.name)
        let client = DaemonClient(endpoint: endpoint, session: session)
        client.log = { Log.info($0) }
        client.onConnection = { [weak self] state in
            guard let self else { return }
            connection = state
            if state != .connected { for terminal in terminals.values { terminal.attached = false } }
            usageLoaded = false
            if case .connected = state {
                settingListReq = nil
                settingChangeReqs = [:]
            }
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
        client.onConnect = { [weak self] status in
            guard let self else { return }
            handle(status)
            // The daemon reads at most once per its interval however often this asks.
            Log.info("usage ask dueOnly=true")
            if let client = self.client { client.send(.usage(req: client.req(), dueOnly: true)) }
        }
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
        case let .status(_, _, _, _, _, perms, spaces, active, agents, _, _):
            self.agents = agents
            // An unsolicited status means this connection missed daemon-wide events.
            self.perms = perms
            let known = Array(chats.values)
            layoutArrived(spaces, active)
            // A fresh connection starts with its first status: attach the page that is showing again.
            if let shownTerm { attach(shownTerm) }
            // The attaches below replace each held transcript with the daemon's current one.
            for chat in known where chats[chat.name] === chat { attach(chat) }
        case let .paired(device, credential):
            guard let code = trying else { break }
            let credentialed = Pairing(url: code.url, token: credential, certSha256: code.certSha256)
            trying = credentialed
            client?.endpoint = DaemonEndpoint(url: code.url, token: DaemonEndpoint.token(credential))
            pageDaemon.token = credential
            pageDaemon.pairing = credentialed
            Log.info("device \(device) paired; holding its credential")
        case let .spaces(spaces, active):
            layoutArrived(spaces, active)
        case let .sessionStateChanged(session, state):
            chats[session]?.state = state
        case let .sessionAttached(req, session, state, events, config, context, now):
            if let req { sessionReqs[req] = nil }
            guard let chat = chats[session] else { return }
            chat.state = state
            chat.reset(events, daemonNow: now)
            chat.setConfig(config)
            chat.context = context
            chat.error = nil
            chat.notice = nil
            Log.info("session \(session) attached, \(events.count) events")
        case let .sessionConfig(session, options):
            chats[session]?.setConfig(options)
        case let .sessionContext(session, context):
            chats[session]?.context = context
        case let .sessionNotice(session, notice):
            chats[session]?.notice = notice
        case let .session(session, event):
            chats[session]?.apply(event)
        case let .sessionQueued(req, _, _):
            sessionReqs[req] = nil
        case let .permRequested(perm):
            if !perms.contains(where: { $0.id == perm.id }) { perms.append(perm) }
            Log.info("permission \(perm.id) waiting: \(perm.title)")
        case let .permResolved(id, choice):
            perms.removeAll { $0.id == id }
            Log.info("permission \(id) resolved: \(choice)")
        case let .paneReload(pane):
            webPanes[pane]?.reload()
        case let .paneUrl(pane, url):
            spaces = spaces.map { space in
                var space = space
                space.root = space.root.replacing(pane: pane, with: .browser(url: url))
                return space
            }
        case let .termAttached(_, id, replayBytes):
            guard id == shownTerm, let terminal = terminals[id] else { return }
            // Every attach (and every resync) replays from scratch: reset the screen first.
            terminal.session.receive(Data("\u{1b}c".utf8))
            terminal.attached = true
            Log.info("attached to \(id), replay \(replayBytes) bytes")
        case let .termOutput(id, data):
            terminals[id]?.session.receive(data)
        case let .termExited(id, code):
            guard let terminal = terminals[id] else { return }
            terminal.attached = false
            Log.info("terminal \(id) exited with \(code.map(String.init) ?? "?")")
            terminal.session.receive(Data("\r\n[shell exited — close this pane and split for a new one]\r\n".utf8))
        case let .usage(_, board), let .usageChanged(board):
            usage = board
            usageLoaded = true
            Log.info("usage board shown=\(board.shown.map(\.id).joined(separator: ",")) pins=\(board.pins.joined(separator: ",")) rows=\(usageRowsText)")
        case let .settings(req, space, list):
            receivedSettings(list, for: space, req: req)
        case let .artifacts(req, list):
            if let req, req == artifactReq {
                artifactReq = nil
                artifacts = list
                Log.info("artifacts: \(list.count) listed")
            }
        case let .ok(req):
            menuReqs.remove(req)
        case let .paneView(req, pane, view, conversation):
            menuReqs.remove(req)
            Log.info("pane \(pane) view \(view?.rawValue ?? "-") conversation \(conversation ?? "-")")
        case let .started(req, pane, session):
            menuReqs.remove(req)
            Log.info("started \(pane) session \(session ?? "-")")
        case let .opened(req, pane, url):
            // The phone shows no page yet, so it reports the navigation the Mac's web view would.
            if menuReqs.remove(req) != nil {
                Log.info("pane \(pane) opened \(url)")
                client?.send(.paneBrowserUrl(req: nil, pane: pane, url: url))
            }
        case let .created(req, id):
            if menuReqs.remove(req) != nil { Log.info("created \(id)") }
        case let .error(req, _, message):
            if let req, let field = settingChangeReqs.removeValue(forKey: req) {
                settingRefusals[field] = message
                Log.info("setting \(field.key) refused: \(message)")
            }
            if let req, menuReqs.remove(req) != nil { menuError = message }
            if let req, req == artifactReq {
                artifactReq = nil
                artifacts = []
            }
            if let req, let session = sessionReqs.removeValue(forKey: req) { chats[session]?.error = message }
            Log.info("daemon error: \(message)")
        default:
            break
        }
    }

    /// Keeps one `WebPane` per web pane in the layout, loading each new one.
    private func syncWebPanes() {
        let artifacts = WebPanes.artifacts(spaces)
        let stores = WebPanes.stores(spaces)
        for (pane, web) in webPanes where artifacts[pane] != web.artifact {
            webPanes[pane] = nil
            Log.info("web pane \(pane) left the layout")
        }
        for (pane, artifact) in artifacts where webPanes[pane] == nil {
            guard let store = stores[pane], let url = WebPanes.pageURL(pane: pane) else { continue }
            let web = WebPane(pane: pane, artifact: artifact, store: store, daemon: pageDaemon)
            webPanes[pane] = web
            web.load(url)
        }
    }

    private func layoutArrived(_ spaces: [SpaceInfo], _ active: String?) {
        self.spaces = spaces
        self.active = active
        Log.info("layout: \(spaces.count) spaces, active \(active ?? "none"), panes \(activeSpace?.root.readingOrder.map(\.id) ?? [])")
        syncWebPanes()
        dropGoneTerminals()
        guard let client else { return }
        let sessions = Set(AgentPanes.sessions(spaces).values)
        for name in chats.keys where !sessions.contains(name) {
            client.send(.sessionDetach(session: name))
            chats[name] = nil
            Log.info("session \(name) left the layout; detached")
        }
        for name in sessions where chats[name] == nil {
            let chat = SessionChat(name: name)
            chats[name] = chat
            attach(chat)
        }
    }

    /// Stream a session's conversation; `session_attached` resets its transcript.
    private func attach(_ chat: SessionChat) {
        guard let client else { return }
        let req = client.req()
        sessionReqs[req] = chat.name
        Log.info("attaching to session \(chat.name)")
        client.send(.sessionAttach(req: req, session: chat.name))
    }

    // MARK: Agent sessions

    /// Send a session's draft. The draft clears once the message goes out; when it can't go out
    /// now, nothing is sent and the draft stays in the box.
    func send(_ chat: SessionChat) {
        let text = chat.draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        guard case .connected = connection, let client else {
            chat.error = "Not connected to the Mac; your message is still in the box."
            return
        }
        chat.error = nil
        chat.pinned = true
        let req = client.req()
        sessionReqs[req] = chat.name
        Log.info("send to \(chat.name), \(text.count) chars")
        client.send(.sessionSend(req: req, session: chat.name, text: text))
        chat.draft = ""
    }

    func cancel(_ chat: SessionChat) {
        Log.info("cancel \(chat.name)")
        client?.send(.sessionCancel(req: client?.req(), session: chat.name))
    }

    func dismissNotice(_ chat: SessionChat) {
        client?.send(.sessionNoticeDismiss(req: nil, session: chat.name))
    }

    /// Allow or deny a waiting request; the daemon broadcasts the result to every client, and the
    /// card goes when it does.
    func answer(_ perm: PermInfo, _ choice: String) {
        Log.info("answering \(perm.id): \(choice)")
        client?.send(.permAnswer(req: client?.req(), id: perm.id, choice: choice))
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
    // MARK: Usage and settings

    /// What the usage bar shows, one `<agent> <account> <row> <percent>` per bar, for the log.
    var usageRowsText: String {
        usage.shown.flatMap { agent in
            agent.accounts.flatMap { account in
                usage.stripRows(for: account, of: agent).map { "\(agent.id) \(account.name) \($0.tag) \(UsageText.percent($0.pct))" }
            }
        }.joined(separator: ", ")
    }

    /// Ask for every setting as it resolves for `space` (nil: the machine-wide view). Each setting
    /// carries this machine's value too, so one list serves the Global and the Project section.
    func askSettings(for space: String?) {
        guard let client else { return }
        let req = client.req()
        settingListReq = req
        if space != settingsSpace { settingsLoaded = false }
        Log.info("settings ls for \(space ?? "no space")")
        client.send(.settingsLs(req: req, space: space))
    }

    /// Write a setting into this machine's `wheel.toml` (`.global`) or the chosen project's.
    /// A blank value removes the key from that file, so the layer below is in force.
    func setSetting(_ field: SettingField, to text: String) {
        guard let client else { return }
        let value = text.trimmingCharacters(in: .whitespaces)
        let req = client.req()
        settingChangeReqs[req] = field
        settingRefusals[field] = nil
        let machine = field.layer == .global
        if value.isEmpty {
            Log.info("setting unset \(field.key) in \(machine ? "this machine's" : "\(settingsSpace ?? "?")'s") wheel.toml")
            client.send(.settingUnset(req: req, key: field.key, space: settingsSpace, machine: machine))
        } else {
            Log.info("setting set \(field.key) = \(value) in \(machine ? "this machine's" : "\(settingsSpace ?? "?")'s") wheel.toml")
            client.send(.settingSet(req: req, key: field.key, value: value, space: settingsSpace, machine: machine))
        }
    }

    /// A list of settings: the answer to a list or a change, or another client's change. A
    /// broadcast for a space other than the one shown may still have changed this machine's value,
    /// so the screen asks again for its own.
    private func receivedSettings(_ list: [SettingInfo], for space: String?, req: UInt64?) {
        Log.info("settings for \(space ?? "no space"): \(list.map { "\($0.key)=\($0.value)(\($0.origin.rawValue))" }.joined(separator: " "))")
        if let req, settingChangeReqs.removeValue(forKey: req) != nil {
            if space == settingsSpace { showSettings(list, for: space) }
        } else if let req, req == settingListReq {
            settingListReq = nil
            showSettings(list, for: space)
        } else if req == nil {
            if space == settingsSpace, settingsLoaded { showSettings(list, for: space) } else if settingsLoaded { askSettings(for: settingsSpace) }
        }
    }

    private func showSettings(_ list: [SettingInfo], for space: String?) {
        settings = list
        settingsSpace = space
        settingsLoaded = true
    }

    // MARK: Pane menu ops. A refusal reaches `menuError`; the layout the daemon broadcasts is what moves the screen.

    private func menuOp(clearsError: Bool = true, _ make: (UInt64) -> ClientMsg) {
        guard let client else { return }
        if clearsError { menuError = nil }
        let req = client.req()
        menuReqs.insert(req)
        let msg = make(req)
        Log.info("pane op: \((try? msg.json()) ?? "?")")
        client.send(msg)
    }

    func split(_ pane: String, _ dir: Dir) { menuOp { .paneSplit(req: $0, pane: pane, dir: dir) } }
    func closePane(_ pane: String) { menuOp { .paneClose(req: $0, pane: pane) } }
    func setKind(_ pane: String, _ kind: NewPaneKind, artifact: ArtifactInfo? = nil, url: String? = nil) {
        menuOp { .paneSetKind(req: $0, pane: pane, kind: kind, artifact: artifact?.name, scope: artifact?.scope, url: url) }
    }
    /// A browser pane on whatever `text` names, as typing it into the Mac's address field does: a bare
    /// host becomes https. An empty text opens the homepage setting.
    func openBrowser(_ pane: String, _ text: String) {
        setKind(pane, .browser)
        if !text.isEmpty { menuOp(clearsError: false) { .paneBrowserOpen(req: $0, pane: pane, text: text) } }
    }
    func startPane(_ pane: String, _ kind: StartKind, agent: String? = nil) {
        menuOp { .paneStart(req: $0, pane: pane, kind: kind, message: nil, agent: agent) }
    }
    func switchView(_ pane: String, to view: ConversationView) { menuOp { .paneView(req: $0, pane: pane, to: view) } }

    /// Ask for the artifacts a pane in the active space may show.
    func listArtifacts() {
        guard let client else { return }
        artifacts = nil
        let req = client.req()
        artifactReq = req
        client.send(.artifactLs(req: req, space: activeSpace?.id, paneOnly: true))
    }

    /// The other way `kind` can show its conversation: a chat for a terminal running Claude Code,
    /// the Claude Code terminal for an agent pane.
    func otherView(_ kind: PaneKind) -> ConversationView? {
        switch kind {
        case let .terminal(_, agent) where agent?.isClaude == true && agents.contains { $0.name == "claude" && $0.found }: .agent
        case let .agent(_, agent): agent == "claude" ? .terminal : nil
        default: nil
        }
    }
}


/// One key in one `wheel.toml`: this machine's, or the chosen project's.
struct SettingField: Hashable {
    enum Layer: String { case global, project }
    let layer: Layer
    let key: String
}
