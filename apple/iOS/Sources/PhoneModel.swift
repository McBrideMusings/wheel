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
    /// Set when the Mac refused the saved credential (the device was revoked there); the phone
    /// asks to pair again instead of retrying.
    private(set) var credentialRefused = false
    private(set) var spaces: [SpaceInfo] = [] {
        didSet { links.layoutChanged(spaces) }
    }
    /// The sessions wheeld holds, in order, each naming its spaces (ADR-0021).
    private(set) var sessions: [SessionInfo] = []
    /// The session and space this phone shows. The daemon keeps no view (ADR-0022): switching here
    /// draws from the layout already held, moves no other client, and the phone reports it with `view`.
    private(set) var view = SessionView()
    /// The space this phone shows; nil while the shown session holds none.
    var active: String? { view.space }
    private var reportedView: String?
    /// Set until the first layout of a launch arrives: that layout restores the view saved last time.
    private var restorePending = true
    /// A space or pane to show whose layout has not arrived yet (the answer to the op that made it can
    /// come ahead of the `spaces` broadcast).
    private var pendingShow: (space: String?, pane: String?) = (nil, nil)
    /// A session made here, to show once a layout names it.
    private var pendingSession: String?
    /// A pane made here, to show once a layout holds it.
    private var pendingPane: String?
    /// What to show once the daemon's answer names the space, pane or session an op made.
    private enum ShowWhenMade { case space, pane, session }
    private var showWhenMade: [UInt64: ShowWhenMade] = [:]
    /// The agent each `pane_split` request was made to start in its new pane, by request id.
    private var startWhenMade: [UInt64: String] = [:]
    /// The page of each web pane in the layout, kept across swipes and layout changes.
    private(set) var webPanes: [String: WebPane] = [:]
    /// The A2UI model of each web pane, drawn natively when its MCP server links an A2UI layout.
    private(set) var a2uiPanes: [String: A2UIPane] = [:]
    private let pageDaemon = PageDaemon()
    /// One conversation per agent pane in the layout, keyed by agent name.
    private(set) var chats: [String: AgentChat] = [:]
    /// The files each agent pane's composer holds for its next message, keyed by pane id.
    private(set) var trays: [String: AttachmentTray] = [:]
    /// The protocol the connected server speaks, from its last status; 0 until one arrives, so
    /// nothing that needs a newer server shows before the server has said.
    private(set) var serverProtocol: UInt32 = 0
    /// Permission requests waiting on a person, from every agent.
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
    private var agentReqs: [UInt64: String] = [:]
    /// The attaches among `agentReqs`, and the agents whose attach the daemon refused (a restore
    /// keeps an agent's program unstarted until its program is found). Every client hears
    /// `agent_state_changed` and `agent_config`, attached or not, so the first one naming a
    /// refused agent is the signal to attach again.
    private var attachReqs: Set<UInt64> = []
    private var refusedAttach: Set<String> = []
    /// The Settings screen: every setting resolved for `settingsSpace`, each carrying this machine's
    /// value and that project's, as the Mac's window reads them.
    private(set) var settings: [SettingInfo] = []
    private(set) var settingsSpace: String?
    private(set) var settingsLoaded = false
    /// The ranks `status.dots` lists: the statuses that draw a dot. Every rank until the daemon says.
    private(set) var statusDots = Set(StatusRank.allCases)
    /// The settings list asked at connect for `status.dots`, so the dots follow the setting with
    /// the Settings sheet closed.
    private var statusDotsReq: UInt64?
    /// How each space's chats draw, from its `chat.*` settings; a chat reads its own space's entry.
    private(set) var appearances = ChatAppearances()
    /// The outstanding settings lists asked for `appearances`, by request.
    private var appearanceReqs: [UInt64: String] = [:]
    /// The daemon's reason for refusing a key's last change, by layer and key.
    private(set) var settingRefusals: [SettingField: String] = [:]
    private var settingListReq: UInt64?
    private var settingChangeReqs: [UInt64: SettingField] = [:]
    /// The New Space sheet's folders: the directories spaces were made for, the browsing roots and
    /// what `dir_list` answered for each folder asked about.
    private(set) var recentDirs: [RecentDir] = []
    private(set) var folderRoots: [String] = []
    private(set) var folderListings: [String: CommandPalette.Listing] = [:]
    private var recentReq: UInt64?
    private var rootsReq: UInt64?
    private var dirReqs: [UInt64: String] = [:]
    /// A pairing being tried; it is saved to the Keychain only once the daemon answers.
    private var trying: Pairing?
    private var client: DaemonClient?
    /// Bumped for each connection, so a late callback from a replaced one is ignored.
    private var generation = 0
    /// Why the last pairing was not kept.
    private(set) var pairProblem: String?
    /// One Ghostty surface per terminal pane the phone has shown, keyed by the term id the tree names.
    private(set) var terminals: [String: PhoneTerminal] = [:]
    /// The terminal the visible page shows: the only one attached, so the phone's grid size is the
    /// size the shell has while the page is up.
    private var shownTerm: String?
    /// The pane the visible page shows. Every layout looks its terminal up again: a pane can turn
    /// into a terminal while it is on screen (a runner splits an empty pane, then starts its shell).
    private var shownPane: String?
    /// One Ghostty controller per space, so a space's `terminal.*` settings restyle its terminals
    /// and no other space's; a terminal keeps the controller of the space it was made in.
    @ObservationIgnored private var terminalControllers: [String: TerminalController] = [:]
    /// How each space's terminals draw, from its `terminal.*` settings.
    @ObservationIgnored private(set) var terminalLooks: [String: TerminalAppearance] = [:]

    var isTrying: Bool { trying != nil }

    var isPaired: Bool { pairing != nil }
    var isConnected: Bool {
        if case .connected = connection { return true }
        return false
    }
    var activeSpace: SpaceInfo? { spaces.first { $0.id == active } }
    /// The session this phone shows.
    var shownSession: SessionInfo? { sessions.first { $0.id == view.session } }
    /// Agents waiting on the person, across every space of every session: the count beside the space name.
    var needsYouCount: Int { Self.needsYou(in: spaces) }

    /// Agents waiting on the person in `spaces`.
    static func needsYou(in spaces: [SpaceInfo]) -> Int {
        spaces.reduce(0) { n, space in
            n + space.paneStatuses.values.filter { if case .needsYou = $0 { true } else { false } }.count
        }
    }
    /// The shown session's spaces, in its order: the only ones the title menu lists.
    var sessionSpaces: [SpaceInfo] {
        let ids = shownSession?.spaces ?? []
        return spaces.filter { ids.contains($0.id) }.sorted { (ids.firstIndex(of: $0.id) ?? 0) < (ids.firstIndex(of: $1.id) ?? 0) }
    }

    /// Whether the composer offers attachments: the server takes uploads from protocol 3 on
    /// (ADR-0020), and a server that predates that hides the paperclip.
    var canAttach: Bool { isConnected && serverProtocol >= ServerProtocol.current }


    /// The upload client for the connected Mac, with this device's current credential.
    private func uploads() -> Uploads? {
        guard isConnected, let pairing = pageDaemon.pairing, let token = pageDaemon.token else { return nil }
        let client = PinnedHTTP(pairing: pairing) {
            Task { @MainActor in Log.info("upload refused: certificate does not match the pinned fingerprint") }
        }
        return Uploads(client: client, daemon: pairing.url, token: token)
    }

    /// Apple's push token for this install, once it has given one.
    private var apnsToken: String?

    /// Keep `token` and give it to the daemon now, and again at every connection.
    func pushToken(_ token: String) {
        apnsToken = token
        registerPush()
    }

    /// Tell the daemon the push token, once connected to a server that takes it (protocol 8).
    private func registerPush() {
        guard let token = apnsToken, let client, serverProtocol >= 8 else { return }
        client.send(.pushRegister(req: client.req(), token: token))
    }

    func start() {
        #if DEBUG
        if let marks = ProcessInfo.processInfo.environment["WHEEL_LOG_MARKS"] { Log.file.writeMarks(marks) }
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
        credentialRefused = false
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
        sessions = []
        view = SessionView()
        restorePending = true
        pendingShow = (nil, nil)
        pendingSession = nil
        pendingPane = nil
        reportedView = nil
        terminals = [:]
        terminalControllers = [:]
        terminalLooks = [:]
        shownTerm = nil
        shownPane = nil
        pageDaemon.url = nil
        pageDaemon.token = nil
        pageDaemon.pairing = nil
        syncWebPanes()
        chats = [:]
        perms = []
        agents = []
        artifacts = nil
        menuReqs = []
        agentReqs = [:]
        attachReqs = []
        refusedAttach = []
        logins = [:]
        settings = []
        settingsLoaded = false
        settingRefusals = [:]
        Log.info("unpaired")
    }

    private func close() {
        generation += 1
        client?.stop()
        client = nil
    }

    private func open(_ p: Pairing) {
        generation += 1
        let mine = generation
        pageDaemon.url = p.url
        pageDaemon.token = p.token
        pageDaemon.pairing = p
        // A pairing code's token is good once: the daemon spends it and answers with this device's
        // own credential (`.paired`), which every later connection presents.
        let endpoint = DaemonEndpoint(url: p.url, token: DaemonEndpoint.token(p.token), pairingAs: trying == nil ? nil : UIDevice.current.name)
        let client = DaemonClient(endpoint: endpoint) { [weak self] url in
            PinnedWebSocket(url: url, pairing: p) {
                Task { @MainActor in
                    if let self, self.generation == mine { self.pinFailed() }
                }
            }
        }
        client.log = { Log.info($0) }
        Log.file.client = UIDevice.current.userInterfaceIdiom == .pad ? "ipad" : "iphone"
        client.appLog = Log.file
        client.onConnection = { [weak self] state in
            guard let self else { return }
            connection = state
            if state != .connected { for terminal in terminals.values { terminal.attached = false } }
            if state != .connected { links.forget() }
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
            registerPush()
            askStatusDots()
            appearances.keep(only: spaces, reset: true)
            appearanceReqs = [:]
            askAppearances()
        }
        client.onMessage = { [weak self] msg in self?.handle(msg) }
        client.onRefused = { [weak self] in
            guard let self, generation == mine else { return }
            if trying != nil {
                Log.info("the Mac refused the pairing code")
                pairProblem = "The Mac refused the pairing code. Make a new one in Settings › Devices."
            } else {
                Log.info("the Mac refused this phone's credential; asking to pair again")
                credentialRefused = true
            }
            trying = nil
            close()
        }
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
        if links.received(msg) { return }
        if a2uiPanes.values.contains(where: { $0.handle(msg) }) { return }
        switch msg {
        case let .status(_, _, _, _, _, perms, spaces, agents, _, _, _, _, protocolVersion, sessions):
            if serverProtocol != protocolVersion {
                Log.info("server protocol \(protocolVersion), this app's \(ServerProtocol.current); attachments \(protocolVersion >= ServerProtocol.current ? "offered" : "hidden")")
            }
            serverProtocol = protocolVersion
            self.agents = agents
            // An unsolicited status means this connection missed daemon-wide events.
            self.perms = perms
            let known = Array(chats.values)
            reportedView = nil
            layoutArrived(spaces, sessions: sessions)
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
        case let .spaces(spaces, sessions):
            layoutArrived(spaces, sessions: sessions)
        case let .spaceFocused(space, pane):
            Log.info("show requested: space \(space) pane \(pane ?? "-")")
            if let pane { focus(space: space, pane: pane) } else { select(space: space) }
        case let .statusChanged(space, pane, status, spaceStatus):
            spaces = spaces.map { $0.id == space ? $0.applying(pane: pane, status: status, spaceStatus: spaceStatus) : $0 }
            Log.info("status \(space)/\(pane ?? "-"): pane=\(status.map(\.label) ?? "none") space=\(spaceStatus.map(\.label) ?? "none")")
        case let .agentStateChanged(agent, state):
            chats[agent]?.state = state
            attachAgainIfRefused(agent)
        case let .agentAttached(req, agent, state, events, config, context, now):
            if let req { agentReqs[req] = nil; attachReqs.remove(req) }
            refusedAttach.remove(agent)
            guard let chat = chats[agent] else { return }
            chat.state = state
            chat.reset(events, daemonNow: now)
            chat.setConfig(config)
            chat.context = context
            chat.error = nil
            chat.notice = nil
            chat.title = nil
            Log.info("agent \(agent) attached, \(events.count) events")
        case let .agentConfig(agent, options):
            attachAgainIfRefused(agent)
            chats[agent]?.setConfig(options)
        case let .agentContext(agent, context):
            chats[agent]?.context = context
        case let .agentCommands(agent, commands):
            chats[agent]?.commands = Set(commands)
        case let .agentTitle(agent, title):
            chats[agent]?.title = title
        case let .paneInfo(_, pane, kind, fields, login):
            paneInfo[pane] = fields
            logins[pane] = PaneHeaderInfo(folder: fields.first { $0.label == "Directory" }?.value, login: login)
            Log.info("pane \(pane) info (\(kind)): \(fields.count) fields")
        case let .toast(id, level, _, _):
            Log.info("toast \(id) (\(level)) not shown: the phone has no toast surface")
        case .toastClear:
            break
        case let .agentNotice(agent, notice):
            chats[agent]?.notice = notice
        case let .agent(agent, event):
            chats[agent]?.apply(event)
        case let .agentQueued(req, _, _):
            agentReqs[req] = nil
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
        case .usage, .usageChanged:
            // A new reading changes the weekly figure of every login a header has asked about.
            if let client { for pane in logins.keys { client.send(.paneInfo(req: client.req(), pane: pane)) } }
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
            spaceReqs.remove(req)
        case let .paneView(req, pane, view, conversation):
            menuReqs.remove(req)
            Log.info("pane \(pane) view \(view?.rawValue ?? "-") conversation \(conversation ?? "-")")
        case let .started(req, pane, agent):
            menuReqs.remove(req)
            Log.info("started \(pane) agent \(agent ?? "-")")
        case let .opened(req, pane, url):
            // The phone shows no page yet, so it reports the navigation the Mac's web view would.
            if menuReqs.remove(req) != nil {
                Log.info("pane \(pane) opened \(url)")
                client?.send(.paneBrowserUrl(req: nil, pane: pane, url: url))
            }
        case let .created(req, id):
            if menuReqs.remove(req) != nil || spaceReqs.remove(req) != nil { Log.info("created \(id)") }
            if let agent = startWhenMade.removeValue(forKey: req) { startPane(id, .agent, agent: agent) }
            switch showWhenMade.removeValue(forKey: req) {
            case .space?: select(space: id)
            case .session?: selectSession(id)
            case .pane?:
                if let space = spaces.first(where: { $0.allPanes.contains { $0.id == id } }) { focus(space: space.id, pane: id) } else { pendingPane = id }
            case nil: break
            }
        case let .error(req, _, message):
            if let req, let field = settingChangeReqs.removeValue(forKey: req) {
                settingRefusals[field] = message
                Log.info("setting \(field.key) refused: \(message)")
            }
            if let req { startWhenMade[req] = nil }
            if let req, menuReqs.remove(req) != nil { menuError = message }
            if let req, spaceReqs.remove(req) != nil {
                spaceError = message
                showWhenMade[req] = nil
            }
            if let req, req == artifactReq {
                artifactReq = nil
                artifacts = []
            }
            if let req, let agent = agentReqs.removeValue(forKey: req) {
                chats[agent]?.error = message
                if attachReqs.remove(req) != nil { refusedAttach.insert(agent) }
            }
            Log.info("daemon error: \(message)")
        case let .spaceRecent(req, dirs):
            guard req == recentReq else { break }
            recentReq = nil
            recentDirs = dirs
            Log.info("new space: \(dirs.count) recent directories listed")
        case let .dirRoots(req, roots):
            guard req == rootsReq else { break }
            rootsReq = nil
            folderRoots = roots
            Log.info("new space: roots \(roots)")
            for root in roots { askDirList(root) }
        case let .dirList(req, path, dirs, error):
            guard let parent = dirReqs.removeValue(forKey: req) else { break }
            folderListings[CommandPalette.listingKey(parent)] = .init(dirs: dirs, error: error)
            Log.info("new space: \(parent) -> \(path): \(dirs.count) folders\(error.map { ", error \($0)" } ?? "")")
        default:
            break
        }
    }

    /// Keeps one `WebPane` per web pane in the layout, loading each new one.
    private func syncWebPanes() {
        let wanted = WebPanes.panes(spaces)
        for (pane, web) in webPanes where wanted[pane]?.artifact != web.artifact {
            webPanes[pane] = nil
            a2uiPanes[pane] = nil
            Log.info("web pane \(pane) left the layout")
        }
        for (pane, shown) in wanted where webPanes[pane] == nil {
            guard let url = WebPanes.pageURL(pane: pane) else { continue }
            let web = WebPane(pane: pane, artifact: shown.artifact, store: shown.store, daemon: pageDaemon)
            webPanes[pane] = web
            web.load(url)
            if let client {
                let space = spaces.first { $0.allPanes.contains { $0.id == pane } }?.id
                let a2ui = A2UIPane(pane: pane, artifact: shown.artifact, space: space, send: { client.send($0) }, nextReq: { client.req() })
                a2uiPanes[pane] = a2ui
                a2ui.start()
            }
        }
    }

    private func layoutArrived(_ spaces: [SpaceInfo], sessions: [SessionInfo]) {
        // The layout broadcast leaves statuses out; `status_changed` carries them. The daemon keeps no
        // focused pane, so each space keeps the one this phone had, while that pane is there.
        let held = Dictionary(uniqueKeysWithValues: self.spaces.map { ($0.id, $0) })
        self.spaces = spaces.map { space in
            var space = space.keepingStatuses(of: held[space.id])
            if let was = held[space.id] { space.focus = was.focusKept(in: space.root) }
            return space
        }
        let old = self.sessions
        self.sessions = sessions
        // A layout with no sessions (a server still loading) restores nothing and saves nothing.
        if restorePending, !sessions.isEmpty {
            restorePending = false
            restoreView()
        } else {
            view.layoutChanged(from: old, to: sessions)
        }
        if let space = pendingShow.space, self.spaces.contains(where: { $0.id == space }) {
            let pane = pendingShow.pane
            pendingShow = (nil, nil)
            if let pane { focus(space: space, pane: pane) } else { select(space: space) }
        }
        if let pane = pendingPane, let space = self.spaces.first(where: { $0.allPanes.contains { $0.id == pane } }) {
            pendingPane = nil
            focus(space: space.id, pane: pane)
        }
        if let session = pendingSession, sessions.contains(where: { $0.id == session }) {
            pendingSession = nil
            selectSession(session)
        }
        reportView()
        Log.info("layout: \(spaces.count) spaces, active \(active ?? "none"), panes \(activeSpace?.root.readingOrder.map(\.id) ?? []), statuses \(statusText)")
        Log.info("sessions: \(sessions.map { "\($0.id)=\($0.name)[\($0.spaces.joined(separator: ","))]" }.joined(separator: " ")); shown \(view.session ?? "none")")
        appearances.keep(only: self.spaces)
        askAppearances()
        syncWebPanes()
        dropGoneTerminals()
        showTerm()
        // One tray per agent pane and agent: a pane that stops showing an agent, or shows a
        // new agent under the same id, had its uploads deleted on the Mac, so its chips go too.
        let agentPanes = AgentPanes.agents(spaces)
        for (pane, tray) in trays where agentPanes[pane] != tray.agent {
            trays[pane] = nil
            Log.info("pane \(pane) no longer shows agent \(tray.agent); its attachments dropped")
        }
        for (pane, agent) in agentPanes where trays[pane] == nil {
            let tray = AttachmentTray(pane: pane, agent: agent)
            tray.uploads = { [weak self] in self?.uploads() }
            trays[pane] = tray
        }
        guard let client else { return }
        let agents = Set(AgentPanes.agents(spaces).values)
        for name in chats.keys where !agents.contains(name) {
            client.send(.agentDetach(agent: name))
            chats[name] = nil
            Log.info("agent \(name) left the layout; detached")
        }
        for name in agents where chats[name] == nil {
            let chat = AgentChat(name: name)
            chat.sendUnsend = { [weak self, weak chat] turn in
                if let chat { self?.unsend(chat, turn: turn) }
            }
            chat.sendClearQueue = { [weak self, weak chat] in
                if let chat { self?.clearQueue(chat) }
            }
            chats[name] = chat
            attach(chat)
        }
    }

    /// The daemon has just said something about `agent`: when its attach was refused earlier, the
    /// agent exists now, so attach again.
    private func attachAgainIfRefused(_ agent: String) {
        guard refusedAttach.remove(agent) != nil, let chat = chats[agent] else { return }
        Log.info("agent \(agent) is there now; attaching again")
        attach(chat)
    }

    /// Stream an agent's conversation; `agent_attached` resets its transcript.
    private func attach(_ chat: AgentChat) {
        guard let client else { return }
        let req = client.req()
        agentReqs[req] = chat.name
        attachReqs.insert(req)
        Log.info("attaching to agent \(chat.name)")
        client.send(.agentAttach(req: req, agent: chat.name))
    }

    // MARK: Agents

    /// Send an agent's draft, with the Mac paths of the files in `tray` after it, one per line.
    /// The draft and the tray clear once the message goes out; when it can't go out now (or a file
    /// is still on its way), nothing is sent and both stay.
    func send(_ chat: AgentChat, attachments tray: AttachmentTray? = nil) {
        let typed = chat.draft.trimmingCharacters(in: .whitespacesAndNewlines)
        let paths = tray?.paths ?? []
        if let tray, !tray.ready {
            chat.error = "A file is still uploading or failed; your message is still in the box."
            return
        }
        let text = composerMessage(typed: typed, paths: paths)
        guard !text.isEmpty else { return }
        guard case .connected = connection, let client else {
            chat.error = "Not connected to the Mac; your message is still in the box."
            return
        }
        chat.error = nil
        chat.pinned = true
        let req = client.req()
        agentReqs[req] = chat.name
        Log.info("send to \(chat.name), \(text.count) chars, \(paths.count) attachments")
        client.send(.agentSend(req: req, agent: chat.name, text: text))
        chat.draft = ""
        tray?.sent()
    }

    func cancel(_ chat: AgentChat) {
        Log.info("cancel \(chat.name)")
        client?.send(.agentCancel(req: client?.req(), agent: chat.name))
    }

    func unsend(_ chat: AgentChat, turn: UInt64) {
        Log.info("unsend \(chat.name) turn \(turn)")
        chat.error = nil
        guard let client else { return }
        let req = client.req()
        agentReqs[req] = chat.name
        client.send(.agentUnsend(req: req, agent: chat.name, turn: turn))
    }

    func clearQueue(_ chat: AgentChat) {
        Log.info("clear queue \(chat.name)")
        chat.error = nil
        guard let client else { return }
        let req = client.req()
        agentReqs[req] = chat.name
        client.send(.agentClearQueue(req: req, agent: chat.name))
    }

    func dismissNotice(_ chat: AgentChat) {
        client?.send(.agentNoticeDismiss(req: nil, agent: chat.name))
    }

    /// Allow or deny a waiting request; the daemon broadcasts the result to every client, and the
    /// card goes when it does.
    func answer(_ perm: PermInfo, _ choice: String) {
        Log.info("answering \(perm.id): \(choice)")
        client?.send(.permAnswer(req: client?.req(), id: perm.id, choice: choice))
    }

    // MARK: Ops. Each one asks the daemon; the layout it broadcasts back is what changes the screen.

    /// Show `pane` of `space` on this phone. Nothing is sent but the `view` report.
    func focus(space: String, pane: String) {
        Log.info("focus pane \(pane) in \(space)")
        guard let i = spaces.firstIndex(where: { $0.id == space && $0.allPanes.contains { $0.id == pane } }) else {
            pendingShow = (space, pane)
            return
        }
        if spaces[i].focus != pane { spaces[i].focus = pane }
        view.show(space: space, in: sessions)
        reportView()
        showTerm()
    }

    /// Show `space` on this phone: no other client moves.
    func select(space: String) {
        Log.info("switch to space \(space)")
        guard spaces.contains(where: { $0.id == space }) else {
            pendingShow = (space, nil)
            return
        }
        view.show(space: space, in: sessions)
        reportView()
        showTerm()
    }

    /// Show `session`: the space this phone last showed in it, else its first; nothing is sent but `view`.
    func selectSession(_ session: String) {
        guard sessions.contains(where: { $0.id == session }) else {
            pendingSession = session
            return
        }
        view.show(session: session, in: sessions)
        Log.info("switch to session \(session): space \(view.space ?? "none")")
        reportView()
        showTerm()
    }

    /// The view saved at the last launch for this Mac, restored once the first layout is here.
    private func restoreView() {
        let saved = pairing.flatMap { UserDefaults.standard.dictionary(forKey: Self.viewKey($0)) as? [String: String] } ?? [:]
        let (restored, took) = SessionView.restored(session: saved["session"], space: saved["space"], in: sessions)
        view = restored
        if let pane = saved["pane"], let i = spaces.firstIndex(where: { $0.id == restored.space }), spaces[i].allPanes.contains(where: { $0.id == pane }) {
            spaces[i].focus = pane
        }
        Log.info("restored last view session=\(saved["session"] ?? "-") space=\(saved["space"] ?? "-") pane=\(saved["pane"] ?? "-"): took \(took), showing \(view.session ?? "none")/\(view.space ?? "none")/\(activeSpace?.focus ?? "none")")
    }

    /// Each Mac's ids are its own, so the view is saved per paired daemon.
    private static func viewKey(_ pairing: Pairing) -> String { "lastView \(pairing.url.absoluteString)" }

    /// Tell the daemon what this phone shows (`client_ls` lists it); only a change is sent.
    private func reportView() {
        let space = activeSpace
        let key = "\(view.session ?? "-")/\(space?.id ?? "-")/\(space?.focus ?? "-")"
        guard key != reportedView else { return }
        reportedView = key
        if let pairing, view.session != nil {
            var saved: [String: String] = [:]
            saved["session"] = view.session
            saved["space"] = space?.id
            saved["pane"] = space?.focus
            UserDefaults.standard.set(saved, forKey: Self.viewKey(pairing))
        }
        client?.send(.view(req: nil, window: "phone", space: space?.id, pane: space?.focus, session: view.session))
    }

    func createSpace(path: String) {
        let cwd = path.trimmingCharacters(in: .whitespaces)
        guard !cwd.isEmpty else { return }
        Log.info("new space at \(cwd)")
        let session = view.session
        spaceOp(showing: .space) { .spaceCreate(req: $0, cwd: cwd, focus: false, session: session) }
    }

    // MARK: Space and session ops, from the title menu. A refusal reaches `spaceError`.

    /// The daemon's refusal of the last space or session op, shown as an alert until dismissed.
    var spaceError: String?
    private var spaceReqs: Set<UInt64> = []

    private func spaceOp(showing: ShowWhenMade? = nil, _ make: (UInt64) -> ClientMsg) {
        guard let client else { return }
        spaceError = nil
        let req = client.req()
        spaceReqs.insert(req)
        if let showing { showWhenMade[req] = showing }
        let msg = make(req)
        Log.info("space op: \((try? msg.json()) ?? "?")")
        client.send(msg)
    }

    func renameSpace(_ space: String, to name: String) {
        spaceOp { .spaceRename(req: $0, space: space, name: name.trimmingCharacters(in: .whitespaces)) }
    }
    func closeSpace(_ space: String) { spaceOp { .spaceClose(req: $0, id: space) } }
    func moveSpace(_ space: String, toSession session: String) { spaceOp { .sessionMove(req: $0, space: space, session: session) } }
    /// A new pane beside the space's shown pane, shown once it is made.
    func newPane(in space: String) {
        guard let pane = spaces.first(where: { $0.id == space })?.focus else { return }
        spaceOp(showing: .pane) { .paneSplit(req: $0, pane: pane, dir: .right, focus: false) }
    }
    /// A new empty session, shown once it is made.
    func newSession(named name: String) {
        spaceOp(showing: .session) { .sessionNew(req: $0, name: name.trimmingCharacters(in: .whitespaces)) }
    }

    // MARK: New Space folders

    /// Ask for the recent directories and the browsing roots (and, from the roots' answer, each root's subfolders).
    func askFolders() {
        guard let client else { return }
        let recent = client.req()
        recentReq = recent
        client.send(.spaceRecent(req: recent))
        let roots = client.req()
        rootsReq = roots
        client.send(.dirRoots(req: roots))
    }

    /// Ask the daemon for the subfolders of `path`; the answer lands in `folderListings`.
    func askDirList(_ path: String) {
        guard let client, !dirReqs.values.contains(path) else { return }
        let req = client.req()
        dirReqs[req] = path
        client.send(.dirList(req: req, path: path))
    }

    /// Forget the open requests when the sheet closes, so a late answer is ignored.
    func stopAskingFolders() {
        recentReq = nil
        rootsReq = nil
        dirReqs = [:]
    }

    // MARK: Terminals

    /// The surface for `term`, made the first time a page asks.
    func terminal(_ term: String) -> PhoneTerminal {
        if let existing = terminals[term] { return existing }
        let terminal = PhoneTerminal(term: term, controller: terminalController(for: spaceId(ofTerm: term)), model: self)
        terminals[term] = terminal
        return terminal
    }

    /// The controller for `space`, made on first use from the look held for it (the defaults until
    /// its settings arrive, then restyled live).
    private func terminalController(for space: String?) -> TerminalController {
        let key = space ?? ""
        if let held = terminalControllers[key] { return held }
        let look = terminalLooks[key] ?? .standard
        let made = TerminalController(configuration: TerminalStyle.configuration(look), theme: TerminalStyle.theme(look))
        terminalControllers[key] = made
        return made
    }

    private func spaceId(ofTerm term: String) -> String? {
        spaces.first { space in
            space.root.panes.contains { if case let .terminal(t, _) = $0.kind { return t == term } else { return false } }
        }?.id
    }

    /// A space's `terminal.*` settings changed: restyle its controller in place, so the shells keep
    /// running and every surface redraws.
    private func restyleTerminals(of space: String, from list: [SettingInfo]) {
        let look = TerminalAppearance(settings: list)
        guard terminalLooks[space] != look else { return }
        terminalLooks[space] = look
        Log.info("terminal appearance for \(space): \(look.fontFamily.isEmpty ? "own font" : look.fontFamily) \(look.fontSize)pt, \(look.theme.rawValue), cursor \(look.cursor.rawValue)\(look.cursorBlink ? " blinking" : "")")
        guard let controller = terminalControllers[space] else { return }
        let a = controller.setTerminalConfiguration(TerminalStyle.configuration(look))
        let b = controller.setTheme(TerminalStyle.theme(look))
        Log.info("terminal controller for \(space) restyled (config \(a), theme \(b))")
    }

    /// The page showing `pane` came up: attach its terminal, when it is one, and detach the one
    /// the previous page showed.
    func show(pane: String?) {
        shownPane = pane
        showTerm()
    }

    private func showTerm() {
        let term = activeSpace?.allPanes.first { $0.id == shownPane }.flatMap { entry -> String? in
            if case let .terminal(term, _) = entry.kind { term } else { nil }
        }
        guard term != shownTerm else { return }
        if let old = shownTerm {
            client?.send(.termDetach(id: old))
            terminals[old]?.attached = false
            Log.info("detached from \(old)")
        }
        shownTerm = term
        if let term { attach(term) }
    }

    #if DEBUG
    /// Checks type through the surface as the keyboard would: `text` as a paste, then Return.
    func typeIntoShown(_ text: String) {
        guard let shownTerm, let terminal = terminals[shownTerm] else { return }
        let pasted = terminal.view.paste(text: text), entered = terminal.view.sendKey(.enter)
        Log.info("typed into \(shownTerm): paste \(pasted), return \(entered)")
    }

    /// Checks raise the keyboard in the shown terminal as a tap on it does.
    func focusShown() {
        guard let shownTerm, let terminal = terminals[shownTerm] else { return }
        terminal.view.requestFocus()
        Log.info("keyboard requested in \(shownTerm)")
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
        let held = Set(spaces.flatMap { $0.allPanes.compactMap { pane -> String? in
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

    /// What an agent pane's header and card show beyond the chat: its folder and its login, by
    /// pane id, from the last `paneInfo` reply.
    private(set) var logins: [String: PaneHeaderInfo] = [:]
    /// Ask for a pane's folder and login; with `refresh`, ask the daemon to read the usage first
    /// (it reads at most once per its interval however often this asks).
    func requestHeader(_ pane: String, refresh: Bool = false) {
        guard let client else { return }
        if refresh { client.send(.usage(req: client.req(), dueOnly: true)) }
        client.send(.paneInfo(req: client.req(), pane: pane))
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

    /// What the links in a chat reply know and do: the layout is read from this model, requests go
    /// over its client, and a click that opens a pane asks the daemon for it.
    @ObservationIgnored lazy var links: ChatLinkResolver = {
        let resolver = ChatLinkResolver()
        resolver.locate = { [unowned self] agent in
            ChatLinks.place(ofAgent: agent, in: spaces).map { .init(space: $0.space, cwd: $0.cwd, pane: $0.pane) }
        }
        resolver.paneChip = { [unowned self] id in ChatLinks.chip(for: id, in: spaces) }
        resolver.request = { [unowned self] make in
            guard let client, isConnected else { return nil }
            let req = client.req()
            client.send(make(req))
            return req
        }
        resolver.perform = { [unowned self] action, agent in performChatLink(action, agent: agent) }
        return resolver
    }()

    private func performChatLink(_ action: ChatLinkResolver.Action, agent: String) {
        Log.info("chat link from \(agent): \(action)")
        func beside(url: String? = nil, artifact: String? = nil) {
            guard let place = links.locate(agent) else { return }
            menuOp(showing: .pane) { .paneOpen(req: $0, pane: place.pane, dir: .right, url: url, artifact: artifact, focus: false) }
        }
        switch action {
        case let .focusPane(id):
            if let space = spaces.first(where: { $0.allPanes.contains { $0.id == id } }) { focus(space: space.id, pane: id) }
        case let .openPage(url): beside(url: url)
        case let .openArtifact(name): beside(artifact: name)
        case let .openFile(path, line): beside(url: ChatLinks.fileURL(path: path, line: line))
        case let .openInSystemBrowser(url):
            if let url = URL(string: url) { UIApplication.shared.open(url) }
        case let .runCommand(command):
            guard let place = links.locate(agent) else { return }
            menuOp(showing: .pane) { .paneRun(req: $0, pane: place.pane, command: command, focus: false) }
        case let .copyText(text):
            UIPasteboard.general.string = text
        }
    }

    /// The appearance of the chat showing `agent`, from its space's resolved `chat.*` settings.
    func appearance(forAgent agent: String) -> ChatAppearance {
        appearances.appearance(forAgent: agent, in: spaces)
    }

    /// Ask for the settings of every space an appearance is not held for.
    private func askAppearances() {
        guard let client else { return }
        for space in appearances.missing(in: spaces) where !appearanceReqs.values.contains(space) {
            let req = client.req()
            appearanceReqs[req] = space
            client.send(.settingsLs(req: req, space: space))
        }
    }

    /// Ask for the settings of every space but `except`: a machine-wide write changed what each
    /// one falls back to, and a space's own write changed that space.
    private func refreshAppearances(except: String?) {
        guard let client else { return }
        for space in spaces.map(\.id) where space != except {
            let req = client.req()
            appearanceReqs[req] = space
            client.send(.settingsLs(req: req, space: space))
        }
    }

    private func storeAppearance(_ list: [SettingInfo], for space: String?) {
        if let space { restyleTerminals(of: space, from: list) }
        var held = appearances
        held.store(list, for: space)
        if held != appearances { appearances = held }
    }

    /// Ask for the settings once at connect, for `status.dots`; a later change arrives as a broadcast.
    private func askStatusDots() {
        guard let client else { return }
        let req = client.req()
        statusDotsReq = req
        client.send(.settingsLs(req: req, space: nil))
    }

    /// Every status held, `<space>=<label>` then `<pane>=<label>`, for the log.
    private var statusText: String {
        spaces.flatMap { space in
            (space.status.map { ["\(space.id)=\($0.label)"] } ?? [])
                + space.paneStatuses.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value.label)" }
        }.joined(separator: ", ")
    }

    func paneStatus(_ pane: String) -> PaneStatus? { spaces.status(ofPane: pane) }

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
        if let dots = list.first(where: { $0.key == "status.dots" }) {
            let ranks = StatusRank.listed(dots.value)
            if ranks != statusDots { Log.info("status.dots = \(dots.value)") }
            statusDots = ranks
        }
        if let req, appearanceReqs.removeValue(forKey: req) != nil {
            storeAppearance(list, for: space)
            return
        }
        if req == nil {
            storeAppearance(list, for: space)
            refreshAppearances(except: space)
        }
        if let req, req == statusDotsReq {
            statusDotsReq = nil
            return
        }
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

    private func menuOp(clearsError: Bool = true, showing: ShowWhenMade? = nil, startingAgent: String? = nil, _ make: (UInt64) -> ClientMsg) {
        guard let client else { return }
        if clearsError { menuError = nil }
        let req = client.req()
        menuReqs.insert(req)
        if let showing { showWhenMade[req] = showing }
        if let startingAgent { startWhenMade[req] = startingAgent }
        let msg = make(req)
        Log.info("pane op: \((try? msg.json()) ?? "?")")
        client.send(msg)
    }

    /// A new pane right of the space's shown pane, started as `agent` and shown once it is made.
    func newAgentPane(in space: String, agent: String) {
        guard let pane = spaces.first(where: { $0.id == space })?.focus else { return }
        menuOp(showing: .pane, startingAgent: agent) { .paneSplit(req: $0, pane: pane, dir: .right, focus: false) }
    }
    /// `artifact` in a new pane right of the space's shown pane, shown once it is made.
    func openArtifact(_ artifact: ArtifactInfo, in space: String) {
        guard let pane = spaces.first(where: { $0.id == space })?.focus else { return }
        menuOp(showing: .pane) { .paneOpen(req: $0, pane: pane, dir: .right, url: nil, artifact: artifact.name, focus: false) }
    }
    func split(_ pane: String, _ dir: Dir?) { menuOp(showing: .pane) { .paneSplit(req: $0, pane: pane, dir: dir, focus: false) } }
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
    /// The fields of each pane's info as the daemon last listed them, by pane id.
    private(set) var paneInfo: [String: [InfoField]] = [:]
    /// Ask the daemon what `pane` is; the answer lands in `paneInfo`, and the last one is dropped
    /// first so the sheet shows its spinner rather than values from an earlier open.
    func requestPaneInfo(_ pane: String) {
        guard let client else { return }
        paneInfo[pane] = nil
        client.send(.paneInfo(req: client.req(), pane: pane))
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

    /// The other way `kind` can show its conversation: a chat for a terminal running Claude Code
    /// or Codex, the agent's own terminal for an agent pane.
    func otherView(_ kind: PaneKind) -> ConversationView? {
        kind.otherView { name in agents.contains { $0.name == name && $0.found } }
    }
}


/// One key in one `wheel.toml`: this machine's, or the chosen project's.
struct SettingField: Hashable {
    enum Layer: String { case global, project }
    let layer: Layer
    let key: String
}

/// An agent pane's folder and the login it runs under, from `paneInfo`.
struct PaneHeaderInfo: Equatable {
    var folder: String?
    var login: PaneLogin?
}
