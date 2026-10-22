import Foundation
import Observation
import Security
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
    /// One conversation per agent pane in the layout, keyed by session name.
    private(set) var chats: [String: SessionChat] = [:]
    /// Permission requests waiting on a person, from every session.
    private(set) var perms: [PermInfo] = []
    private var sessionReqs: [UInt64: String] = [:]
    /// A pairing being tried; it is saved to the Keychain only once the daemon answers.
    private var trying: Pairing?
    private var client: DaemonClient?
    private var session: URLSession?
    /// Bumped for each connection, so a late callback from a replaced one is ignored.
    private var generation = 0
    /// Why the last pairing was not kept.
    private(set) var pairProblem: String?

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
        chats = [:]
        perms = []
        sessionReqs = [:]
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
        case let .status(_, _, _, _, _, perms, spaces, active, _, _, _):
            // An unsolicited status means this connection missed daemon-wide events.
            self.perms = perms
            let known = Array(chats.values)
            layoutArrived(spaces, active)
            // The attaches below replace each held transcript with the daemon's current one.
            for chat in known where chats[chat.name] === chat { attach(chat) }
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
        case let .paneUrl(pane, url):
            spaces = spaces.map { space in
                var space = space
                space.root = space.root.replacing(pane: pane, with: .browser(url: url))
                return space
            }
        case let .error(req, _, message):
            if let req, let session = sessionReqs.removeValue(forKey: req) { chats[session]?.error = message }
            Log.info("daemon error: \(message)")
        default:
            break
        }
    }

    private func layoutArrived(_ spaces: [SpaceInfo], _ active: String?) {
        self.spaces = spaces
        self.active = active
        Log.info("layout: \(spaces.count) spaces, active \(active ?? "none"), panes \(activeSpace?.root.readingOrder.map(\.id) ?? [])")
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
}
