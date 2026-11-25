import Foundation
import GhosttyTerminal
import Observation
import Security
import SwiftUI
import UIKit
import WheelKit
import WheelUI

/// What the phone knows: the pairing, the daemon connection and the latest layout the daemon sent.
/// The daemon owns the layout (ADR-0001); this model renders it and changes it only by sending ops.
@MainActor @Observable
final class PhoneModel {
    /// The `task_output` requests in flight: which agent's task each reply belongs to.
    @ObservationIgnored private var taskOutputAsks: [UInt64: (agent: String, task: String)] = [:]

    /// Read a running background task's recent output for the full view.
    func readTaskOutput(_ chat: AgentChat, task: String) {
        guard let client else { return }
        let req = client.req()
        taskOutputAsks[req] = (chat.name, task)
        client.send(.taskOutput(req: req, agent: chat.name, task: task))
    }
    private(set) var pairing: Pairing?
    private(set) var connection: DaemonClient.Connection = .connecting
    /// Set when the Mac presented a certificate other than the one the pairing pins.
    private(set) var certificateMismatch = false
    /// Set when the Mac refused the saved credential (the device was revoked there); the phone
    /// asks to pair again instead of retrying.
    private(set) var credentialRefused = false
    private(set) var spaces: [SpaceInfo] = [] {
        didSet {
            links.layoutChanged(spaces)
            onLayout?()
        }
    }
    /// Called after every change to `spaces`: the server list resolves a notification tap waiting for its space.
    @ObservationIgnored var onLayout: (() -> Void)?
    /// The sessions wheeld holds, in order, each naming its spaces (ADR-0021).
    private(set) var sessions: [SessionInfo] = []
    /// The session and space this phone shows. The daemon keeps no view (ADR-0022): switching here
    /// draws from the layout already held, moves no other client, and the phone reports it with `view`.
    private(set) var view = SessionView()
    /// What the phone's body shows, a `WindowMode` id: the space's pages, or another mode in their
    /// place. The phone holds it alone and saves it with the view, per paired Mac; wheeld never hears of it (ADR-0022).
    private(set) var mode = WindowMode.spaces
    /// The page Orchestrator mode shows: a pane of the orchestrator's space, nil for the chat. Saved with the view.
    private(set) var orchestratorPage: String?

    /// Show another mode, as the mode menu's row does.
    func setMode(_ new: String) {
        guard WindowMode.builtIn.contains(new) else {
            Log.info("mode \(new): no such mode")
            return
        }
        guard new != mode else { return }
        Log.info("mode \(mode) -> \(new)")
        mode = new
        saveView()
        reportView()
        showTerm()
        readTopic()
    }

    /// The orchestrator's folder (ADR-0054), from the machine's `orchestrator.dir` setting; nil until wheeld lists it.
    private(set) var orchestratorDir: String?
    /// The space open on the orchestrator's folder, the first when several are: Orchestrator mode shows it in place of
    /// the shown space.
    var orchestratorSpace: SpaceInfo? {
        guard let dir = orchestratorDir.map(Self.folder) else { return nil }
        return spaces.first { Self.folder($0.cwd) == dir }
    }
    /// The orchestrator space's first agent pane, whose chat is Orchestrator mode's first page; nil in another mode or
    /// while that space holds no agent pane.
    var orchestratorPane: String? {
        guard mode == WindowMode.orchestrator else { return nil }
        return orchestratorSpace?.allPanes.first { if case .agent = $0.kind { true } else { false } }?.id
    }
    /// The orchestrator space's Queue plugin pane (`wheelhouse`'s `queue`), whose script reads the queue; nil in another
    /// mode or while that space holds none.
    var orchestratorQueuePane: String? {
        guard mode == WindowMode.orchestrator else { return nil }
        return orchestratorSpace?.allPanes.first { $0.kind.plugin.map(Self.isQueue) ?? false }?.id
    }
    /// The queue Orchestrator mode's tray shows, nil until the Queue pane's feed arrives.
    var orchestratorQueue: OrchestratorQueue? { orchestratorQueuePane.flatMap { queues[$0] } }
    /// Each Queue pane's queue, as its script's `wheelhouse.queue` subscription last held it.
    private(set) var queues: [String: OrchestratorQueue] = [:]
    private static func isQueue(_ ids: (plugin: String, pane: String)) -> Bool { ids.plugin == "wheelhouse" && ids.pane == "queue" }

    /// The agent in the orchestrator's chat pane; nil in another mode or with no such pane.
    private var orchestratorAgent: String? {
        guard let pane = orchestratorPane,
              case let .agent(agent, _, _)? = orchestratorSpace?.allPanes.first(where: { $0.id == pane })?.kind else { return nil }
        return agent
    }

    /// The orchestrator's current topic, read from its folder; nil with no `threads/current`, in another mode, or
    /// before the first read.
    private(set) var orchestratorTopic: OrchestratorTopic?
    private enum TopicRead { case current, notes(String) }
    private var topicReqs: [UInt64: TopicRead] = [:]

    /// Reads `threads/current`, then that topic's notes: on entering Orchestrator mode, when the folder is listed,
    /// and each time the orchestrator's turn ends, since `tools/note` and `tools/switch` run inside its turns.
    private func readTopic() {
        guard mode == WindowMode.orchestrator, let dir = orchestratorDir, case .connected = connection, let client else { return }
        let req = client.req()
        topicReqs[req] = .current
        client.send(.filePeek(req: req, path: "threads/current", cwd: dir, line: nil, context: 1))
    }

    private func topicRead(_ read: TopicRead, _ peek: FilePeek) {
        switch read {
        case .current:
            guard peek.found, let id = OrchestratorTopic.currentID(peek.lines), let dir = orchestratorDir,
                  case .connected = connection, let client else {
                if orchestratorTopic != nil { Log.info("topic: none current") }
                orchestratorTopic = nil
                return
            }
            let req = client.req()
            topicReqs[req] = .notes(id)
            client.send(.filePeek(req: req, path: "threads/\(id).md", cwd: dir, line: nil, context: 200))
        case let .notes(id):
            let topic = OrchestratorTopic(id: id, lines: peek.found ? peek.lines : [])
            if topic != orchestratorTopic { Log.info("topic \(id): \(topic.title), \(peek.found ? "\(peek.lines.count) lines" : "no notes file")") }
            orchestratorTopic = topic
        }
    }

    /// Tapping a queue item: its message goes to the orchestrator's chat, as if typed there.
    func ask(about item: OrchestratorQueue.Item) {
        guard let agent = orchestratorAgent else { return }
        guard case .connected = connection, let client else {
            chats[agent]?.error = "Not connected to the Mac."
            return
        }
        let req = client.req()
        agentReqs[req] = agent
        Log.info("queue: asked about \(item.id) (\(item.kind)) in \(agent)")
        client.send(.agentSend(req: req, agent: agent, text: OrchestratorQueue.message(for: item)))
    }
    /// A folder with no trailing slash, so two spellings of one folder compare equal.
    private static func folder(_ path: String) -> String {
        path.count > 1 && path.hasSuffix("/") ? String(path.dropLast()) : path
    }

    /// The name a page of `space` goes by: a mirror's title, the name the pane was given, else its kind's.
    func pageName(_ pane: String, in space: SpaceInfo) -> String {
        let kind = space.allPanes.first { $0.id == pane }?.kind
        return mirror(pane)?.title ?? space.paneNames[pane] ?? kind.flatMap { PhoneCommands.paneName($0, status: nil) } ?? kind?.name ?? pane
    }

    /// The panes pinned in each space, drawn as a sheet over the carousel in place of a page. Saved per paired Mac,
    /// since each Mac's ids are its own; the phone holds them alone and wheeld never hears of a pin (ADR-0022).
    private(set) var pins = Pins()
    /// Each space's pin sheet as last moved: the pinned pane it shows and where it sits. Held for this launch only.
    private var sheets: [String: PinSheet] = [:]

    /// The pin sheet of `space`: the pinned pane it shows (the first pinned until the person picks one) and its
    /// position (low until moved); nil while the space has no pins.
    func sheet(in space: String) -> PinSheet? {
        let list = pins.pinned(in: space)
        guard let first = list.first else { return nil }
        let held = sheets[space]
        let pane = held.map(\.pane).flatMap { list.contains($0) ? $0 : nil } ?? first
        return PinSheet(pane: pane, position: held?.position ?? .low)
    }

    /// Pin `pane` of the shown space: it leaves the carousel and the sheet shows it at low.
    func pin(_ pane: String) {
        guard let space = activeSpace?.id else { return }
        pins.pin(pane, in: space)
        sheets[space] = PinSheet(pane: pane, position: .low)
        savePins()
        Log.info("pin \(pane) in \(space): pinned \(pins.pinned(in: space)), sheet low")
        showTerm()
    }

    /// Unpin `pane` of the shown space: it goes back to the carousel, and the sheet shows the next pinned pane.
    func unpin(_ pane: String) {
        guard let space = activeSpace?.id else { return }
        pins.unpin(pane, in: space)
        if sheets[space]?.pane == pane { sheets[space]?.pane = pins.pinned(in: space).first ?? pane }
        savePins()
        Log.info("unpin \(pane) in \(space): pinned \(pins.pinned(in: space))")
        showTerm()
    }

    /// Show pinned pane `pane` in the shown space's sheet, where the sheet already sits.
    func showPinned(_ pane: String) {
        guard let space = activeSpace?.id, let held = sheet(in: space), pins.isPinned(pane, in: space), held.pane != pane else { return }
        sheets[space] = PinSheet(pane: pane, position: held.position)
        Log.info("sheet \(space): shows \(pane)")
        showTerm()
    }

    /// Move the shown space's sheet to `position`, as a drag ending there or the hidden sheet's button does.
    func moveSheet(to position: SheetPosition) {
        guard let space = activeSpace?.id, let held = sheet(in: space), held.position != position else { return }
        sheets[space] = PinSheet(pane: held.pane, position: position)
        Log.info("sheet \(space): \(held.position.rawValue) -> \(position.rawValue)")
        showTerm()
    }

    private static func pinsKey(_ pairing: Pairing) -> String { "pins \(pairing.url.absoluteString)" }

    private func savePins() {
        guard let pairing else { return }
        UserDefaults.standard.set(pins.bySpace, forKey: Self.pinsKey(pairing))
    }

    /// The pins saved at the last launch for this Mac.
    private func restorePins() {
        let saved = pairing.flatMap { UserDefaults.standard.dictionary(forKey: Self.pinsKey($0)) as? [String: [String]] } ?? [:]
        pins = Pins(saved)
        sheets = [:]
        Log.info("pins restored: \(pins.bySpace.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value.joined(separator: ","))" }.joined(separator: " "))")
    }

    /// The space this phone shows; nil while the shown session holds none.
    /// The space this phone shows: Orchestrator mode's is the orchestrator's space; nil while none is open.
    var active: String? { mode == WindowMode.orchestrator ? orchestratorSpace?.id : view.space }
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
    /// The phone's own browser of each browser pane in the layout.
    private(set) var browsers: [String: PhoneBrowser] = [:]
    /// The script, state and feeds of each plugin pane in the layout (ADR-0041), kept across swipes and layout changes.
    private(set) var pluginPanes: [String: PluginPaneLink] = [:]
    /// What `phone.log` last said about each plugin pane, so a line is written only when it changes.
    private var pluginLogged: [String: String] = [:]
    private let pageDaemon = PageDaemon()
    /// The values pushed into the web panes' pages, carried on this app's connection (`page_watch`).
    @ObservationIgnored private lazy var pageFeed = PageFeed(send: { [weak self] in self?.client?.send($0) })
    /// The artifacts plugin pages embed (`WebView{artifact}`, ADR-0047), made with the first one.
    @ObservationIgnored private var artifactEmbeds: ArtifactEmbeds?
    /// One conversation per agent pane in the layout, keyed by agent name.
    private(set) var chats: [String: AgentChat] = [:]
    /// Voice mode over one agent pane (`docs/voice-mode/index.md`); the lease and agent calls it makes go through `voiceTransport`.
    let voice = VoiceSession()
    @ObservationIgnored private var voiceClaimReq: UInt64?
    @ObservationIgnored private var voiceNameReqs: Set<UInt64> = []
    /// The pane voice mode was opened in, where the pill's orb returns to.
    @ObservationIgnored private var voicePane: String?
    /// The files each agent pane's composer holds for its next message, keyed by pane id.
    private(set) var trays: [String: AttachmentTray] = [:]
    /// Each agent pane's earlier conversations, drawn dimmed above its transcript, keyed by pane
    /// id, with the agent they belong to: a pane that shows another agent starts a new store.
    private var earlier: [String: (agent: String, store: EarlierConversations)] = [:]
    /// The pane and conversation each `agent_conversation_history` sent reads.
    private var historyReqs: [UInt64: (pane: String, id: String)] = [:]
    /// What the connected server offers, from its last status; `.none` until one arrives, so
    /// nothing whose op the server may lack shows before the server has said (ADR-0050).
    private(set) var serverOffer: ServerOffer = .none
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
    /// One report of a browser pane's page console: `reset` when the page just loaded, `req` once sent.
    private struct ConsoleReport {
        var req: UInt64?
        var reset: Bool
        var entries: [ConsoleEntry]
    }
    /// What each browser pane's page wrote to its console that wheeld has not answered, in write
    /// order. A report stays until wheeld's `ok`; the first status of each connection sends every
    /// one again, so a line is delivered at least once (a report wheeld applied whose `ok` was lost
    /// arrives twice). A page load clears what was written before it.
    private var consoleOutbox: [String: [ConsoleReport]] = [:]
    /// The pane of each sent console report on this connection, so a refusal names it.
    private var consoleReqs: [UInt64: String] = [:]
    private static let consoleOutboxLimit = 500
    private var agentReqs: [UInt64: String] = [:]
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
    /// The terminal the visible page shows. With the terms `embeds` the shown plugin page draws, the only ones attached,
    /// so the phone's grid size is the size the shell has while the page is up.
    private var shownTerm: String?
    /// The pane the shown space's pin sheet draws while the sheet is up: shown as the visible page is, so its terminal, or a
    /// plugin pane's embedded terminals, attach and size their shells.
    private var sheetPane: String?
    /// `sheetPane`'s terminal, when it is one.
    private var sheetTerm: String?
    /// The surfaces plugin pages draw for `Terminal{pane}` (ADR-0042) and mirror pages draw for their source terminal, one
    /// per embed view while it is on screen, in the order they came up; each is its own Ghostty surface, never the terminal
    /// page's, since one surface draws in one view.
    /// The carousel's paged TabView holds extra copies of a page for a moment while the space's tree changes, so one
    /// embed can be here more than once; the newest of a term sizes the shell.
    private var embeds: [PhoneTerminal] = []
    /// For each term being attached, the replay bytes still to come and the surfaces they go to: only the surfaces
    /// that were not yet drawing, so a surface already showing the screen is not reset when another embed comes up.
    private var replays: [String: (bytes: Int, to: [PhoneTerminal])] = [:]
    /// The terms this connection streams: `shownTerm` and the shown plugin page's embedded terms.
    private var attachedTerms: Set<String> = []
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

    /// Whether the composer offers attachments: an upload is an HTTP request every connected
    /// server takes, and the message that names the files is `agent_send`.
    var canAttach: Bool { isConnected && serverOffer.has("agent_send") }


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

    /// Tell the daemon the push token, once connected to a server that takes it.
    private func registerPush() {
        guard let token = apnsToken, let client, serverOffer.has("push_register") else { return }
        client.send(.pushRegister(req: client.req(), token: token))
    }

    /// Connect with a pairing the Keychain kept.
    func resume(_ saved: Pairing) {
        #if DEBUG
        watchToolbarPresses()
        #endif
        guard client == nil else { return }
        pairing = saved
        // A server that is down at launch still shows the name and mark it last reported.
        if let last = UserDefaults.standard.dictionary(forKey: Self.hostKey(saved)) as? [String: String] {
            host = HostAccess(exe: "", id: last["id"], name: last["name"], color: last["color"], platform: last["platform"], folders: [])
        }
        open(saved)
    }

    /// Called once a pairing code this model tried is kept: the server list takes the model in.
    @ObservationIgnored var onPaired: ((PhoneModel) -> Void)?

    /// Who this wheeld says it is (`status.host`): its id, name, colour and platform (ADR-0057).
    private(set) var host: HostAccess?

    /// The server's name for people: its `server.name`, else the host its pairing reaches.
    var serverName: String {
        if let name = host?.name, !name.isEmpty { return name }
        return (pairing ?? trying)?.url.host() ?? "Server"
    }

    /// Whether the phone shows this server now. A server not shown streams no terminal and holds
    /// no view on its wheeld (`client_ls` lists the phone only on the server it shows).
    private(set) var onScreen = true

    func setOnScreen(_ shown: Bool) {
        guard shown != onScreen else { return }
        onScreen = shown
        Log.info("server \(serverName) \(shown ? "shown" : "hidden")")
        reportedView = nil
        if shown {
            reportView()
        } else {
            client?.send(.view(req: nil, window: "phone", space: nil, pane: nil, session: nil))
            voice.end()
        }
        syncAttachments()
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
        if let pairing {
            PairingStore.remove(pairing)
            UserDefaults.standard.removeObject(forKey: Self.hostKey(pairing))
        }
        pairing = nil
        host = nil
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
        embeds = []
        replays = [:]
        attachedTerms = []
        terminalControllers = [:]
        terminalLooks = [:]
        shownTerm = nil
        sheetTerm = nil
        sheetPane = nil
        shownPane = nil
        pins = Pins()
        sheets = [:]
        pageDaemon.url = nil
        pageDaemon.token = nil
        pageDaemon.pairing = nil
        syncWebPanes()
        syncBrowsers()
        syncPluginPanes()
        chats = [:]
        perms = []
        agents = []
        artifacts = nil
        menuReqs = []
        earlier = [:]
        historyReqs = [:]
        consoleOutbox = [:]
        consoleReqs = [:]
        agentReqs = [:]
        logins = [:]
        settings = []
        settingsLoaded = false
        settingRefusals = [:]
        Log.info("unpaired")
    }

    /// Drop the connection and keep the pairing: another model holds this server now.
    func disconnect() { close() }

    private func close() {
        generation += 1
        client?.stop()
        client = nil
    }

    private func open(_ p: Pairing) {
        generation += 1
        let mine = generation
        consoleReqs = [:]
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
            let wasConnected = connection == .connected
            connection = state
            if state != .connected {
                for terminal in terminals.values { terminal.attached = false }
                // A read in flight is lost with the connection; the next layout makes the stores
                // again, and each divider asks once more.
                if !historyReqs.isEmpty {
                    historyReqs = [:]
                    earlier = [:]
                }
            }
            if wasConnected, state != .connected { for link in pluginPanes.values { link.dropped() } }
            if state != .connected {
                links.forget()
                consoleReqs = [:]
                voiceClaimReq = nil
                voice.end("Voice mode ended: the connection to the Mac dropped.")
            }
            if case .connected = state {
                settingListReq = nil
                settingChangeReqs = [:]
            }
            if case .connected = state, let trying {
                self.trying = nil
                let status = PairingStore.keep(trying)
                if status == errSecSuccess {
                    pairing = trying
                    Log.info("paired with \(trying.url.absoluteString) (server \(trying.server ?? "unnamed"))")
                    onPaired?(self)
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

    /// Keep who the wheeld says it is, and record its id with a pairing kept before it said one.
    private func named(_ host: HostAccess) {
        if self.host != host {
            self.host = host
            if let pairing {
                let last = ["id": host.id, "name": host.name, "color": host.color, "platform": host.platform].compactMapValues { $0 }
                UserDefaults.standard.set(last, forKey: Self.hostKey(pairing))
            }
            Log.info("server \(host.name ?? "unnamed") id \(host.id ?? "none") colour \(host.color ?? "none") platform \(host.platform ?? "none")")
        }
        guard let id = host.id, !id.isEmpty, var held = pairing, held.server != id else { return }
        held.server = id
        if PairingStore.keep(held) == errSecSuccess {
            pairing = held
            Log.info("pairing \(held.url.absoluteString) is server \(id)")
        }
    }

    private func pinFailed() {
        Log.info("certificate does not match the pinned fingerprint")
        certificateMismatch = true
        close()
        trying = nil
    }

    private func handle(_ msg: ServerMsg) {
        if links.received(msg) { return }
        if pluginPanes.values.contains(where: { $0.handle(msg) }) { return }
        if artifactEmbeds?.handle(msg) == true { return }
        if pageFeed.handle(msg) { return }
        switch msg {
        case let .status(_, _, _, _, _, perms, spaces, agents, _, _, _, host, offer, sessions):
            if let host { named(host) }
            if serverOffer != offer {
                Log.info("server protocol \(offer.protocolVersion), level \(offer.compat), this app's \(ServerOffer.clientCompat); attachments \(offer.has("agent_send") ? "offered" : "hidden")")
            }
            serverOffer = offer
            offerBrowsers()
            self.agents = agents
            // An unsolicited status means this connection missed daemon-wide events.
            self.perms = perms
            let known = Array(chats.values)
            let heldPlugins = Array(pluginPanes.values)
            reportedView = nil
            layoutArrived(spaces, sessions: sessions)
            // This connection's reqs start over: everything unanswered goes again, in order.
            consoleReqs = [:]
            flushConsole(resend: true)
            // A new connection holds no feeds: each plugin pane kept from the last one asks for its script and feeds again.
            for link in heldPlugins where pluginPanes[link.pane] === link { link.start() }
            artifactEmbeds?.connected()
            // A wheeld without `page_watch` serves each page's event stream over a connection of its own.
            if offer.has("page_watch") {
                pageDaemon.feed = pageFeed
                pageFeed.connected()
            } else {
                pageDaemon.feed = nil
                pageFeed.end()
            }
            // A fresh connection starts with its first status: attach the page that is showing, and its embeds, again.
            attachedTerms = []
            syncAttachments()
            // The attaches below replace each held transcript with the daemon's current one.
            for chat in known where chats[chat.name] === chat { attach(chat) }
        case let .paired(device, credential, server):
            guard let code = trying else { break }
            let credentialed = Pairing(url: code.url, token: credential, certSha256: code.certSha256, server: server)
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
            if agent == orchestratorAgent, state == .ready { readTopic() }
        case let .agentAttached(req, agent, state, events, config, context, now):
            if let req { agentReqs[req] = nil }
            guard let chat = chats[agent] else { return }
            chat.state = state
            chat.reset(events, daemonNow: now)
            chat.setConfig(config)
            chat.context = context
            chat.error = nil
            chat.notice = nil
            chat.title = nil
            Log.info("agent \(agent) attached, \(events.count) events")
            if agent == orchestratorAgent { readTopic() }
        case let .agentConfig(agent, options):
            chats[agent]?.setConfig(options)
        case let .agentContext(agent, context):
            chats[agent]?.context = context
        case let .taskOutput(req, text, truncated):
            // A reply for a task that ended meanwhile is not kept.
            if let ask = taskOutputAsks.removeValue(forKey: req), let chat = chats[ask.agent], chat.footer?.tasks.contains(where: { $0.id == ask.task }) == true {
                chat.taskOutputs[ask.task] = (text, truncated)
            }
        case let .agentFooter(agent, footer):
            chats[agent]?.footer = footer
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
        case .toastClear, .stopping:
            break
        case let .agentNotice(agent, notice):
            chats[agent]?.notice = notice
        case let .agent(agent, event):
            chats[agent]?.apply(event)
            switch event.event {
            case let .turnEnd(_, reason) where reason == "end_turn":
                if let chat = chats[agent] { voice.turnEnded(agent: agent, reply: chat.transcript.lastReply) }
            case let .card(title):
                voice.cardShown(agent: agent, title: title)
            default:
                break
            }
        case let .leaseClaimed(req, key, owner):
            if req == voiceClaimReq, key == VoiceSession.leaseKey {
                voiceClaimReq = nil
                voice.claimed(owner: owner)
            }
        case let .lease(key, owner):
            voice.lease(key: key, owner: owner)
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
            browsers[pane]?.reload()
        case let .inspectPane(id, pane, op, arg):
            guard let browser = browsers[pane] else {
                client?.send(.renderRefused(id: id, message: RenderRefusal.notShown))
                return
            }
            Task { [weak self] in
                let reply: ClientMsg = switch await browser.run(op, arg: arg) {
                case let .text(text): .inspected(id: id, text: text)
                case let .png(png): .rendered(id: id, png: png)
                case let .film(film): .recorded(id: id, mov: film.mov, frames: film.frames, millis: film.millis)
                case let .refused(message): .renderRefused(id: id, message: message)
                }
                self?.client?.send(reply)
            }
        case let .renderPane(id, pane, full):
            guard let browser = browsers[pane] else {
                client?.send(.renderRefused(id: id, message: RenderRefusal.notShown))
                return
            }
            Task { [weak self] in
                let reply: ClientMsg = switch await browser.shot(full: full) {
                case let .success(png): .rendered(id: id, png: png)
                case let .failure(refusal): .renderRefused(id: id, message: refusal.message)
                }
                self?.client?.send(reply)
            }
        case let .termAttached(_, id, replayBytes):
            guard attachedTerms.contains(id) else { return }
            // Every attach replays the screen from scratch. It goes to the surfaces not yet drawing, reset first; when
            // every surface is drawing (the daemon's resync after the phone fell behind) it goes to all of them.
            let all = surfaces(of: id)
            let fresh = all.filter { !$0.attached }
            let to = fresh.isEmpty ? all : fresh
            replays[id] = (replayBytes, to)
            for terminal in to {
                terminal.session.receive(Data("\u{1b}c".utf8))
                terminal.attached = true
                terminal.replayBeforeLayout = !fitsPage(terminal)
                settle(terminal)
            }
            Log.info("attached to \(id), replay \(replayBytes) bytes to \(to.count) of \(all.count) surfaces")
        case let .termOutput(id, data):
            var live = data
            if let replay = replays[id] {
                // The first `bytes` of output after an attach are its replay.
                let n = min(replay.bytes, data.count)
                for terminal in replay.to { terminal.session.receive(data.prefix(n)) }
                live = data.dropFirst(n)
                replays[id] = replay.bytes - n > 0 ? (replay.bytes - n, replay.to) : nil
            }
            if !live.isEmpty { for terminal in surfaces(of: id) { terminal.session.receive(live) } }
        case let .termExited(id, code):
            let shown = surfaces(of: id)
            guard !shown.isEmpty else { return }
            Log.info("terminal \(id) exited with \(code.map(String.init) ?? "?")")
            for terminal in shown {
                terminal.attached = false
                terminal.session.receive(Data("\r\n[shell exited — close this pane and split for a new one]\r\n".utf8))
            }
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
            voiceNameReqs.remove(req)
            consoleAnswered(req)
            menuReqs.remove(req)
            spaceReqs.remove(req)
        case let .conversationHistory(req, agent, id, found, events):
            guard let asked = historyReqs.removeValue(forKey: req), let store = earlier[asked.pane]?.store else { break }
            if found { store.loaded(id, agent: agent, events: events) } else { store.gone(id) }
            Log.info("pane \(asked.pane) earlier \(id): \(found ? "\(events.count) events" : "not in the store")")
        case let .conversation(req, agent, id):
            if menuReqs.remove(req) != nil { Log.info("agent \(agent) resumed \(id)") }
        case let .paneView(req, pane, view, conversation):
            menuReqs.remove(req)
            Log.info("pane \(pane) view \(view?.rawValue ?? "-") conversation \(conversation ?? "-")")
        case let .started(req, pane, agent):
            menuReqs.remove(req)
            Log.info("started \(pane) agent \(agent ?? "-")")
        case let .opened(req, pane, url):
            if menuReqs.remove(req) != nil {
                Log.info("pane \(pane) opened \(url)")
                // The phone's own browser goes there; the Mac's stays where it is.
                browsers[pane]?.open(url)
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
            if let req, req == voiceClaimReq {
                voiceClaimReq = nil
                voice.refused(message)
            }
            if let req, voiceNameReqs.remove(req) != nil { voice.end(message) }
            if let req { taskOutputAsks[req] = nil }
            if let req, let pane = consoleAnswered(req) {
                Log.info("browser pane \(pane) console report refused: \(message)")
            }
            if let req, let field = settingChangeReqs.removeValue(forKey: req) {
                settingRefusals[field] = message
                Log.info("setting \(field.key) refused: \(message)")
            }
            if let req { startWhenMade[req] = nil }
            if let req, let asked = historyReqs.removeValue(forKey: req) {
                // A refusal (a harness wheeld cannot read without loading) leaves Resume usable.
                Log.info("pane \(asked.pane) earlier \(asked.id) refused: \(message)")
            }
            if let req, menuReqs.remove(req) != nil {
                menuError = message
                Log.info("pane op refused: \(message)")
            }
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
        case let .filePeek(req, peek):
            if let read = topicReqs.removeValue(forKey: req) { topicRead(read, peek) }
        case let .dirList(req, path, dirs, error):
            guard let parent = dirReqs.removeValue(forKey: req) else { break }
            folderListings[CommandPalette.listingKey(parent)] = .init(dirs: dirs, error: error)
            Log.info("new space: \(parent) -> \(path): \(dirs.count) folders\(error.map { ", error \($0)" } ?? "")")
        default:
            break
        }
    }

    /// Offer page commands to wheeld; sent with every status, since a new connection starts with
    /// no offer.
    private func offerBrowsers() {
        client?.send(.renderOffer)
    }

    /// Keeps one `PhoneBrowser` per browser pane in the layout. Each opens at the pane's home when
    /// it is made and never moves because the layout names another home; it keeps the latest for
    /// `goHome`.
    private func syncBrowsers() {
        let wanted = BrowserPanes.urls(spaces)
        for pane in browsers.keys where wanted[pane] == nil {
            if let browser = browsers[pane] { PageSchemeHandler.close(browser.view) }
            browsers[pane] = nil
            consoleOutbox[pane] = nil
            Log.info("browser pane \(pane) left the layout")
        }
        for (pane, url) in wanted {
            if let browser = browsers[pane] {
                browser.homeIs(url)
            } else {
                browsers[pane] = PhoneBrowser(pane: pane, home: url, daemon: pageDaemon, console: { [weak self] reset, entries in
                    self?.relayConsole(pane, reset: reset, entries: entries)
                })
            }
        }
    }

    /// Queues what pane `pane`'s page wrote behind what is unanswered, then sends it if the
    /// connection is up. A reset drops what was queued before it; past `consoleOutboxLimit` lines the
    /// oldest go, and a dropped reset passes to the report that is now first.
    private func relayConsole(_ pane: String, reset: Bool, entries: [ConsoleEntry]) {
        var queue = consoleOutbox[pane] ?? []
        if reset { queue = [] }
        queue.append(ConsoleReport(req: nil, reset: reset, entries: entries))
        var count = queue.reduce(0) { $0 + $1.entries.count }
        var dropped = 0
        while count > Self.consoleOutboxLimit {
            let over = count - Self.consoleOutboxLimit
            if queue[0].entries.count <= over, queue.count > 1 {
                let head = queue.removeFirst()
                count -= head.entries.count
                dropped += head.entries.count
                if head.reset { queue[0].reset = true }
            } else {
                queue[0].entries.removeFirst(over)
                count -= over
                dropped += over
            }
        }
        if dropped > 0 { Log.info("browser pane \(pane) console: dropped the \(dropped) oldest unanswered lines (limit \(Self.consoleOutboxLimit))") }
        consoleOutbox[pane] = queue
        flushConsole(resend: false)
    }

    /// Sends the console reports not yet sent (every one, when `resend`), in write order, when the
    /// connection is up. Reports for a pane the layout no longer shows as a browser pane go nowhere.
    private func flushConsole(resend: Bool) {
        guard let client, client.connection == .connected else { return }
        for pane in Array(consoleOutbox.keys) {
            guard browsers[pane] != nil else {
                consoleOutbox[pane] = nil
                continue
            }
            for i in consoleOutbox[pane]!.indices where resend || consoleOutbox[pane]![i].req == nil {
                let req = client.req()
                consoleOutbox[pane]![i].req = req
                consoleReqs[req] = pane
                let report = consoleOutbox[pane]![i]
                client.send(.paneConsole(req: req, pane: pane, reset: report.reset, entries: report.entries))
            }
        }
    }

    /// wheeld answered console report `req`: it leaves the outbox. Returns its pane.
    @discardableResult
    private func consoleAnswered(_ req: UInt64) -> String? {
        guard let pane = consoleReqs.removeValue(forKey: req) else { return nil }
        consoleOutbox[pane]?.removeAll { $0.req == req }
        if consoleOutbox[pane]?.isEmpty == true { consoleOutbox[pane] = nil }
        return pane
    }

    /// A press of `button` left of browser pane `pane`'s URL bar; a disabled one does nothing.
    func press(_ button: BrowserButton, in pane: String) {
        guard let browser = browsers[pane], browser.enabled(button) else { return }
        switch button {
        case .back: browser.goBack()
        case .forward: browser.goForward()
        case .home: browser.goHome()
        case .reload: browser.reload()
        }
    }

    /// Return in browser pane `pane`'s URL bar: what it holds goes to wheeld (`browse`).
    func submitURL(in pane: String) {
        guard let browser = browsers[pane] else { return }
        Log.info("browser pane \(pane) URL bar submitted: \(browser.urlText)")
        browse(pane, browser.urlText)
    }

    /// Keeps one `WebPane` per web pane in the layout, loading each new one.
    private func syncWebPanes() {
        let wanted = WebPanes.panes(spaces)
        for (pane, web) in webPanes where wanted[pane]?.artifact != web.artifact {
            PageSchemeHandler.close(web.view)
            webPanes[pane] = nil
            Log.info("web pane \(pane) left the layout")
        }
        for (pane, shown) in wanted where webPanes[pane] == nil {
            guard let url = WebPanes.pageURL(pane: pane) else { continue }
            let web = WebPane(pane: pane, artifact: shown.artifact, store: shown.store, daemon: pageDaemon)
            webPanes[pane] = web
            web.load(url)
        }
    }

    /// Keeps one `PluginPaneLink` per plugin pane in the layout. A pane that leaves, or comes to run another plugin pane,
    /// closes its link; a new one asks wheeld for its script once a connection is up.
    private func syncPluginPanes() {
        var wanted: [String: (plugin: String, pane: String, space: SpaceInfo)] = [:]
        for space in spaces {
            for pane in space.allPanes {
                if let ids = pane.kind.plugin { wanted[pane.id] = (ids.plugin, ids.pane, space) }
            }
        }
        for (pane, link) in pluginPanes {
            guard let want = wanted[pane], link.plugin.context["plugin"] == .string(want.plugin),
                  link.plugin.context["plugin_pane"] == .string(want.pane) else {
                link.close()
                pluginPanes[pane] = nil
                pluginLogged[pane] = nil
                queues[pane] = nil
                Log.info("plugin pane \(pane) left the layout")
                continue
            }
        }
        guard let client else { return }
        for (pane, want) in wanted where pluginPanes[pane] == nil {
            let context: [String: JSONValue] = [
                "pane": .string(pane), "space": .string(want.space.id),
                "cwd": want.space.cwd.isEmpty ? .null : .string(want.space.cwd),
                "plugin": .string(want.plugin), "plugin_pane": .string(want.pane),
            ]
            let link = PluginPaneLink(pane: pane, context: context, send: { client.send($0) }, nextReq: { client.req() })
            link.plugin.onUpdate = { [weak self, weak link] in
                guard let self, let link else { return }
                self.logPlugin(link)
                if Self.isQueue((want.plugin, want.pane)) { self.readQueue(link) }
            }
            pluginPanes[pane] = link
            Log.info("plugin pane \(pane): \(want.plugin)/\(want.pane) in \(want.space.id)")
            if connection == .connected { link.start() }
        }
    }

    /// Keeps the queue a Queue pane's script holds, logging each change of its badge.
    private func readQueue(_ link: PluginPaneLink) {
        let queue = OrchestratorQueue(link.plugin.value(of: "wheelhouse.queue"))
        guard queues[link.pane] != queue else { return }
        if queues[link.pane]?.count != queue?.count || queues[link.pane]?.tone != queue?.tone {
            Log.info("queue \(link.pane): \(queue.map { "\($0.count) \($0.tone)" } ?? "none")")
        }
        queues[link.pane] = queue
    }

    /// One `phone.log` line per change of what a plugin pane shows: which script, how many reloads, the error, the state.
    private func logPlugin(_ link: PluginPaneLink) {
        let plugin = link.plugin
        let state = plugin.stateJSON
        let line = "plugin pane \(link.pane): \(plugin.chunk), reloads \(plugin.reloads), error \(plugin.error ?? "none"), state \(state.count > 300 ? String(state.prefix(300)) + "…" : state)"
        guard pluginLogged[link.pane] != line else { return }
        pluginLogged[link.pane] = line
        Log.info(line)
    }

    private func layoutArrived(_ spaces: [SpaceInfo], sessions listed: [SessionInfo]) {
        let sessions = listed
        // The layout broadcast leaves statuses out; `status_changed` carries them. The daemon keeps no
        // focused pane, so each space keeps the one this phone had, while that pane is there.
        let held = Dictionary(uniqueKeysWithValues: self.spaces.map { ($0.id, $0) })
        // A mirror draws as its source, so its terminal, chat or page attaches to the source's.
        self.spaces = spaces.resolvingMirrors().map { space in
            var space = space.keepingStatuses(of: held[space.id])
            if let was = held[space.id] { space.focus = was.focusKept(in: space) }
            return space
        }
        let old = self.sessions
        self.sessions = sessions
        // A layout with no sessions (a server still loading) restores nothing and saves nothing.
        if restorePending, !listed.isEmpty {
            restorePending = false
            restoreView()
            restorePins()
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
        // A pinned pane that closed leaves its space's pins.
        let kept = pins.kept(in: self.spaces)
        if kept != pins {
            Log.info("pins: \(pins.bySpace) -> \(kept.bySpace), a pinned pane closed")
            pins = kept
            savePins()
        }
        reportView()
        Log.info("layout: \(spaces.count) spaces, active \(active ?? "none"), panes \(activeSpace?.root.readingOrder.map(\.id) ?? []), statuses \(statusText)")
        for space in self.spaces {
            for (pane, shown) in space.mirrors.sorted(by: { $0.key < $1.key }) {
                Log.info("mirror \(pane) in \(space.id) shows \(shown.source) as \(space.allPanes.first { $0.id == pane }?.kind.name ?? "-"): \(shown.title)")
            }
        }
        Log.info("sessions: \(sessions.map { "\($0.id)=\($0.name)[\($0.spaces.joined(separator: ","))]" }.joined(separator: " ")); shown \(view.session ?? "none")")
        appearances.keep(only: self.spaces)
        askAppearances()
        syncWebPanes()
        syncBrowsers()
        syncPluginPanes()
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
        syncEarlier()
        guard let client else { return }
        let agents = Set(AgentPanes.agents(spaces).values)
        for name in chats.keys where !agents.contains(name) {
            client.send(.agentDetach(agent: name))
            chats[name] = nil
            if voice.agent == name { voice.end("Voice mode ended: its pane closed.") }
            Log.info("agent \(name) left the layout; detached")
        }
        for name in agents where chats[name] == nil {
            let chat = AgentChat(name: name)
            chat.sendUnsend = { [weak self, weak chat] turn in
                if let chat { self?.unsend(chat, turn: turn) }
            }
            chat.sendNow = { [weak self, weak chat] turn in
                if let chat { self?.sendNow(chat, turn: turn) }
            }
            chat.sendClearQueue = { [weak self, weak chat] in
                if let chat { self?.clearQueue(chat) }
            }
            chat.sendFork = { [weak self, weak chat] turns in
                if let chat { self?.fork(chat, turns: turns) }
            }
            chat.canFork = { [weak self, weak chat] in
                guard let self, let chat else { return false }
                return self.serverOffer.has("agent_fork")
                    && AgentPanes.canFork(harness: AgentPanes.harness(ofAgent: chat.name, in: self.spaces))
            }
            chats[name] = chat
            attach(chat)
        }
    }

    /// The earlier conversations agent pane `pane` draws above its transcript.
    func earlier(for pane: String) -> EarlierConversations {
        earlier[pane]?.store ?? .none
    }

    /// Give each agent pane the conversations it showed before its current one (the last of
    /// `visited`); a server that cannot read a past conversation gets none drawn.
    private func syncEarlier() {
        let reads = serverOffer.has("agent_conversation_history")
        var shown: [String: (agent: String, ids: [String])] = [:]
        for space in spaces {
            for pane in space.allPanes {
                if case let .agent(agent, _, visited) = pane.kind {
                    shown[pane.id] = (agent, reads ? Array(visited.dropLast()) : [])
                }
            }
        }
        for pane in earlier.keys where shown[pane]?.agent != earlier[pane]?.agent {
            earlier[pane] = nil
        }
        for (pane, want) in shown {
            let store: EarlierConversations
            if let held = earlier[pane]?.store {
                store = held
            } else {
                store = EarlierConversations()
                store.load = { [weak self] id in self?.readEarlier(pane: pane, agent: want.agent, id: id) }
                store.resume = { [weak self] id in self?.resumeEarlier(pane: pane, agent: want.agent, id: id) }
                earlier[pane] = (want.agent, store)
            }
            let was = store.list.map(\.id)
            store.show(want.ids)
            if was != want.ids { Log.info("pane \(pane) earlier conversations: \(want.ids.joined(separator: ","))") }
        }
    }

    private func readEarlier(pane: String, agent: String, id: String) {
        guard let client else { return }
        let req = client.req()
        historyReqs[req] = (pane, id)
        Log.info("pane \(pane) reading earlier \(id)")
        client.send(.agentConversationHistory(req: req, agent: agent, id: id))
    }

    private func resumeEarlier(pane: String, agent: String, id: String) {
        guard let client else { return }
        let req = client.req()
        menuReqs.insert(req)
        Log.info("pane \(pane) resume \(id)")
        client.send(.agentConversationResume(req: req, agent: agent, id: id))
    }

    /// Stream an agent's conversation; `agent_attached` resets its transcript.
    private func attach(_ chat: AgentChat) {
        guard let client else { return }
        let req = client.req()
        agentReqs[req] = chat.name
        Log.info("attaching to agent \(chat.name)")
        client.send(.agentAttach(req: req, agent: chat.name))
    }

    // MARK: Agents

    /// Change one of an agent's config options, as the composer's Model menu does. The menu redraws when wheeld
    /// broadcasts the agent's new list, never from the tap, so its check is always on what the agent took.
    func setConfig(_ chat: AgentChat, _ option: ConfigOption, to value: String) {
        guard case .connected = connection, let client else {
            chat.error = "Not connected to the Mac."
            return
        }
        chat.error = nil
        let req = client.req()
        agentReqs[req] = chat.name
        Log.info("set \(chat.name) \(option.id) to \(value)")
        client.send(.agentConfigSet(req: req, agent: chat.name, config: option.id, value: value))
    }

    /// Start the agent's conversation over, as the composer's Clear chat does: `/clear`, which wheeld turns into a
    /// conversation reset.
    func clearChat(_ chat: AgentChat) {
        guard case .connected = connection, let client else {
            chat.error = "Not connected to the Mac."
            return
        }
        chat.error = nil
        let req = client.req()
        agentReqs[req] = chat.name
        Log.info("clear chat \(chat.name)")
        client.send(.agentSend(req: req, agent: chat.name, text: "/clear"))
    }

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

    /// Whether the composer offers voice mode: wheeld takes the `voice` lease and `agent_voice`.
    var voiceOffered: Bool { isConnected && VoiceSession.offered(by: serverOffer.ops) }

    /// The composer's round button with an empty field: voice mode over `pane`'s agent. The microphone and the
    /// speaker are voice mode's alone, so this conversation's read-aloud steps aside.
    func openVoice(_ chat: AgentChat, pane: String, harness: String, orchestrator: Bool) {
        if voice.transport == nil { voice.transport = voiceTransport }
        chat.readsAloud = false
        voicePane = pane
        let space = spaces.first { $0.allPanes.contains { $0.id == pane } }?.name
        voice.open(agent: chat.name,
                   title: orchestrator ? "Orchestrator" : AgentInfo.title(of: chat.name),
                   subtitle: orchestrator ? harness : [space, harness].compactMap { $0 }.joined(separator: " · "),
                   heard: Voice.shared.said?.id ?? 0)
        Log.info("voice mode: opening for \(chat.name)")
    }

    #if DEBUG
    /// `WHEEL_VOICE_SHOT` and `WHEEL_DICTATE_SHOT`: a state of voice mode or of dictation drawn with no audio.
    func demoVoice(_ chat: AgentChat, pane: String, shot: String) {
        voicePane = pane
        if shot == "dictate" {
            Dictation.shared.demoRecording(for: chat.name)
            return
        }
        let space = spaces.first { $0.allPanes.contains { $0.id == pane } }?.name
        voice.transport = voiceTransport
        voice.demoOn(agent: chat.name, title: AgentInfo.title(of: chat.name), subtitle: [space, "claude"].compactMap { $0 }.joined(separator: " · "),
                     card: shot == "card" ? "Diff of src/lib.rs" : nil, pill: shot == "pill")
        let reply = "The tests pass. I changed the parser to read the lease key first, and nothing else needed to move."
        switch shot {
        case "asking": Voice.shared.start(for: chat.name)
        case "hearing": Voice.shared.demo(phase: .hearing, owner: chat.name, level: 0.7)
        case "speaking": Voice.shared.demo(phase: .speaking, owner: chat.name, level: 0.6, caption: reply)
        case "pill": Voice.shared.demo(phase: .speaking, owner: chat.name, level: 0.5, caption: reply)
        case "thinking": Voice.shared.demo(phase: .thinking, owner: chat.name)
        case "muted": Voice.shared.demo(phase: .listening, owner: chat.name, muted: true)
        default: Voice.shared.demo(phase: .listening, owner: chat.name, level: 0.4)
        }
    }
    #endif

    /// The pill's orb: the layer comes back on the agent's pane.
    func restoreVoice() {
        voice.restore()
        if let pane = voicePane, let space = spaces.first(where: { $0.allPanes.contains { $0.id == pane } }) {
            focus(space: space.id, pane: pane)
        }
    }

    private var voiceTransport: VoiceTransport {
        VoiceTransport(
            claim: { [weak self] in
                guard let self else { return }
                guard case .connected = connection, let client else { voice.refused("Not connected to the Mac."); return }
                let req = client.req()
                voiceClaimReq = req
                client.send(.leaseClaim(req: req, key: VoiceSession.leaseKey))
            },
            release: { [weak self] in
                guard let client = self?.client else { return }
                client.send(.leaseRelease(req: client.req(), key: VoiceSession.leaseKey))
            },
            name: { [weak self] agent in
                guard let self, let client else { return }
                let req = client.req()
                // Only a refusal to name an agent is news; clearing one cannot fail the person's way.
                if agent != nil { voiceNameReqs.insert(req) }
                client.send(.agentVoice(req: req, agent: agent))
            },
            send: { [weak self] agent, text in
                guard let self, let client, let chat = chats[agent] else { return }
                chat.error = nil
                chat.pinned = true
                let req = client.req()
                agentReqs[req] = agent
                Log.info("voice: send to \(agent), \(text.count) chars")
                client.send(.agentSend(req: req, agent: agent, text: text))
            }
        )
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

    func sendNow(_ chat: AgentChat, turn: UInt64) {
        Log.info("send now \(chat.name) turn \(turn)")
        chat.error = nil
        guard let client else { return }
        let req = client.req()
        agentReqs[req] = chat.name
        client.send(.agentSendNow(req: req, agent: chat.name, turn: turn))
    }

    /// Fork the agent's conversation through `turns` turns into a new pane beside its own.
    func fork(_ chat: AgentChat, turns: Int) {
        Log.info("fork \(chat.name) through \(turns) turn(s)")
        chat.error = nil
        guard let client else { return }
        let req = client.req()
        agentReqs[req] = chat.name
        client.send(.agentFork(req: req, agent: chat.name, turns: UInt32(turns)))
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
    /// Showing a pane shows Spaces, whichever mode was shown, but for a pane of the orchestrator's space in
    /// Orchestrator mode, which shows there as the carousel's page while the Spaces mode view stays.
    func focus(space: String, pane: String) {
        Log.info("focus pane \(pane) in \(space)")
        if mode == WindowMode.orchestrator, space == orchestratorSpace?.id {
            let page = pane == orchestratorPane ? nil : pane
            if page != orchestratorPage {
                orchestratorPage = page
                Log.info("orchestrator page \(page ?? "chat")")
                saveView()
            }
            if let i = spaces.firstIndex(where: { $0.id == space }), spaces[i].allPanes.contains(where: { $0.id == pane }), spaces[i].focus != pane {
                spaces[i].focus = pane
            }
            reportView()
            showTerm()
            return
        }
        setMode(WindowMode.spaces)
        guard let i = spaces.firstIndex(where: { $0.id == space && $0.allPanes.contains { $0.id == pane } }) else {
            pendingShow = (space, pane)
            return
        }
        if spaces[i].focus != pane { spaces[i].focus = pane }
        view.show(space: space, in: sessions)
        reportView()
        showTerm()
    }

    /// Show `space` on this phone: no other client moves. Showing a space shows Spaces, whichever mode was shown.
    func select(space: String) {
        Log.info("switch to space \(space)")
        setMode(WindowMode.spaces)
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

    /// The view saved at the last launch for this Mac, restored once the first layout is here: the mode, Spaces mode's
    /// session, space and pane, and Orchestrator mode's page. A saved page whose pane is gone falls back to the chat, and
    /// a saved pane to the space's first page.
    private func restoreView() {
        let saved = pairing.flatMap { UserDefaults.standard.dictionary(forKey: Self.viewKey($0)) as? [String: String] } ?? [:]
        mode = saved["mode"].flatMap { WindowMode.builtIn.contains($0) ? $0 : nil } ?? WindowMode.spaces
        // Checked against every space: the orchestrator's folder may not be listed yet, and the carousel shows the chat
        // for a page its space does not hold.
        orchestratorPage = saved["orchestratorPage"].flatMap { page in spaces.contains { $0.allPanes.contains { $0.id == page } } ? page : nil }
        Log.info("mode restored: \(mode), orchestrator page \(orchestratorPage ?? "chat")")
        let (restored, took) = SessionView.restored(session: saved["session"], space: saved["space"], in: sessions)
        view = restored
        if let pane = saved["pane"], let i = spaces.firstIndex(where: { $0.id == restored.space }), spaces[i].allPanes.contains(where: { $0.id == pane }) {
            spaces[i].focus = pane
        }
        Log.info("restored last view session=\(saved["session"] ?? "-") space=\(saved["space"] ?? "-") pane=\(saved["pane"] ?? "-"): took \(took), showing \(view.session ?? "none")/\(view.space ?? "none")/\(activeSpace?.focus ?? "none")")
    }

    /// Save what the phone shows for this Mac: the mode, Spaces mode's session, space and pane, and Orchestrator mode's
    /// page. Nothing is saved until the launch's first layout has restored the last save.
    private func saveView() {
        guard let pairing, !restorePending else { return }
        let shown = spaces.first { $0.id == view.space }
        var saved: [String: String] = [:]
        saved["mode"] = mode
        saved["session"] = view.session
        saved["space"] = shown?.id
        saved["pane"] = shown?.focus
        saved["orchestratorPage"] = orchestratorPage
        UserDefaults.standard.set(saved, forKey: Self.viewKey(pairing))
    }

    /// The name and mark each paired wheeld last reported.
    private static func hostKey(_ pairing: Pairing) -> String { "serverHost \(pairing.url.absoluteString)" }

    /// Each Mac's ids are its own, so the view is saved per paired daemon.
    private static func viewKey(_ pairing: Pairing) -> String { "lastView \(pairing.url.absoluteString)" }

    /// Tell the daemon what this phone shows (`client_ls` lists it); only a change is sent.
    private func reportView() {
        guard onScreen else { return }
        let space = activeSpace
        let key = "\(view.session ?? "-")/\(space?.id ?? "-")/\(space?.focus ?? "-")"
        guard key != reportedView else { return }
        reportedView = key
        saveView()
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

    /// `term`'s own page came up: make its surface the first time.
    func pageAppeared(_ term: String) {
        guard terminals[term] == nil else { return }
        _ = makeSurface(term)
        if attachedTerms.contains(term) {
            // Already streamed for another surface (a mirror's or an embed's): attach again for the replay, which goes to
            // this surface alone.
            Log.info("attaching to \(term) for a new surface")
            client?.send(.termAttach(req: client?.req(), id: term))
        }
    }

    private func makeSurface(_ term: String) -> PhoneTerminal {
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
            space.ownedPanes.contains { if case let .terminal(t, _) = $0.kind { return t == term } else { return false } }
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
        if mode == WindowMode.orchestrator, orchestratorPane != nil {
            let page = pane == orchestratorPane ? nil : pane
            if page != orchestratorPage {
                orchestratorPage = page
                saveView()
            }
        }
        showTerm()
    }

    private func showTerm() {
        shownTerm = ownTerm(of: shownPane)
        sheetPane = activeSpace.flatMap { space in sheet(in: space.id).flatMap { $0.position == .header ? nil : $0.pane } }
        let term = ownTerm(of: sheetPane)
        if term != sheetTerm { Log.info("sheet terminal: \(term ?? "none")") }
        sheetTerm = term
        syncAttachments()
    }

    /// The term whose own page surface `pane` draws: a terminal pane's, never a mirror's, which draws an embed of its own.
    private func ownTerm(of pane: String?) -> String? {
        guard let pane, mirror(pane) == nil else { return nil }
        return activeSpace?.allPanes.first { $0.id == pane }.flatMap { entry -> String? in
            if case let .terminal(term, _) = entry.kind { term } else { nil }
        }
    }

    /// Every surface that draws `term`: its page's and each embed's.
    private func surfaces(of term: String) -> [PhoneTerminal] {
        guard !embeds.isEmpty else { return terminals[term].map { [$0] } ?? [] }
        return (terminals[term].map { [$0] } ?? []) + embeds.filter { $0.term == term }
    }

    /// The embed of `term` on the shown plugin page that sizes the shell: the newest one up.
    private func drivingEmbed(of term: String) -> PhoneTerminal? {
        embeds.last { $0.term == term && ($0.host == shownPane || ($0.host != nil && $0.host == sheetPane)) }
    }

    /// Attach the terms the shown page needs (its own, or the embeds of a plugin page) and detach the rest.
    private func syncAttachments() {
        var wanted = Set<String>()
        if onScreen {
            if let shownTerm { wanted.insert(shownTerm) }
            if let sheetTerm { wanted.insert(sheetTerm) }
            for embed in embeds where embed.host == shownPane || (embed.host != nil && embed.host == sheetPane) { wanted.insert(embed.term) }
        }
        for old in attachedTerms.subtracting(wanted) {
            client?.send(.termDetach(id: old))
            for terminal in surfaces(of: old) { terminal.attached = false }
            Log.info("detached from \(old)")
        }
        let added = wanted.subtracting(attachedTerms)
        attachedTerms = wanted
        for term in added { attach(term) }
    }

    /// A plugin page's `Terminal{pane}`, or a mirror page of a terminal, came on screen: a surface of its own for `term`,
    /// streamed while `host` is shown.
    func embedTerminal(host: String, term: String) -> PhoneTerminal {
        PhoneTerminal(term: term, host: host, controller: terminalController(for: spaceId(ofTerm: term)), model: self)
    }

    func embedShown(_ terminal: PhoneTerminal) {
        guard !embeds.contains(where: { $0 === terminal }) else { return }
        embeds.append(terminal)
        terminal.attached = false
        terminal.size = nil
        Log.info("\(embedKind(terminal)) embed terminal \(terminal.host ?? "?") shows \(terminal.term): \(embeds.count) embed surfaces")
        if attachedTerms.contains(terminal.term) {
            // Already streamed (another embed of it): attach again for the replay, which goes to this surface alone.
            Log.info("attaching to \(terminal.term) for a new surface")
            client?.send(.termAttach(req: client?.req(), id: terminal.term))
        } else {
            syncAttachments()
        }
    }

    func embedHidden(_ terminal: PhoneTerminal) {
        guard let i = embeds.firstIndex(where: { $0 === terminal }) else { return }
        embeds.remove(at: i)
        terminal.attached = false
        if let replay = replays[terminal.term] { replays[terminal.term] = (replay.bytes, replay.to.filter { $0 !== terminal }) }
        Log.info("\(embedKind(terminal)) embed terminal \(terminal.host ?? "?") hid \(terminal.term): \(embeds.count) embed surfaces")
        syncAttachments()
        // The next newest embed of the term, if one is left, sizes the shell now.
        if let next = drivingEmbed(of: terminal.term) {
            next.size = nil
            settle(next)
        }
    }

    private func embedKind(_ terminal: PhoneTerminal) -> String {
        terminal.host.flatMap { mirror($0) } == nil ? "plugin" : "mirror"
    }

    /// Artifact `name` shown inside plugin pane `host`'s page, its page asked of wheeld over this connection.
    func artifactEmbed(host: String, name: String) -> AnyView {
        let embeds = artifactEmbeds ?? ArtifactEmbeds(daemon: pageDaemon, send: { [weak self] in self?.client?.send($0) },
                                                      nextReq: { [weak self] in self?.client?.req() ?? 0 })
        artifactEmbeds = embeds
        let cwd = spaces.first { $0.allPanes.contains { $0.id == host } }?.cwd ?? ""
        return embeds.view(pane: host, name: name, cwd: cwd)
    }

    /// What mirror pane `pane` shows and is called; nil for any other pane.
    func mirror(_ pane: String) -> MirrorShown? {
        spaces.lazy.compactMap { $0.mirrors[pane] }.first
    }

    /// What pane `pane` shows, in any space; nil when it is gone.
    func kind(ofPane pane: String) -> PaneKind? {
        for space in spaces {
            if let entry = space.allPanes.first(where: { $0.id == pane }) { return entry.kind }
        }
        return nil
    }

    #if DEBUG
    /// A check drives a browser pane's row by writing `<pane> <word> [<text>]` to `Documents/toolbar-press`:
    /// `back`, `forward`, `home` or `reload` presses that button through the call its tap makes; `type <text>` puts the
    /// URL bar in the person's hands holding `<text>`, as typing does; `submit` is Return in the bar
    /// (the field's `onSubmit` call, then the keyboard let go); `blur` lets go with no Return; `state`
    /// logs `toolbar <pane>: back=on|off/image|noimage forward=… home=… reload=… typing=true|false field=<what the bar holds>`.
    private func watchToolbarPresses() {
        guard let file = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first?
            .appendingPathComponent("toolbar-press") else { return }
        Task { @MainActor [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(250))
                guard let self, onScreen, let text = try? String(contentsOf: file, encoding: .utf8) else { continue }
                try? FileManager.default.removeItem(at: file)
                let words = text.split(separator: " ", maxSplits: 2).map(String.init)
                guard words.count >= 2, let browser = browsers[words[0]] else { continue }
                let pane = words[0]
                Log.info("toolbar \(pane): \(words[1])\(words.count > 2 ? " \(words[2])" : "")")
                switch words[1] {
                case "type" where words.count == 3:
                    browser.typing = true
                    browser.urlText = words[2]
                case "submit":
                    submitURL(in: pane)
                    browser.typing = false
                case "blur":
                    browser.typing = false
                case "state":
                    let images = BrowserButton.allCases.map {
                        "\($0.rawValue)=\(browser.enabled($0) ? "on" : "off")/\(UIImage(systemName: $0.symbol) == nil ? "noimage" : "image")"
                    }
                    Log.info("toolbar \(pane): \(images.joined(separator: " ")) typing=\(browser.typing) field=\(browser.urlText)")
                default:
                    if let button = BrowserButton(rawValue: words[1]) { press(button, in: pane) }
                }
            }
        }
    }

    /// The surface checks type into: the shown terminal page's, else the shown plugin page's first terminal embed.
    private var shownSurface: PhoneTerminal? {
        if let shownTerm { return terminals[shownTerm] }
        return embeds.last { $0.host == shownPane }
    }

    /// Checks type through the surface as the keyboard would: `text` as a paste, then Return.
    /// False while no attached surface is shown.
    @discardableResult
    func typeIntoShown(_ text: String) -> Bool {
        guard let terminal = shownSurface, terminal.attached else { return false }
        let pasted = terminal.view.paste(text: text), entered = terminal.view.sendKey(.enter)
        Log.info("typed into \(terminal.term)\(terminal.host.map { " in plugin pane \($0)" } ?? ""): paste \(pasted), return \(entered)")
        return true
    }

    /// Checks read what the shown page's own surface holds: `screen <pane>: <surface> <rows joined by " | ">`, where
    /// `<surface>` is `own` for the terminal page's surface and `embed` for a mirror's or plugin page's.
    func logShownScreen() {
        guard let pane = shownPane, let terminal = shownSurface else { return Log.info("screen \(shownPane ?? "none"): no surface") }
        let rows = (terminal.session.readViewportText() ?? "").split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
        Log.info("screen \(pane): \(terminal.host == nil ? "own" : "embed") \(rows.filter { !$0.isEmpty }.joined(separator: " | "))")
    }

    /// Checks raise the keyboard in the shown terminal as a tap on it does.
    func focusShown() {
        guard let terminal = shownSurface else { return }
        terminal.view.requestFocus()
        Log.info("keyboard requested in \(terminal.term)")
    }
    #endif

    private func attach(_ term: String) {
        // Made here, before the attach this sends, so it takes this attach's replay.
        if term == shownTerm || term == sheetTerm, terminals[term] == nil { _ = makeSurface(term) }
        replays[term] = nil
        for terminal in surfaces(of: term) {
            terminal.attached = false
            terminal.size = nil
        }
        Log.info("attaching to \(term)")
        client?.send(.termAttach(req: client?.req(), id: term))
        for terminal in surfaces(of: term) { settle(terminal) }
    }

    private func dropGoneTerminals() {
        let held = Set(spaces.flatMap { $0.allPanes.compactMap { pane -> String? in
            if case let .terminal(term, _) = pane.kind { term } else { nil }
        } })
        for term in terminals.keys where !held.contains(term) {
            terminals[term] = nil
            if shownTerm == term { shownTerm = nil }
            if sheetTerm == term { sheetTerm = nil }
            Log.info("terminal \(term) left the layout")
        }
        attachedTerms.formIntersection(held)
    }

    func typed(_ terminal: PhoneTerminal, _ data: Data) {
        guard terminal.attached else { return }
        client?.send(.termInput(req: nil, id: terminal.term, data: data))
    }

    func resized(_ terminal: PhoneTerminal, _ vp: InMemoryTerminalViewport) {
        guard vp.columns > 0, vp.rows > 0 else { return }
        terminal.reported = vp
        settle(terminal)
    }

    /// The surface's grid fills its laid-out page, give or take its padding and one cell.
    private func fitsPage(_ terminal: PhoneTerminal) -> Bool {
        guard let vp = terminal.reported, let layout = terminal.layout, vp.widthPixels > 0, vp.heightPixels > 0,
              vp.columns > 0, vp.rows > 0 else { return false }
        let slack = PhoneTerminal.paddingPoints * layout.scale
        let dw = layout.size.width * layout.scale - CGFloat(vp.widthPixels)
        let dh = layout.size.height * layout.scale - CGFloat(vp.heightPixels)
        // The surface reports no cell size on iOS (0x0), so the cell is the grid's pixels over its count.
        let cellW = CGFloat(vp.widthPixels) / CGFloat(vp.columns), cellH = CGFloat(vp.heightPixels) / CGFloat(vp.rows)
        return (0...slack + cellW).contains(dw) && (0...slack + cellH).contains(dh)
    }

    /// Send the surface's grid to the daemon once it fits the page's laid-out size, and only for
    /// the terminal the page shows. A new surface first reports a default frame before SwiftUI
    /// lays it out, which would resize the shell to a grid that is not the phone's.
    /// Called when the surface reports, when the page's layout changes and when it is attached,
    /// whichever comes last.
    func settle(_ terminal: PhoneTerminal) {
        // The shown page sizes the shell: the terminal's own page, or the newest embed of it on a plugin page.
        let drives = terminal.host == nil ? terminal.term == shownTerm || terminal.term == sheetTerm : drivingEmbed(of: terminal.term) === terminal
        guard drives, terminal.attached || terminal.size == nil, let vp = terminal.reported else { return }
        let cols = Int(vp.columns), rows = Int(vp.rows)
        guard let layout = terminal.layout, vp.widthPixels > 0, vp.heightPixels > 0 else {
            Log.info("surface \(terminal.term) \(cols)x\(rows): held until the page is laid out")
            return
        }
        let fits = fitsPage(terminal)
        Log.info("surface \(terminal.term) \(cols)x\(rows) at \(vp.widthPixels)x\(vp.heightPixels)px in page \(layout.size.width)x\(layout.size.height)pt @\(layout.scale)x: \(fits ? "sent" : "held until laid out")")
        guard fits else { return }
        if let size = terminal.size, size == (cols, rows) { return }
        terminal.size = (cols, rows)
        client?.send(.termResize(req: nil, id: terminal.term, cols: cols, rows: rows))
        if terminal.attached, terminal.replayBeforeLayout {
            // The replay came while the surface still had its frame from before layout: ask again, after the resize, so
            // it draws the screen at this grid. It goes to this surface alone.
            Log.info("surface \(terminal.term) took its replay before layout: attaching again at \(cols)x\(rows)")
            terminal.replayBeforeLayout = false
            terminal.attached = false
            client?.send(.termAttach(req: client?.req(), id: terminal.term))
        }
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
        // In Orchestrator mode only the panes the orchestrator placed are links: a pane in another space is plain text,
        // so no tap leaves the mode or adds a page.
        resolver.paneChip = { [unowned self] id in
            if mode == WindowMode.orchestrator, orchestratorSpace?.allPanes.contains(where: { $0.id == id }) != true { return nil }
            return ChatLinks.chip(for: id, in: spaces)
        }
        resolver.request = { [unowned self] make in
            guard let client, isConnected else { return nil }
            let req = client.req()
            client.send(make(req))
            return req
        }
        resolver.offers = { [unowned self] op in serverOffer.has(op) }
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
        if let setting = list.first(where: { $0.key == "orchestrator.dir" }), setting.orchestratorDir != orchestratorDir {
            orchestratorDir = setting.orchestratorDir
            Log.info("orchestrator folder \(setting.orchestratorDir)")
            readTopic()
        }
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
    /// Whether `pane` sends no push of its own, as wheeld's layout last said.
    func isQuiet(_ pane: String) -> Bool { spaces.contains { $0.quietPanes.contains(pane) } }
    func setPush(_ pane: String, on: Bool) { menuOp { .panePush(req: $0, pane: pane, on: on) } }
    func setKind(_ pane: String, _ kind: NewPaneKind, artifact: ArtifactInfo? = nil, url: String? = nil) {
        menuOp { .paneSetKind(req: $0, pane: pane, kind: kind, artifact: artifact?.name, scope: artifact?.scope, url: url) }
    }
    /// A browser pane on whatever `text` names, as typing it into the Mac's address field does: a bare
    /// host becomes https. An empty text opens the homepage setting.
    func openBrowser(_ pane: String, _ text: String) {
        setKind(pane, .browser, url: text.isEmpty ? nil : text)
    }
    /// What the person typed into browser pane `pane`'s URL bar: wheeld reads it and this phone's
    /// own browser goes there (the `opened` reply); the Mac's browser is not asked.
    func browse(_ pane: String, _ text: String) {
        guard client?.connection == .connected else {
            menuError = "Not connected to wheeld"
            Log.info("browser pane \(pane) URL bar: not connected to wheeld")
            return
        }
        menuOp(clearsError: false) { .paneBrowserOpen(req: $0, pane: pane, text: text) }
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
        client.send(.artifactLs(req: req, space: activeSpace?.id))
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

/// A space's pin sheet: the pinned pane it shows and where it sits.
struct PinSheet: Equatable {
    var pane: String
    var position: SheetPosition
}
