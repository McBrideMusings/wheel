import Foundation
import GhosttyTerminal
import SwiftUI
import WheelKit

/// Everything the window shows, fed by one connection to wheeld. The daemon holds the real
/// state (ADR-0001): the spaces and their split trees arrive whole in each `spaces` broadcast,
/// and this model changes them only by sending ops. Quitting and relaunching loses nothing.
/// The release daemon's address, for a build that carries no wheeld of its own.
private let wheelDefaultListen = "127.0.0.1:7420"

@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var connection: DaemonClient.Connection = .connecting
    @Published private(set) var spaces: [SpaceInfo] = []
    /// The space the grid shows; nil when there are none.
    @Published private(set) var active: String?
    /// The last layout op the daemon refused, shown above the grid until the next op that
    /// changes the layout (a focus click leaves it up).
    @Published private(set) var layoutError: String?
    /// Ratios of dividers being dragged, drawn in place of the tree's. A divider still under the
    /// pointer keeps its entry through any broadcast; once its `split_ratio` is sent, the entry
    /// goes with the next broadcast (sent even when the value is unchanged) or a refusal.
    @Published private(set) var dragRatios: [String: Double] = [:]
    /// Dividers whose drag ended and whose `split_ratio` went out, waiting for the daemon.
    private var settling: Set<String> = []
    /// The lead's conversation; always attached while connected.
    let lead = SessionChat(name: AppModel.lead)
    /// Every attached session by name: the lead and one per agent pane in the layout.
    @Published private(set) var chats: [String: SessionChat] = [:]
    /// Permission requests waiting on a person, from every session.
    @Published private(set) var perms: [PermInfo] = []
    /// The Settings window: wheel's settings for the chosen project, and the edits to them.
    let settings = SettingsModel()
    /// The artifact palette open inside a pane, by pane id: at most one per pane, drawn over that
    /// pane alone.
    @Published var palettes: [String: PanePalette] = [:]
    /// `artifact_ls` requests, by request id, so the answer reaches the palette that asked.
    var paletteReqs: [UInt64: String] = [:]

    /// Tool-call ids of the lead's transcript rows that show at least partly in the transcript's
    /// viewport. A row scrolled out of view is absent from this set.
    @Published private(set) var visibleLeadCalls: Set<String> = []
    /// The last scroll the debug endpoint asked of the lead's transcript; the view acts on each new
    /// serial. A check needs a row out of view without a pointer.
    @Published private(set) var leadScroll = LeadScroll(serial: 0, toTop: false)

    struct LeadScroll: Equatable {
        var serial: Int
        var toTop: Bool
    }

    func leadRow(_ call: String, visible: Bool) {
        if visible { visibleLeadCalls.insert(call) } else { visibleLeadCalls.remove(call) }
    }

    /// What the request card last drew of its asking session's tool line, for the state query.
    /// Not published: the card writes it after drawing, and nothing reads it to draw.
    private(set) var requestCardShown: (call: String, content: [ToolContent])?

    func recordRequestCard(call: String, content: [ToolContent]) {
        requestCardShown = (call, content)
        Log.info("request card drew \(call) with \(content.count) content items")
    }

    /// The card stopped showing this call's tool line (answered, or the row is gone).
    func clearRequestCard(call: String) {
        guard requestCardShown?.call == call else { return }
        requestCardShown = nil
        Log.info("request card cleared \(call)")
    }

    func scrollLead(toTop: Bool) {
        leadScroll = LeadScroll(serial: leadScroll.serial + 1, toTop: toTop)
    }

    /// The one request the lead's transcript draws, expanded on its tool line: the oldest waiting
    /// lead request whose line is on screen. Only the lead's transcript draws one: it is always on
    /// screen, where an agent pane may be closed or hidden.
    var inlinePerm: PermInfo? {
        perms.first { $0.session == Self.lead && $0.toolCall.map(visibleLeadCalls.contains) == true }
    }

    /// The request to draw expanded on this tool line, if it is the one `inlinePerm` names.
    func perm(session: String, toolCall: String) -> PermInfo? {
        guard let perm = inlinePerm, perm.session == session, perm.toolCall == toolCall else { return nil }
        return perm
    }

    /// Requests the card above the composer takes one at a time: every waiting request except the
    /// one `inlinePerm` draws expanded on its line, so each one has choices somewhere.
    var boxPerms: [PermInfo] {
        let inline = inlinePerm?.id
        let shown = shownWorkerSessions
        return perms.filter { $0.id != inline && !shown.contains($0.session) }
    }

    /// What a session's header says it is doing: a turn blocked on a permission request waits on
    /// the person, so it reads "Waiting on you" instead of the daemon's busy "Working".
    func stateLabel(_ chat: SessionChat) -> String {
        if case let .busy(queued) = chat.state, perms.contains(where: { $0.session == chat.name }) {
            return queued > 0 ? "Waiting on you · \(queued) queued" : "Waiting on you"
        }
        // A lead that needs a login cannot take a turn, whatever its session says.
        if chat.name == Self.lead, leadLogin?.state == "needed", case .ready = chat.state {
            return "Needs login"
        }
        return chat.state?.label ?? "Not running"
    }

    /// Where a waiting request stands among all of them, counting from 1: the same count the card
    /// and the lead's tool lines report.
    func position(of perm: PermInfo) -> Int {
        perms.firstIndex { $0.id == perm.id }.map { $0 + 1 } ?? perms.count
    }
    /// The conversations the lead's agent holds for its directory, newest first: read again each
    /// time the lead attaches (which a new or resumed conversation also causes) and each time a
    /// turn ends, since a turn moves its conversation to the top.
    /// The lead's login, from the daemon; nil until it has said. The lead card shows a picker while
    /// `showsPicker`.
    @Published private(set) var leadLogin: LeadLogin?
    /// A login the picker started and the daemon has not finished.
    @Published private(set) var loginStarting = false
    private var loginReq: UInt64?
    @Published private(set) var leadSessions: [ConversationInfo] = []
    /// Why the list could not be read — an agent that keeps no conversations says so here.
    @Published private(set) var leadSessionsError: String?
    /// The newest listing request, the only one whose answer is taken: an older one arriving last
    /// would put back a list from before. Its failure fills `leadSessionsError` rather than the
    /// composer's error line.
    private var listReq: UInt64?
    /// The conversation the lead is in, which the Resume menu checks. A switch this window asked
    /// for sets it from the daemon's reply, before any listing says so — a fresh conversation may
    /// not be listed at all; a listing asked after that switch names it otherwise.
    @Published private(set) var leadConversation: String?
    /// The switch request whose reply last set `leadConversation`: a listing asked before it went
    /// out describes the conversation the lead left.
    private var leadConversationReq: UInt64 = 0

    /// Whether `id` is the conversation the lead is in: the one answer the Resume menu's check and
    /// the state query's `lead_sessions` rows both give.
    func isLeadConversation(_ id: String) -> Bool { id == leadConversation }

    /// The conversation picker over the lead's transcript; nil while it is closed.
    @Published private(set) var leadPicker: ConversationPicker?
    /// When the picker opened, to time its first paint.
    private var leadPickerOpenedAt: Date?
    /// A synthetic listing standing in for the agent's, for timing the picker over thousands of
    /// rows; set from the debug endpoint only.
    private(set) var leadFixture = false

    /// One Ghostty surface per terminal pane, keyed by the term id the tree names.
    private(set) var terminals: [String: PaneTerminal] = [:]
    /// One page per web pane, keyed by pane id.
    private(set) var webPanes: [String: WebPane] = [:]
    /// One page and its chrome per browser pane, keyed by pane id.
    private(set) var browserPanes: [String: BrowserPane] = [:]
    /// Where each pane on screen sits in the window, in SwiftUI's global space: what `render_pane`
    /// cuts out of the window for a pane that is not a web page. Not published, so a move redraws nothing.
    var paneFrames: [String: CGRect] = [:]
    /// The pane whose unlock the confirm sheet asks about.
    @Published var unlockPrompt: String?
    /// Locked panes showing their read-only transcript in place of the dashboard.
    @Published private(set) var transcriptPanes: Set<String> = []
    /// Who the pointer is over in a managed pane's badge: that manager's pane (or the lead card) draws an outline.
    @Published private(set) var highlightedManager: CreatedBy?
    /// When this window saw each pane change from locked to unlocked.
    private(set) var unlockedAt: [String: Date] = [:]
    /// The daemon's pane-locking switch as its last `status` said. Off, no pane is
    /// locked and no lock control is drawn; this window keeps no setting of its own.
    @Published private(set) var paneLocking = false
    /// What each pane's lock was at the last layout, to see it change.
    private var locks: [String: ManagedPane.Lock] = [:]
    /// One board per supervisor pane, keyed by pane id.
    private(set) var supervisorPanes: [String: SupervisorPaneModel] = [:]
    /// What the last `supervisor_show` said of each supervisor, by pane: the space tabs' marks.
    @Published private(set) var supervisorInfos: [String: SupervisorInfo] = [:]
    /// The `supervisor_*` requests out, by request: the pane whose board the reply fills.
    private var supervisorReqs: [UInt64: String] = [:]
    /// The ones a button press sent, whose success clears the board's error line.
    private var supervisorControlReqs: Set<UInt64> = []
    /// The `worker_push` requests out, by request: the supervisor pane whose drill-in they leave.
    private var workerPushReqs: [UInt64: String] = [:]
    /// Asks the daemon about every supervisor pane while any is in the layout.
    private var supervisorPoll: Task<Void, Never>?
    /// The lead's inbox as `inbox_ls` last listed it, and the open count `inbox_count` last sent:
    /// the lead card's "Inbox N" and its For you list.
    @Published private(set) var inboxItems: [InboxItem] = []
    @Published private(set) var inboxOpen = 0
    /// The lead card shows the full board (Supervisors and For you) in place of the transcript.
    @Published var leadBoardExpanded = false {
        didSet { if leadBoardExpanded { askInbox() } }
    }
    private var inboxReqs: Set<UInt64> = []
    /// The daemon token, for the web panes' page addresses; read at each connect.
    private var pageToken: String?
    /// Ghostty's default light/dark theme pair, shared by every pane's surface, with padding
    /// inside the terminal's own background.
    private let controller = TerminalController {
        $0.withWindowPaddingX(10)
        $0.withWindowPaddingY(8)
    }
    let endpoint: DaemonEndpoint
    let client: DaemonClient
    /// Session requests (a composer's send, an attach), by session, so a failure shows under
    /// that session's composer.
    private var sessionReqs: [UInt64: String] = [:]
    /// Layout ops, so only their failures show above the grid.
    private var layoutReqs: Set<UInt64> = []
    /// Typed address-field text the daemon is reading, by request: the pane and the text, so the
    /// answer reaches that pane and a refusal shows over it.
    private var openReqs: [UInt64: (pane: String, text: String)] = [:]
    /// `<pane>/<term>` of the focus this window last acted on, so a broadcast that leaves it
    /// unchanged does not pull the keyboard back from wherever the person put it.
    private(set) var focusKey: String?
    /// The daemon token, which also guards the debug endpoint.
    let tokenFile: URL
    let sidebar = SidebarLayout()
    /// The wheeld this app carries and runs; nil when WHEEL_URL points the app at another daemon
    /// (every check does), or the build has none.
    let service: DaemonService?

    init() {
        let env = ProcessInfo.processInfo.environment
        let override = env["WHEEL_URL"].flatMap(URL.init(string:))
        service = override == nil ? DaemonService.bundled() : nil
        let url = override ?? service?.url ?? URL(string: "ws://\(wheelDefaultListen)/ws")!
        tokenFile = env["WHEEL_TOKEN_FILE"].map { URL(filePath: $0) }
            ?? service?.tokenFile
            ?? FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Application Support/wheel/token")
        let endpoint = DaemonEndpoint(url: url, token: DaemonEndpoint.tokenFile(tokenFile))
        self.endpoint = endpoint
        client = DaemonClient(endpoint: endpoint)
        chats[lead.name] = lead
        client.log = { Log.info($0) }
        client.onConnection = { [weak self] state in
            self?.connection = state
            if state != .connected { self?.terminals.values.forEach { $0.attached = false } }
        }
        client.onConnect = { [weak self] status in self?.connected(status) }
        client.onMessage = { [weak self] msg in self?.handle(msg) }
    }

    func start() {
        Log.info("app start, daemon \(endpoint.url.absoluteString)")
        service?.register()
        client.start()
    }

    var activeSpace: SpaceInfo? { spaces.first { $0.id == active } }

    /// Send the daemon a frame that is not a request of this model's own.
    func reply(_ msg: ClientMsg) { client.send(msg) }

    /// The daemon's status at connect carries the whole layout; every terminal pane and every
    /// session reattaches.
    private func connected(_ status: ServerMsg) {
        // Answers to the last connection's requests never come.
        sessionReqs = [:]
        for open in openReqs.values {
            browserPanes[open.pane]?.refused(typed: open.text, "The connection to wheeld dropped before it read this address.")
        }
        openReqs = [:]
        supervisorReqs = [:]
        supervisorControlReqs = []
        workerPushReqs = [:]
        inboxReqs = []
        askInbox()
        client.send(.renderOffer)
        settings.forgetRequests()
        // Answers to the last connection's palette listings never come.
        paletteReqs = [:]
        listReq = nil
        // The daemon may have restarted with another conversation; the lead's attach lists again.
        leadSessions = []
        leadSessionsError = nil
        leadPicker = nil
        leadFixture = false
        leadConversation = nil
        leadConversationReq = 0
        let known = Array(chats.values)
        if case let .status(_, _, _, uptime, _, leadState, perms, spaces, active, locking) = status {
            // The restarted daemon closes this connection; the client reconnects to the new one.
            if service?.restartIfOlder(uptimeSecs: uptime) == true { return }
            lead.state = leadState
            self.perms = perms
            paneLocking = locking
            apply(spaces, active)
        }
        // A palette open across a reconnect lists again, against the layout the new daemon sent.
        for pane in palettes.keys { askArtifacts(pane) }
        if settings.open { askSettings() }
        askLeadLogin()
        // `apply` attached the sessions it added; the rest were attached on an earlier connection.
        for chat in known where chats[chat.name] === chat { attach(chat) }
        for terminal in terminals.values { attach(terminal) }
        Task {
            do {
                pageToken = try await endpoint.token()
            } catch {
                Log.info("no token for web panes: \(error.localizedDescription)")
                return
            }
            // A page loaded before this connect may be stale (the daemon restarted, files changed).
            for web in webPanes.values { web.loaded ? web.reload() : open(web) }
        }
    }

    /// Mirror a new layout: make a surface for each terminal pane that appeared, drop the ones
    /// whose pane went, and move keyboard focus when the daemon moved it. Returns the new
    /// surfaces, which the caller attaches.
    @discardableResult
    private func apply(_ spaces: [SpaceInfo], _ active: String?) -> [PaneTerminal] {
        self.spaces = spaces
        self.active = active
        let held = Set(spaces.flatMap { $0.root.panes.map { $0.id } })
        paneFrames = paneFrames.filter { held.contains($0.key) }
        // A palette is for the pane as it was when it opened: a pane that left, or that something
        // else (the lead, the CLI) changed to another kind, closes it.
        let kinds = Dictionary(uniqueKeysWithValues: spaces.flatMap { $0.root.panes }.map { ($0.id, $0.kind.name) })
        for (pane, palette) in palettes where kinds[pane] != palette.openedOver {
            palettes[pane] = nil
            Log.info("palette \(pane): its pane \(kinds[pane] == nil ? "left the layout" : "became \(kinds[pane] ?? "")"); closed")
        }
        keepSettingsProject()
        trackLocks(spaces)
        settle()
        let plan = SurfacePlan(spaces, active: active, existing: Set(terminals.keys), focusKey: focusKey)
        for term in plan.removed {
            client.send(.termDetach(id: term))
            terminals[term] = nil
            Log.info("terminal \(term) left the layout; detached")
        }
        let added = plan.added.map { term in
            let terminal = PaneTerminal(term: term, controller: controller, model: self)
            terminals[term] = terminal
            return terminal
        }
        for (term, visible) in plan.visible { terminals[term]?.view.isSurfaceVisible = visible }
        let artifacts = WebPanes.artifacts(spaces)
        for (pane, web) in webPanes where artifacts[pane] != web.artifact {
            webPanes[pane] = nil
            Log.info("web pane \(pane) left the layout")
        }
        for (pane, artifact) in artifacts where webPanes[pane] == nil {
            let web = WebPane(pane: pane, artifact: artifact)
            webPanes[pane] = web
            open(web)
        }
        let pages = BrowserPanes.urls(spaces)
        for pane in browserPanes.keys where pages[pane] == nil {
            browserPanes[pane] = nil
            Log.info("browser pane \(pane) left the layout")
        }
        for (pane, url) in pages {
            if let browser = browserPanes[pane] {
                browser.daemonSays(url)
            } else {
                Log.info("browser pane \(pane) opening \(url)")
                browserPanes[pane] = BrowserPane(
                    pane: pane, url: url, daemonIsRemote: !endpoint.isLoopback,
                    report: { [weak self] in self?.reportPage(pane, $0) },
                    ask: { [weak self] in self?.openTyped(pane, $0) }
                )
            }
        }
        let sessions = Set(AgentPanes.sessions(spaces).values).union(drilledSessions)
        for name in chats.keys where name != Self.lead && !sessions.contains(name) {
            client.send(.sessionDetach(session: name))
            chats[name] = nil
            Log.info("session \(name) left the layout; detached")
        }
        for name in sessions where chats[name] == nil {
            let chat = SessionChat(name: name)
            chats[name] = chat
            attach(chat)
        }
        applySupervisors(spaces)
        Log.info("spaces: \(spaces.map { "\($0.id)(\($0.root.panes.map { "\($0.id):\($0.kind.name)" }.joined(separator: ",")))" }.joined(separator: " ")) active=\(active ?? "none")")
        focusKey = plan.focusKey
        if let term = plan.focus { terminals[term]?.view.requestFocus() }
        return added
    }

    private func handle(_ msg: ServerMsg) {
        switch msg {
        case let .supervisor(req, info):
            supervisorReqs[req] = nil
            guard let pane = supervisorPanes[info.pane] else { return }
            if supervisorControlReqs.remove(req) != nil { pane.error = nil }
            if pane.info != info { pane.info = info }
            pane.remember(info.running)
            // A worker that ended while open takes the pane back to its supervisor.
            if let open = pane.openWorker, !info.running.contains(where: { $0.session == open }) {
                Log.info("worker \(open) ended; supervisor pane \(info.pane) is back on the supervisor")
                closeWorker(info.pane)
            }
            if supervisorInfos[info.pane] != info { supervisorInfos[info.pane] = info }
            Log.info("supervisor \(info.supervisor.id) \(info.supervisor.state.rawValue): \(info.running.count) running, \(info.inbox) in the inbox")
        case let .supervisorEvents(req, events):
            guard let id = supervisorReqs.removeValue(forKey: req) else { return }
            if supervisorPanes[id]?.events != events { supervisorPanes[id]?.events = events }
        case let .supervisorInbox(req, items):
            guard let id = supervisorReqs.removeValue(forKey: req) else { return }
            if supervisorPanes[id]?.inbox != items { supervisorPanes[id]?.inbox = items }
        case let .inbox(req, items):
            guard inboxReqs.remove(req) != nil else { return }
            if inboxItems != items { inboxItems = items }
            // The daemon pushes the count only when it changes, so a connection that has seen no
            // change reads the open count off the list.
            let open = LeadBoard.waiting(items).count
            if inboxOpen != open { inboxOpen = open }
            Log.info("lead inbox: \(items.count) items, \(open) open")
        case let .inboxCount(new, open):
            Log.info("lead inbox count: \(new) new, \(open) open")
            if inboxOpen != open { inboxOpen = open }
            askInbox()
        case let .status(_, _, _, _, _, leadState, perms, spaces, active, locking):
            // An unsolicited status means this connection missed daemon-wide events.
            lead.state = leadState
            self.perms = perms
            paneLocking = locking
            apply(spaces, active).forEach(attach)
        case let .spaces(spaces, active):
            apply(spaces, active).forEach(attach)
        case let .renderPane(id, pane):
            renderPane(id: id, pane: pane)
        case let .paneReload(pane):
            webPanes[pane]?.reload()
        case let .paneUrl(pane, url):
            // One field of one pane, so the layout is patched in place rather than replaced: the
            // daemon sends this instead of the whole layout because a person browsing changes it
            // on every page load. A pane this client does not hold is not an error — the next
            // `spaces` or `status` carries the truth.
            spaces = spaces.map { space in
                var space = space
                space.root = space.root.replacing(pane: pane, with: .browser(url: url))
                return space
            }
            browserPanes[pane]?.daemonSays(url)
        case let .settings(req, space, list):
            received(settings: list, for: space, req: req)
        case let .artifacts(req, list):
            if let req, let pane = paletteReqs.removeValue(forKey: req) {
                palettes[pane]?.state.listed(list)
                Log.info("palette \(pane): \(list.count) artifacts listed")
            }
        case let .created(req, id):
            if let pane = workerPushReqs.removeValue(forKey: req) {
                supervisorPanes[pane]?.openWorker = nil
                Log.info("worker pushed to pane \(id); supervisor pane \(pane) is back on the supervisor")
                askSupervisor(pane)
            }
            if layoutReqs.remove(req) != nil { Log.info("created \(id)") }
        case let .started(req, pane, session):
            if layoutReqs.remove(req) != nil { Log.info("started \(pane) session \(session ?? "-")") }
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
        case let .leadLoginInfo(_, login), let .leadLoginChanged(login):
            leadLogin = login
            Log.info("lead login \(login.state) agentRequired=\(login.agentRequired) methods=\(login.methods.map(\.id).joined(separator: ",")) running=\(login.runningPane ?? "-")")
        case let .leadLoginFinished(req, code):
            if req == loginReq { loginReq = nil; loginStarting = false }
            Log.info("lead login finished with \(code.map(String.init) ?? "a signal")")
        case let .sessionStateChanged(session, state):
            chats[session]?.state = state
            if session == Self.lead, state == .ready { refreshLeadSessions() }
        case let .sessionAttached(req, session, state, events, config, context, now):
            if let req { sessionReqs[req] = nil }
            guard let chat = chats[session] else { return }
            chat.state = state
            chat.reset(events, daemonNow: now)
            chat.setConfig(config)
            chat.context = context
            chat.error = nil
            chat.notice = nil
            Log.info("session \(session) attached, \(events.count) events, \(config.count) config options, context \(context?.helpText ?? "unknown")")
            if session == Self.lead { refreshLeadSessions() }
        case let .conversations(req, session, conversations):
            guard req == listReq, session == Self.lead else {
                Log.info("lead conversations for request \(req) dropped: newest is \(listReq.map(String.init) ?? "none")")
                return
            }
            listReq = nil
            leadSessions = conversations
            leadSessionsError = nil
            leadPicker?.listed(conversations)
            let listed = conversations.first { $0.current }?.id
            if req > leadConversationReq, let listed { leadConversation = listed }
            Log.info("lead conversations: \(conversations.count), current \(listed ?? "none"), menu checks \(leadConversation ?? "none")")
        case let .conversation(req, session, id):
            sessionReqs[req] = nil
            if session == Self.lead {
                leadConversation = id
                leadConversationReq = req
            }
            Log.info("session \(session) is in conversation \(id)")
        case let .sessionConfig(session, options):
            guard let chat = chats[session] else { return }
            chat.setConfig(options)
            Log.info("session \(session) config: \(options.map { "\($0.id)=\($0.currentLabel ?? "?")" }.joined(separator: " "))")
        case let .sessionContext(session, context):
            guard let chat = chats[session] else { return }
            chat.context = context
            Log.info("session \(session) context: \(context?.helpText ?? "unknown")")
        case let .sessionNotice(session, notice):
            guard let chat = chats[session] else { return }
            chat.notice = notice
            Log.info("session \(session) notice: \(notice ?? "down")")
        case let .session(session, event):
            chats[session]?.apply(event)
        case let .permRequested(perm):
            if !perms.contains(where: { $0.id == perm.id }) { perms.append(perm) }
            Log.info("permission \(perm.id) waiting: \(perm.title)")
        case let .permResolved(id, choice):
            perms.removeAll { $0.id == id }
            Log.info("permission \(id) resolved: \(choice)")
        case let .sessionQueued(req, _, _):
            sessionReqs[req] = nil
        case let .opened(req, pane, url):
            if openReqs.removeValue(forKey: req) != nil { browserPanes[pane]?.opened(url) }
        case let .ok(req):
            layoutReqs.remove(req)
        case let .error(req, term, message):
            if let req, req == listReq {
                listReq = nil
                leadSessionsError = message
                leadPicker?.failed(message)
                Log.info("lead conversations not listed: \(message)")
                return
            }
            if let req { supervisorControlReqs.remove(req) }
            if let req, let pane = workerPushReqs.removeValue(forKey: req) {
                supervisorPanes[pane]?.error = message
                Log.info("worker push from \(pane) refused: \(message)")
            }
            if let req, let pane = supervisorReqs.removeValue(forKey: req) {
                supervisorPanes[pane]?.error = message
                Log.info("supervisor request for \(pane) refused: \(message)")
            }
            if let req, let session = sessionReqs.removeValue(forKey: req) { chats[session]?.error = message }
            if let req, let open = openReqs.removeValue(forKey: req) { browserPanes[open.pane]?.refused(typed: open.text, message) }
            if let req, layoutReqs.remove(req) != nil {
                layoutError = message
                settle()
            }
            if let req, let field = settings.changeReqs.removeValue(forKey: req) {
                Log.info("setting \(field.key) (\(field.layer.rawValue)) refused: \(message)")
                settings.refused(field, message)
            }
            if let req, let pane = paletteReqs.removeValue(forKey: req) {
                palettes[pane]?.state.failed(message)
                Log.info("palette \(pane): listing refused: \(message)")
            }
            if let req, req == settings.listReq {
                settings.listReq = nil
                settings.listFailed(message)
            }
            Log.info("daemon error\(term.map { " on \($0)" } ?? ""): \(message)")
        default:
            break
        }
    }

    // MARK: Supervisors

    /// Keep one board per supervisor pane in the layout, and ask about each one now when the
    /// layout says its state moved.
    private func applySupervisors(_ spaces: [SpaceInfo]) {
        let held = SupervisorPanes.all(spaces)
        let shown = Set(AgentPanes.sessions(spaces).values)
        for board in supervisorPanes.values { board.forget(except: shown) }
        for pane in supervisorPanes.keys where held[pane] == nil {
            supervisorPanes[pane] = nil
            supervisorInfos[pane] = nil
            Log.info("supervisor pane \(pane) left the layout")
        }
        for (pane, supervisor) in held {
            if let board = supervisorPanes[pane] {
                if board.supervisor != supervisor {
                    board.supervisor = supervisor
                    askSupervisor(pane)
                }
            } else {
                supervisorPanes[pane] = SupervisorPaneModel(pane: pane, supervisor: supervisor)
                Log.info("supervisor pane \(pane) shows \(supervisor.id)")
                askSupervisor(pane)
            }
        }
        if supervisorPanes.isEmpty {
            supervisorPoll?.cancel()
            supervisorPoll = nil
        } else if supervisorPoll == nil {
            supervisorPoll = Task { @MainActor [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(2))
                    guard let self else { return }
                    for (pane, board) in self.supervisorPanes {
                        board.now = Date()
                        self.askSupervisor(pane)
                    }
                }
            }
        }
    }

    /// Ask the daemon for the lead's inbox. The count it pushes moves only when items open or
    /// close, so the list is asked again then and whenever the board opens.
    func askInbox() {
        guard connection == .connected else { return }
        let req = client.req()
        inboxReqs.insert(req)
        client.send(.inboxLs(req: req))
    }

    /// What the lead card's board draws: every supervisor across spaces and the open inbox items.
    var leadBoard: LeadBoard {
        let summaries = LeadBoard.summaries(spaces, infos: supervisorInfos) { pane in
            supervisorPanes[pane]?.board.map { ($0.landed.count, $0.total) }
        }
        return LeadBoard(supervisors: summaries, forYou: LeadBoard.waiting(inboxItems), inboxOpen: inboxOpen)
    }

    /// Ask the daemon for one supervisor's workers, decisions and inbox.
    func askSupervisor(_ pane: String) {
        guard connection == .connected, let id = supervisorPanes[pane]?.supervisor.id else { return }
        for make in [ClientMsg.supervisorShow, ClientMsg.supervisorEvents, ClientMsg.supervisorInbox] {
            let req = client.req()
            supervisorReqs[req] = pane
            client.send(make(req, id))
        }
    }

    private func controlSupervisor(_ pane: String, _ what: String, _ make: (UInt64, String) -> ClientMsg) {
        guard let id = supervisorPanes[pane]?.supervisor.id else { return }
        let req = client.req()
        supervisorReqs[req] = pane
        supervisorControlReqs.insert(req)
        Log.info("supervisor \(id): \(what)")
        client.send(make(req, id))
    }

    func pauseSupervisor(_ pane: String) { controlSupervisor(pane, "pause") { .supervisorPause(req: $0, id: $1) } }
    func resumeSupervisor(_ pane: String) { controlSupervisor(pane, "resume") { .supervisorResume(req: $0, id: $1) } }
    func setSupervisorWorkers(_ pane: String, _ workers: Int) {
        controlSupervisor(pane, "workers \(workers)") { .supervisorWorkers(req: $0, id: $1, workers: workers) }
    }
    func setSupervisorCap(_ pane: String, _ cap: Int?) {
        controlSupervisor(pane, "cap \(cap.map(String.init) ?? "none")") { .supervisorCap(req: $0, id: $1, cap: cap) }
    }

    /// The worker sessions supervisor panes show drilled in.
    var drilledSessions: Set<String> { Set(supervisorPanes.values.compactMap(\.openWorker)) }

    /// The worker sessions drawn somewhere now, drilled in or in a pane of their own: their
    /// requests show on that card, so the lead card leaves them out.
    var shownWorkerSessions: Set<String> {
        // Only the space on screen draws a card, so only its panes count.
        guard let space = spaces.first(where: { $0.id == active }) else { return [] }
        var out = Set<String>()
        for pane in space.root.panes {
            switch pane.kind {
            case let .worker(session): out.insert(session)
            case .supervisor: if let open = supervisorPanes[pane.id]?.openWorker { out.insert(open) }
            default: break
            }
        }
        return out
    }

    /// Whether `pane` is the focused pane of the space on screen: the one a ⌘↩ answers for.
    func paneIsFocused(_ pane: String) -> Bool {
        spaces.first { $0.id == active }?.focus == pane
    }

    /// Open `session` in supervisor pane `pane`, in place of the supervisor.
    func openWorker(_ pane: String, session: String) {
        guard let board = supervisorPanes[pane] else { return }
        if chats[session] == nil {
            let chat = SessionChat(name: session)
            chats[session] = chat
            attach(chat)
        }
        board.error = nil
        board.openWorker = session
        Log.info("supervisor pane \(pane) opened worker \(session)")
    }

    /// Back to the supervisor, as it was.
    func closeWorker(_ pane: String) {
        guard let board = supervisorPanes[pane], let session = board.openWorker else { return }
        board.openWorker = nil
        Log.info("supervisor pane \(pane) is back on the supervisor, leaving worker \(session)")
        if !AgentPanes.sessions(spaces).values.contains(session), !drilledSessions.contains(session) {
            client.send(.sessionDetach(session: session))
            chats[session] = nil
        }
    }

    /// Give the worker open in `pane` a pane of its own; the supervisor pane goes back to the
    /// supervisor when the daemon has made it.
    func pushWorker(_ pane: String, session: String) {
        guard let board = supervisorPanes[pane] else { return }
        let req = client.req()
        workerPushReqs[req] = pane
        board.error = nil
        Log.info("supervisor \(board.supervisor.id): push worker \(session) to a pane")
        client.send(.workerPush(req: req, id: board.supervisor.id, session: session))
    }

    /// The supervisor pane a worker session belongs to, with the worker's line on its board.
    func workerLine(session: String) -> (pane: String, line: WorkerLine)? {
        for (pane, board) in supervisorPanes {
            if let line = board.board?.line(session: session, now: board.now) { return (pane, line) }
        }
        return nil
    }

    /// Text typed into a supervisor's composer. Typing folds the board back to its pinned lines.
    func supervisorTyped(pane: String, draft: String) {
        guard let board = supervisorPanes[pane], let chat = chats[board.supervisor.session] else { return }
        if draft != chat.draft, !draft.isEmpty, board.expanded {
            board.expanded = false
            Log.info("supervisor pane \(pane) collapsed: typing in the composer")
        }
        chat.draft = draft
    }

    // MARK: Managed panes

    /// Every supervisor in the layout by id: how a header names the manager of a pane.
    var supervisorsById: [String: Supervisor] {
        Dictionary(supervisorPanes.values.map { ($0.supervisor.id, $0.supervisor) }, uniquingKeysWith: { a, _ in a })
    }

    /// Who made pane `pane`, and who managed it before the person unlocked it.
    func managers(of pane: String) -> (createdBy: CreatedBy, prior: CreatedBy?)? {
        for space in spaces {
            if let found = Self.managers(of: pane, in: space.root) { return found }
        }
        return nil
    }

    private static func managers(of pane: String, in node: Node) -> (createdBy: CreatedBy, prior: CreatedBy?)? {
        switch node {
        case let .pane(id, _, createdBy, prior): id == pane ? (createdBy, prior) : nil
        case let .split(_, _, _, a, b): managers(of: pane, in: a) ?? managers(of: pane, in: b)
        }
    }

    /// The lock of pane `pane` as the layout last said.
    func lock(of pane: String) -> ManagedPane.Lock {
        managers(of: pane).map { ManagedPane.lock(createdBy: $0.createdBy, prior: $0.prior, locking: paneLocking) } ?? .yours
    }

    /// Forget the transcript toggle of a pane that is no longer locked, and note when one unlocked.
    private func trackLocks(_ spaces: [SpaceInfo]) {
        var now: [String: ManagedPane.Lock] = [:]
        for space in spaces { Self.collectLocks(space.root, locking: paneLocking, into: &now) }
        for (pane, lock) in now where lock == .unlocked && locks[pane] == .locked {
            unlockedAt[pane] = Date()
            Log.info("pane \(pane) unlocked by the person")
        }
        for (pane, lock) in locks where lock == .unlocked && now[pane] == .locked {
            Log.info("pane \(pane) handed back")
        }
        unlockedAt = unlockedAt.filter { now[$0.key] == .unlocked }
        let stillLocked = transcriptPanes.filter { now[$0] == .locked }
        if stillLocked != transcriptPanes { transcriptPanes = stillLocked }
        if let asked = unlockPrompt, now[asked] != .locked { unlockPrompt = nil }
        locks = now
    }

    private static func collectLocks(_ node: Node, locking: Bool, into out: inout [String: ManagedPane.Lock]) {
        switch node {
        case let .pane(id, _, createdBy, prior): out[id] = ManagedPane.lock(createdBy: createdBy, prior: prior, locking: locking)
        case let .split(_, _, _, a, b):
            collectLocks(a, locking: locking, into: &out)
            collectLocks(b, locking: locking, into: &out)
        }
    }

    /// What a locked pane draws, so the view and the state query read one answer.
    enum LockedShowing: String { case dashboard, transcript, content, loading }

    func lockedHasDashboard(_ kind: PaneKind) -> Bool {
        switch kind {
        case .worker, .supervisor: true
        default: false
        }
    }

    func lockedShowing(_ pane: String) -> LockedShowing {
        guard let kind = spaces.lazy.compactMap({ $0.root.panes.first { $0.id == pane }?.kind }).first else { return .loading }
        let transcript = transcriptPanes.contains(pane)
        switch kind {
        case let .worker(session):
            if transcript, chats[session] != nil { return .transcript }
            return supervisorPane(of: session) != nil ? .dashboard : .loading
        case let .supervisor(supervisor):
            guard supervisorPanes[pane] != nil, chats[supervisor.session] != nil else { return .loading }
            return transcript ? .transcript : .dashboard
        case let .agent(session): return chats[session] != nil ? .transcript : .loading
        default: return .content
        }
    }

    func toggleTranscript(_ pane: String) {
        if transcriptPanes.contains(pane) { transcriptPanes.remove(pane) } else { transcriptPanes.insert(pane) }
        Log.info("managed pane \(pane): transcript \(transcriptPanes.contains(pane) ? "on" : "off")")
    }

    /// The Unlock… button: ask first, naming what changes.
    func requestUnlock(_ pane: String) {
        guard lock(of: pane) == .locked, connection == .connected else { return }
        unlockPrompt = pane
    }

    func cancelUnlock() { unlockPrompt = nil }

    /// The sheet's Unlock and take over.
    func confirmUnlock() {
        guard let pane = unlockPrompt else { return }
        unlockPrompt = nil
        guard connection == .connected else { return }
        layout { .paneUnlock(req: $0, pane: pane) }
    }

    func handBack(_ pane: String) { layout { .paneHandBack(req: $0, pane: pane) } }

    /// The manager's badge was hovered or left.
    func highlight(manager: CreatedBy?) {
        if highlightedManager != manager { highlightedManager = manager }
    }

    /// The pane that shows `manager`: a supervisor's pane; the lead has a card, not a pane.
    func pane(of manager: CreatedBy) -> String? {
        guard case let .supervisor(id) = manager else { return nil }
        return supervisorPanes.first { $0.value.supervisor.id == id }?.key
    }

    /// The manager's badge was clicked: focus its pane (the lead card is always on screen).
    func focus(manager: CreatedBy) {
        if let pane = pane(of: manager) { focus(pane: pane) }
    }

    /// The supervisor pane whose loop ran the worker in `session`, with the board it draws.
    func supervisorPane(of session: String) -> (id: String, pane: SupervisorPaneModel)? {
        supervisorPanes.first { $0.value.seen[session] != nil }.map { ($0.key, $0.value) }
    }

    /// The bead the worker in `session` is on, while a supervisor pane remembers it.
    func workerBead(session: String) -> String? { supervisorPane(of: session)?.pane.seen[session]?.bead }

    /// The dashboard of the worker in `session`, from its supervisor's board and its own transcript.
    func workerDashboard(session: String, now: Date = Date()) -> WorkerDashboard? {
        guard let (_, pane) = supervisorPane(of: session), let board = pane.board, let worker = pane.seen[session] else { return nil }
        return WorkerDashboard(worker: worker, board: board, rows: chats[session]?.transcript.rows ?? [],
                               supervisorName: board.supervisor.scope, now: now)
    }

    // MARK: Layout ops — the tab bar, pane headers, dividers and the debug endpoint all call these.

    private func layout(clearsError: Bool = true, _ make: (UInt64) -> ClientMsg) {
        if clearsError { layoutError = nil }
        let req = client.req()
        layoutReqs.insert(req)
        let msg = make(req)
        Log.info("layout op: \((try? msg.json()) ?? "?")")
        client.send(msg)
    }

    func createSpace(cwd: String) { layout { .spaceCreate(req: $0, cwd: cwd) } }
    func closeSpace(_ id: String) { layout { .spaceClose(req: $0, id: id) } }
    func focus(space: String) { layout(clearsError: false) { .focus(req: $0, space: space, pane: nil) } }
    func focus(pane: String) { layout(clearsError: false) { .focus(req: $0, space: nil, pane: pane) } }
    func split(_ pane: String, _ dir: Dir) { layout { .paneSplit(req: $0, pane: pane, dir: dir) } }
    func closePane(_ pane: String) { layout { .paneClose(req: $0, pane: pane) } }
    func setKind(_ pane: String, _ kind: NewPaneKind, artifact: String? = nil, scope: ArtifactScope? = nil) {
        layout { .paneSetKind(req: $0, pane: pane, kind: kind, artifact: artifact, scope: scope) }
    }
    func startPane(_ pane: String, _ kind: StartKind, message: String? = nil) {
        layout { .paneStart(req: $0, pane: pane, kind: kind, message: message) }
    }
    func setRatio(_ split: String, _ ratio: Double) {
        // Offline the op goes nowhere and no answer comes, so the divider snaps back now.
        guard connection == .connected else {
            dragRatios[split] = nil
            return
        }
        settling.insert(split)
        layout { .splitRatio(req: $0, split: split, ratio: ratio) }
    }
    /// Draw a divider at `ratio` while it is dragged; `setRatio` at the drag's end tells the daemon.
    func dragRatio(_ split: String, _ ratio: Double) { dragRatios[split] = ratio }

    /// The daemon answered: drop the overrides of every divider whose ratio it has heard.
    private func settle() {
        for split in settling { dragRatios[split] = nil }
        settling = []
    }

    /// A terminal or an agent pane took keyboard focus (the person clicked into it): tell the
    /// daemon, unless it already has that pane focused.
    func paneFocused(pane: String) {
        guard let space = spaces.first(where: { $0.root.panes.contains { $0.id == pane } }),
              space.focus != pane || space.id != active else { return }
        focusKey = SurfacePlan.key(pane: pane, in: spaces)
        focus(pane: pane)
    }

    // MARK: Sessions — the lead card and every agent pane

    /// The lead's session name.
    static let lead = "lead"

    /// Stream a session's conversation to this window; `session_attached` resets its transcript.
    private func attach(_ chat: SessionChat) {
        let req = client.req()
        sessionReqs[req] = chat.name
        Log.info("attaching to session \(chat.name)")
        client.send(.sessionAttach(req: req, session: chat.name))
    }

    /// Take down the notice over `chat`: the banner's close button. The daemon forgets it and tells
    /// every client, this one included, so the banner goes when the daemon says so.
    func dismissNotice(_ chat: SessionChat) {
        guard chat.notice != nil, connection == .connected else { return }
        Log.info("dismiss notice over \(chat.name)")
        client.send(.sessionNoticeDismiss(req: nil, session: chat.name))
    }

    /// Send a session's draft, the text in its composer. The draft clears once the message goes
    /// out; when it can't go out now, nothing is sent and the draft stays in the box.
    func send(_ chat: SessionChat) {
        let text = chat.draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        guard connection == .connected else {
            chat.error = "Not connected to wheeld; your message is still in the box."
            return
        }
        chat.error = nil
        // Sending is going back to the conversation's end: the person wants to see what they sent
        // and what comes back, wherever they had scrolled.
        chat.pinned = true
        let req = client.req()
        sessionReqs[req] = chat.name
        // Sized, never the text, as the daemon logs it.
        Log.info("send to \(chat.name), \(text.count) chars")
        client.send(.sessionSend(req: req, session: chat.name, text: text))
        chat.draft = ""
    }

    func cancel(_ chat: SessionChat) {
        Log.info("cancel \(chat.name)")
        client.send(.sessionCancel(req: client.req(), session: chat.name))
    }

    /// Change one of a session's config options. The control redraws when the daemon broadcasts
    /// the agent's new list, never from the click, so what it shows is always what the agent took.
    func setConfig(_ chat: SessionChat, _ option: ConfigOption, to value: String) {
        guard connection == .connected else {
            chat.error = "Not connected to wheeld."
            return
        }
        chat.error = nil
        let req = client.req()
        sessionReqs[req] = chat.name
        Log.info("set \(chat.name) \(option.id) to \(value)")
        client.send(.sessionConfigSet(req: req, session: chat.name, config: option.id, value: value))
    }

    /// Whether the lead can move to another conversation now: the daemon refuses a switch while a
    /// turn runs or waits in the queue, so the menu does not offer one.
    var canSwitchLead: Bool { connection == .connected && lead.state == .ready }

    /// Log the lead in with the picker's choice: the daemon opens a terminal pane running that
    /// method, where the person answers the login.
    func login(method: AuthMethodInfo) {
        guard connection == .connected, !loginStarting else { return }
        let req = client.req()
        loginReq = req
        loginStarting = true
        Log.info("lead login with \(method.id), request \(req)")
        client.send(.leadLogin(req: req, method: method.id))
    }

    /// Ask the daemon for the lead's login, running its preflight.
    func askLeadLogin() {
        guard connection == .connected else { return }
        client.send(.leadLoginStatus(req: client.req()))
    }

    /// Ask the lead's agent which conversations it holds; `conversations` answers.
    func refreshLeadSessions() {
        guard !leadFixture else { return }
        guard connection == .connected else {
            leadPicker?.failed("wheeld is not connected")
            return
        }
        let req = client.req()
        listReq = req
        leadPicker?.relisting()
        Log.info("listing lead conversations, request \(req)")
        client.send(.sessionConversationLs(req: req, session: Self.lead))
    }

    // MARK: Conversation picker — the header's history button, the keys and the debug endpoint call these.

    func toggleLeadPicker() {
        if leadPicker == nil { openLeadPicker() } else { closeLeadPicker() }
    }

    /// Show the picker in place of the transcript and ask the agent for a fresh listing; what the
    /// last listing held shows meanwhile.
    func openLeadPicker() {
        guard leadPicker == nil else { return }
        var picker = ConversationPicker()
        if !leadSessions.isEmpty { picker.listed(leadSessions) }
        leadPicker = picker
        leadPickerOpenedAt = Date()
        Log.info("conversation picker opened, \(leadSessions.count) conversations held")
        refreshLeadSessions()
    }

    func closeLeadPicker() {
        guard leadPicker != nil else { return }
        leadPicker = nil
        leadPickerOpenedAt = nil
        Log.info("conversation picker closed")
    }

    func leadPickerTyped(_ text: String) {
        let start = Date()
        leadPicker?.type(text)
        guard let picker = leadPicker else { return }
        Log.info("conversation picker query \"\(text)\": \(picker.rows.count) of \(picker.all.count) in \(Int(Date().timeIntervalSince(start) * 1000)) ms")
    }

    func leadPickerMoved(by: Int) { leadPicker?.move(by) }

    func leadPickerSelected(_ id: String) { leadPicker?.select(id) }

    /// Return: resume the selected conversation, or close when it is the one the lead is in.
    func leadPickerEntered() {
        guard let picked = leadPicker?.selectedConversation else { return }
        leadPickerChose(picked.id)
    }

    func leadPickerChose(_ id: String) {
        guard canSwitchLead else {
            Log.info("conversation picker: \(id) not resumed, the lead is working")
            return
        }
        if !isLeadConversation(id) { resumeLeadConversation(id) }
        closeLeadPicker()
    }

    /// The first row of the list drew: log how long the picker took from the button to the screen.
    func leadPickerPainted() {
        guard let opened = leadPickerOpenedAt, leadPicker?.firstPaintMs == nil else { return }
        let ms = Int(Date().timeIntervalSince(opened) * 1000)
        leadPicker?.firstPaintMs = ms
        Log.info("conversation picker first paint \(ms) ms, \(leadPicker?.all.count ?? 0) conversations")
    }

    /// Replace the agent's listing with `count` synthetic conversations (0 restores the real one).
    func useLeadFixture(count: Int) {
        leadFixture = count > 0
        listReq = nil
        guard count > 0 else { refreshLeadSessions(); return }
        let base = Date()
        let f = ISO8601DateFormatter()
        leadSessions = (0..<count).map { i in
            ConversationInfo(id: "fixture-\(i)", cwd: "/fixture", title: "Fixture conversation \(i) about topic \(i % 97)",
                             updatedAt: f.string(from: base.addingTimeInterval(-Double(i) * 600)), current: false)
        }
        leadSessionsError = nil
        leadPicker?.listed(leadSessions)
        Log.info("lead conversations: fixture of \(count)")
    }

    /// Put the lead in a fresh conversation. The daemon resends the transcript, empty, to every
    /// client, and a refusal shows under the composer.
    func newLeadConversation() {
        guard connection == .connected else {
            lead.error = "Not connected to wheeld."
            return
        }
        lead.error = nil
        closeLeadPicker()
        let req = client.req()
        sessionReqs[req] = lead.name
        Log.info("new conversation for lead")
        client.send(.sessionConversationNew(req: req, session: lead.name))
    }

    /// Put the lead back in the past conversation `id`; its replayed history becomes the
    /// transcript. A load that fails leaves the lead where it was and shows under the composer.
    func resumeLeadConversation(_ id: String) {
        guard connection == .connected else {
            lead.error = "Not connected to wheeld."
            return
        }
        lead.error = nil
        let req = client.req()
        sessionReqs[req] = lead.name
        Log.info("resume lead conversation \(id)")
        client.send(.sessionConversationResume(req: req, session: lead.name, id: id))
    }

    /// Allow or deny a waiting request; the daemon broadcasts the result to every client.
    func answer(_ perm: PermInfo, _ choice: String) {
        Log.info("answering \(perm.id): \(choice)")
        client.send(.permAnswer(req: client.req(), id: perm.id, choice: choice))
    }

    // MARK: Settings — the Settings window and the debug endpoint call these.

    /// The window came up: the Project tab starts on the space being shown.
    func settingsAppeared() {
        Log.info("settings window opened")
        settings.open = true
        settings.project = active ?? spaces.first?.id
        askSettings()
    }

    func settingsDisappeared() {
        Log.info("settings window closed")
        settings.open = false
    }

    /// The Project tab's list: edit this space's `wheel.toml`.
    func selectSettingsProject(_ id: String) {
        guard id != settings.project else { return }
        Log.info("settings: project \(id)")
        settings.project = id
        askSettings()
    }

    /// The chosen project gone from the layout: the space being shown, else the first, else none.
    private func keepSettingsProject() {
        guard settings.open else { return }
        if let project = settings.project, spaces.contains(where: { $0.id == project }) { return }
        // No spaces and none chosen: the machine view it holds is still the one to show.
        if settings.project == nil, spaces.isEmpty { return }
        settings.project = active ?? spaces.first?.id
        askSettings()
    }

    /// Ask for every setting as it resolves for the chosen project. Each carries this machine's
    /// value too, so one list serves both tabs; with no project it is the machine-wide view.
    func askSettings() {
        guard connection == .connected else { return }
        let req = client.req()
        settings.listReq = req
        settings.asking(for: settings.project)
        Log.info("settings ls for \(settings.project ?? "no space")")
        client.send(.settingsLs(req: req, space: settings.project))
    }

    /// Return in a field: write what was typed into that layer's file, or remove the key from it
    /// when the field was emptied. A field holding what its file sets sends nothing.
    func saveSetting(_ field: SettingsModel.Field) {
        guard settings.dirty(field), let text = settings.drafts[field] else { return }
        if text.trimmingCharacters(in: .whitespaces).isEmpty {
            clearSetting(field)
        } else {
            change(field, "set \(field.key) = \(text)") { .settingSet(req: $0, key: field.key, value: text, space: self.settings.project, machine: field.layer == .global) }
        }
    }

    /// The clear button: remove the key from that layer's file, so the layer below is in force.
    func clearSetting(_ field: SettingsModel.Field) {
        change(field, "unset \(field.key)") { .settingUnset(req: $0, key: field.key, space: self.settings.project, machine: field.layer == .global) }
    }

    private func change(_ field: SettingsModel.Field, _ what: String, _ make: (UInt64) -> ClientMsg) {
        guard connection == .connected else {
            settings.refused(field, "Not connected to wheeld; your change is still in the field.")
            return
        }
        let req = client.req()
        settings.changeReqs[req] = field
        settings.sending(field)
        Log.info("setting \(what) in \(field.layer == .global ? "this machine's" : "\(settings.project ?? "?")'s") wheel.toml")
        client.send(make(req))
    }

    /// A list of settings: the answer to a change or a list, or another client's change. A
    /// broadcast resolved for a space other than the chosen project still may have changed this
    /// machine's value, which every list carries, so the window asks for its own again.
    private func received(settings list: [SettingInfo], for space: String?, req: UInt64?) {
        Log.info("settings for \(space ?? "no space"): \(list.map { "\($0.key)=\($0.value)(\($0.origin.rawValue)) machine=\($0.machine ?? "-") project=\($0.project ?? "-")" }.joined(separator: " "))")
        if let req, let field = settings.changeReqs.removeValue(forKey: req) {
            settings.changed(field)
            // Answered for the project the change was made on; if the tab has moved on since, its
            // own list is already on the way.
            if space == settings.project { settings.show(list, for: space) }
        } else if let req, req == settings.listReq {
            settings.listReq = nil
            settings.show(list, for: space)
        } else if req == nil {
            if space == settings.space, settings.loaded {
                settings.show(list, for: space)
            } else if settings.open {
                askSettings()
            }
        }
    }

    // MARK: Web panes

    /// Load a web pane's page, once the token for its address is known.
    private func open(_ web: WebPane) {
        guard let token = pageToken, let url = WebPanes.url(daemon: endpoint.url, pane: web.pane, token: token) else { return }
        web.load(url)
    }

    // MARK: Browser panes

    /// A browser pane landed on a new page: the saved layout follows it. The report carries no
    /// request, so the daemon refusing a page no pane may be on (a `data:` document) only logs.
    private func reportPage(_ pane: String, _ url: String) {
        client.send(.paneBrowserUrl(req: nil, pane: pane, url: url))
    }

    /// Ask the daemon what text typed into a browser pane's address field means.
    private func openTyped(_ pane: String, _ text: String) {
        guard connection == .connected else {
            browserPanes[pane]?.refused(typed: text, "Not connected to wheeld, which says what an address means.")
            return
        }
        let req = client.req()
        openReqs[req] = (pane, text)
        client.send(.paneBrowserOpen(req: req, pane: pane, text: text))
    }

    /// Send a browser pane to the `browser.homepage` setting as the daemon resolves it for the
    /// pane's space: the kind again with no page, which is what opening one without a page does.
    func goHome(_ pane: String) {
        browserPanes[pane]?.clearFailure()
        layout { .paneSetKind(req: $0, pane: pane, kind: .browser) }
    }

    // MARK: Terminals

    private func attach(_ terminal: PaneTerminal) {
        Log.info("attaching to \(terminal.term)")
        client.send(.termAttach(req: client.req(), id: terminal.term))
        if let size = terminal.size { client.send(.termResize(req: nil, id: terminal.term, cols: size.cols, rows: size.rows)) }
    }

    fileprivate func typed(_ term: String, _ data: Data) {
        guard terminals[term]?.attached == true else { return }
        // A locked pane takes no input, whoever holds the keyboard focus.
        let locked = spaces.contains { space in
            space.root.panes.contains { pane in
                if case let .terminal(t) = pane.kind, t == term { return lock(of: pane.id) == .locked }
                return false
            }
        }
        if locked {
            Log.info("terminal \(term): input dropped, its pane is locked")
            return
        }
        client.send(.termInput(req: nil, id: term, data: data))
    }

    fileprivate func resized(_ term: String, _ vp: InMemoryTerminalViewport) {
        guard vp.columns > 0, vp.rows > 0, let terminal = terminals[term] else { return }
        terminal.reported = vp
        settle(terminal)
    }

    /// Send the surface's grid to the daemon once it fits the pane's laid-out size. A new surface
    /// first reports a default 800x600 frame minus its padding, before SwiftUI lays it out in the
    /// pane; the daemon would resize the shell to that grid and then again to the real one.
    /// Called when the surface reports and when the pane's layout changes, whichever comes last.
    func settle(_ terminal: PaneTerminal) {
        guard let vp = terminal.reported else { return }
        let cols = Int(vp.columns), rows = Int(vp.rows)
        guard let layout = terminal.layout, vp.widthPixels > 0, vp.heightPixels > 0 else {
            Log.info("surface \(terminal.term) \(cols)x\(rows): held until the pane is laid out")
            return
        }
        let slack = PaneTerminal.paddingPoints * layout.scale
        let dw = layout.size.width * layout.scale - CGFloat(vp.widthPixels)
        let dh = layout.size.height * layout.scale - CGFloat(vp.heightPixels)
        let fits = (0...slack + CGFloat(vp.cellWidthPixels)).contains(dw) && (0...slack + CGFloat(vp.cellHeightPixels)).contains(dh)
        Log.info("surface \(terminal.term) \(cols)x\(rows) at \(vp.widthPixels)x\(vp.heightPixels)px in pane \(layout.size.width)x\(layout.size.height)pt @\(layout.scale)x: \(fits ? "sent" : "held until laid out")")
        guard fits else { return }
        if let size = terminal.size, size == (cols, rows) { return }
        terminal.size = (cols, rows)
        client.send(.termResize(req: nil, id: terminal.term, cols: cols, rows: rows))
    }
}

/// One attached session's conversation: the lead's, or an agent pane's. Each is its own
/// object so a pane redraws only for its own session's events.
@MainActor
final class SessionChat: ObservableObject {
    let name: String
    @Published var state: SessionState?
    /// The folded conversation. Not published: views draw `rows`, so a streamed chunk redraws the
    /// one row it changes and not every view of the list.
    private(set) var transcript = Transcript()
    /// One observable object per transcript row, in order. Reassigned only when a row is added,
    /// removed or moved; a chunk that changes a row's text changes that row's object alone.
    @Published private(set) var rows: [RowModel] = []
    /// `rows` as the transcript lays them out: runs of tool calls grouped. Rebuilt with `rows`;
    /// a row never changes between tool and not-tool, so nothing else moves a group's edges.
    @Published private(set) var items: [ChatItem] = []
    /// How many times `rows` was reassigned and how many row objects were told to redraw, for the
    /// state query: a streamed chunk adds one to `rowUpdates` and nothing to `listUpdates`.
    private(set) var listUpdates = 0
    private(set) var rowUpdates = 0
    /// The last thing the daemon refused for this session, shown under its composer until the
    /// next send or attach.
    @Published var error: String?
    /// A status the daemon holds over this session, drawn as a dismissible banner above its
    /// composer: a conversation that would not reopen. Never part of the transcript.
    @Published var notice: String?
    /// How full the conversation's context window is, as the agent last reported it; nil while it
    /// has reported none, and the header then shows nothing.
    @Published var context: ContextUse?
    /// The text in this session's composer, not yet sent.
    @Published var draft = ""
    /// The transcript follows new output: true until the person scrolls away from the end, and
    /// again once they scroll back to it. Not published: the view reads it when content changes.
    var pinned = true
    /// A person's scroll of the transcript is under way (dragging, or coasting after it).
    var userScrolling = false
    /// When a scroll-wheel event or a scrolling key last reached this transcript. A notched mouse
    /// wheel and the keys report no scroll phase, so this is how the view tells its move from layout settling.
    var wheelAt = Date.distantPast
    /// Counts the End key (and a Down that lands at the end): the view re-pins and goes to its last
    /// item by instruction, since a scroll view's own end shifts while rows near it are measured.
    @Published var endRequest = 0
    /// The window-wide scroll-wheel and key monitor that stamps `wheelAt`, held while the view is on screen.
    var wheelMonitor: Any?
    /// How far the transcript's view sits above its end, in points, for the state query.
    var fromEnd: CGFloat = 0
    /// Where the transcript's scroll view sits in its window, top-left origin, so the debug
    /// endpoint can find that scroll view and move it the way a person's scroll does.
    var frameInWindow: CGRect = .zero
    /// Tool lines, tool groups and thoughts the person opened, by row id (a group by its first
    /// call's). Held here rather than in the views so a row rebuilt when its pane is rebuilt stays
    /// open, and so the debug endpoint can open one the way a click does.
    @Published private(set) var expanded: Set<Int> = []

    func toggle(_ id: Int) {
        if expanded.remove(id) == nil { expanded.insert(id) }
        Log.info("transcript \(name): \(expanded.contains(id) ? "opened" : "closed") \(id)")
    }
    /// Counts requests to put the keyboard in this session's composer; the debug endpoint is the
    /// only sender, standing in for a click the program cannot make.
    @Published var composerFocusRequest = 0
    /// The controls the agent offers over this session, in the order they are drawn. Empty while
    /// the agent starts, and for an agent that offers none.
    @Published private(set) var config: [ConfigOption] = []

    /// Store the agent's options in draw order, so the card and the state query can never
    /// disagree about which control comes first.
    func setConfig(_ options: [ConfigOption]) {
        // Swift's sort is not documented as stable, so options the agent gave no category to keep
        // the order it sent them in only if their position is part of the comparison.
        config = options.enumerated()
            .sorted { ($0.element.category.map(Self.rank) ?? Self.uncategorised, $0.offset) < ($1.element.category.map(Self.rank) ?? Self.uncategorised, $1.offset) }
            .map(\.element)
    }

    /// ACP's own categories say what an option is for, so the row can order them without this
    /// wheel knowing any option's name: what answers, then how hard it thinks, then what it may
    /// do without asking. Anything uncategorised — an agent's own extra control — sorts last,
    /// keeping the order the agent sent it in.
    private static let uncategorised = 4

    private static func rank(_ category: String) -> Int {
        switch category {
        case "model": return 0
        case "thought_level": return 1
        case "mode": return 2
        case "model_config": return 3
        default: return uncategorised
        }
    }

    init(name: String) {
        self.name = name
    }

    func reset(_ events: [StampedEvent], daemonNow: Date) {
        transcript.reset(events, daemonNow: daemonNow)
        syncRows()
    }

    func apply(_ event: StampedEvent) {
        transcript.apply(event)
        syncRows()
    }

    /// Each tool group by the id of a row in it, so a change to one call redraws its group's line.
    private var groupOf: [Int: ToolGroupModel] = [:]

    /// Bring `rows` in line with the transcript, keeping each row's object by its id.
    private func syncRows() {
        let source = transcript.rows
        if source.count == rows.count, zip(source, rows).allSatisfy({ $0.id == $1.id }) {
            for (row, model) in zip(source, rows) where model.update(row) { updated(model) }
            return
        }
        let existing = Dictionary(rows.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        var next: [RowModel] = []
        for (index, row) in source.enumerated() {
            if let model = existing[row.id] {
                model.index = index
                if model.update(row) { updated(model) }
                next.append(model)
            } else {
                next.append(RowModel(row, index: index, chat: self))
            }
        }
        rows = next
        // A group keeps its object as calls join it, keyed by its first call.
        let groups = Dictionary(groupOf.values.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        groupOf = [:]
        items = Transcript.items(source).map { item in
            switch item {
            case let .row(index):
                return .row(next[index])
            case let .tools(range):
                let members = Array(next[range])
                let group = groups[members[0].id] ?? ToolGroupModel(id: members[0].id)
                group.set(members)
                for member in members { groupOf[member.id] = group }
                return .tools(group)
            }
        }
        listUpdates += 1
    }

    private func updated(_ row: RowModel) {
        rowUpdates += 1
        groupOf[row.id]?.objectWillChange.send()
    }
}

/// A run of tool calls drawn as one group. Its own object, so the group's line redraws when any
/// of its calls changes, and keeps its identity — and whether it is open — as calls join it.
@MainActor
final class ToolGroupModel: ObservableObject, Identifiable {
    let id: Int
    private(set) var rows: [RowModel] = []

    /// The group's entry in `SessionChat.expanded`: below zero, so it never names the same thing
    /// as its first call's row id.
    var openKey: Int { -1 - id }

    init(id: Int) {
        self.id = id
    }

    func set(_ members: [RowModel]) {
        guard members.map(\.id) != rows.map(\.id) else { return }
        objectWillChange.send()
        rows = members
    }
}

/// One entry of the laid-out transcript: a row, or a run of tool calls drawn as one group.
enum ChatItem: Identifiable {
    case row(RowModel)
    case tools(ToolGroupModel)

    /// The first row's id, so a group keeps its identity as calls join it.
    var id: Int {
        switch self {
        case let .row(row): row.id
        case let .tools(group): group.id
        }
    }

    @MainActor var isUser: Bool {
        if case let .row(row) = self, case .user = row.row.kind { return true }
        return false
    }
}

/// One transcript row as a view observes it. Its views redraw when this row changes, not when
/// another row does. The row's contents are read from the transcript and never stored here: a
/// second owner of a streaming reply's string would make every appended chunk copy the reply.
@MainActor
final class RowModel: ObservableObject, Identifiable {
    let id: Int
    /// The row's position in the chat's transcript.
    fileprivate(set) var index: Int
    /// The transcript row's revision when this object last told its views to redraw.
    private var revision: Int
    private weak var chat: SessionChat?
    /// Times a view built this row's body, for the state query.
    private(set) var draws = 0

    init(_ row: TranscriptRow, index: Int, chat: SessionChat) {
        id = row.id
        revision = row.revision
        self.index = index
        self.chat = chat
    }

    /// The row as the transcript holds it now.
    /// A row the transcript no longer holds (a view still drawing it as the list is replaced)
    /// reads as an empty notice.
    var row: TranscriptRow {
        guard let rows = chat?.transcript.rows else { return TranscriptRow(id: id, kind: .notice("")) }
        if index < rows.count, rows[index].id == id { return rows[index] }
        return rows.first { $0.id == id } ?? TranscriptRow(id: id, kind: .notice(""))
    }

    /// Note the row's new revision and redraw its views; false (and no redraw) when it is unchanged.
    func update(_ new: TranscriptRow) -> Bool {
        guard new.revision != revision else { return false }
        revision = new.revision
        objectWillChange.send()
        return true
    }

    func drew() { draws += 1 }
}

/// One terminal pane's Ghostty surface and the in-memory session feeding it the daemon's bytes.
@MainActor
final class PaneTerminal: ObservableObject {
    let term: String
    let view: TerminalViewState
    let session: InMemoryTerminalSession
    /// The daemon is streaming this terminal to this connection.
    @Published var attached = false
    /// The grid size last sent to the daemon.
    var size: (cols: Int, rows: Int)?
    /// The pane's laid-out size in points and the display scale, from SwiftUI.
    var layout: (size: CGSize, scale: CGFloat)?
    /// The surface's latest grid, held until `layout` says the surface has its final frame.
    var reported: InMemoryTerminalViewport?
    /// The most the pane exceeds its surface by on one axis, besides the cell remainder: the
    /// surface's padding, 20x16 pt measured.
    static let paddingPoints: CGFloat = 24

    init(term: String, controller: TerminalController, model: AppModel) {
        self.term = term
        view = TerminalViewState(controller: controller)
        // Only columns and rows reach the daemon, so a divider drag's pixel-only updates are noise.
        session = InMemoryTerminalSession(
            write: { [weak model] data in
                Task { @MainActor in model?.typed(term, data) }
            },
            resize: { [weak model] vp in
                Task { @MainActor in model?.resized(term, vp) }
            },
            suppressesPixelOnlyResizes: true
        )
        view.configuration = TerminalSurfaceOptions(backend: .inMemory(session))
    }

    /// The terminal's visible text, for the debug state endpoint.
    var viewportText: String? { session.readViewportText() }
}
