import AppKit
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
    /// Whether the sidebar lists the spaces. Closed, the spaces are tabs above the
    /// grid instead. This window's own preference, kept across launches; the daemon holds no part of it.
    @Published private(set) var sidebarOpen: Bool = UserDefaults.standard.object(forKey: "sidebarOpen") as? Bool ?? true
    /// Ratios of dividers being dragged, drawn in place of the tree's. A divider still under the
    /// pointer keeps its entry through any broadcast; once its `split_ratio` is sent, the entry
    /// goes with the next broadcast (sent even when the value is unchanged) or a refusal.
    @Published private(set) var dragRatios: [String: Double] = [:]
    /// Dividers whose drag ended and whose `split_ratio` went out, waiting for the daemon.
    private var settling: Set<String> = []
    /// A pane being dragged by its header. The grid draws the tree as the drop would leave it;
    /// the daemon's tree changes only when the drop is sent, and `committed` holds the preview
    /// until the daemon's answer replaces it.
    struct PaneDrag: Equatable {
        let pane: String
        var drop: PaneDrop?
        var committed = false
    }
    @Published private(set) var paneDrag: PaneDrag?
    /// Each space's panes as the daemon's tree places them, which a drag is hit-tested against so the
    /// preview moving panes aside never moves the target under the pointer. Not published.
    var baseLayouts: [String: WheelKit.GridLayout] = [:]
    private var dragMonitor: Any?
    /// Escape cancelled the drag in progress; the gesture's remaining events and its end do nothing.
    private(set) var dragCancelled = false
    /// Every attached session by name: one per agent pane in the layout.
    @Published private(set) var chats: [String: SessionChat] = [:]
    /// Permission requests waiting on a person, from every session.
    @Published private(set) var perms: [PermInfo] = []
    /// Every agent the daemon knows, with whether its program was found.
    @Published private(set) var agents: [AgentInfo] = []
    /// The Settings window: wheel's settings for the chosen project, and the edits to them.
    let settings = SettingsModel()
    /// The usage strip and the Usage window: every coding agent's usage, from the daemon.
    let usage = UsageModel()
    private var usageTimer: Timer?
    /// The artifact palette open inside a pane, by pane id: at most one per pane, drawn over that
    /// pane alone.
    @Published var palettes: [String: PanePalette] = [:]
    /// The panes whose context meter shows the raw figures instead of the percent. A pane starts
    /// on the percent and returns to it when it closes.
    @Published var rawContextPanes: Set<String> = []
    /// `artifact_ls` requests, by request id, so the answer reaches the palette that asked.
    var paletteReqs: [UInt64: String] = [:]

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

    /// What a session's header says it is doing: a turn blocked on a permission request waits on
    /// the person, so it reads "Waiting on you" instead of the daemon's busy "Working".
    func stateLabel(_ chat: SessionChat) -> String {
        if case let .busy(queued) = chat.state, perms.contains(where: { $0.session == chat.name }) {
            return queued > 0 ? "Waiting on you · \(queued) queued" : "Waiting on you"
        }
        return chat.state?.label ?? "Not running"
    }

    /// Where a waiting request stands among all of them, counting from 1: the count the card
    /// reports.
    func position(of perm: PermInfo) -> Int {
        perms.firstIndex { $0.id == perm.id }.map { $0 + 1 } ?? perms.count
    }
    /// One Ghostty surface per terminal pane, keyed by the term id the tree names.
    private(set) var terminals: [String: PaneTerminal] = [:]
    /// One page per web pane, keyed by pane id.
    private(set) var webPanes: [String: WebPane] = [:]
    /// One page and its chrome per browser pane, keyed by pane id.
    private(set) var browserPanes: [String: BrowserPane] = [:]
    /// Where each pane on screen sits in the window, in SwiftUI's global space: what `render_pane`
    /// cuts out of the window for a pane that is not a web page. Not published, so a move redraws nothing.
    var paneFrames: [String: CGRect] = [:]
    /// Answers the web panes' requests from the daemon.
    private let pageDaemon = PageDaemon()
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
        client.log = { Log.info($0) }
        client.onConnection = { [weak self] state in
            self?.connection = state
            if state != .connected { self?.usage.disconnected() }
            if state != .connected { self?.terminals.values.forEach { $0.attached = false } }
        }
        client.onConnect = { [weak self] status in self?.connected(status) }
        client.onMessage = { [weak self] msg in self?.handle(msg) }
    }

    func start() {
        Log.info("app start, daemon \(endpoint.url.absoluteString)")
        service?.register()
        client.start()
        // The daemon reads at most once per its interval however often this asks.
        usageTimer = Timer.scheduledTimer(withTimeInterval: 300, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.askUsage(dueOnly: true) }
        }
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
        client.send(.renderOffer)
        settings.forgetRequests()
        // Answers to the last connection's palette listings never come.
        paletteReqs = [:]
        let known = Array(chats.values)
        if case let .status(_, _, _, uptime, _, perms, spaces, active, agents, shellIntegration, loginPath) = status {
            self.agents = agents
            settings.commandLine.shellProblem = shellIntegration
            settings.commandLine.loginPath = loginPath
            // The restarted daemon closes this connection; the client reconnects to the new one.
            if service?.restartIfOlder(uptimeSecs: uptime) == true { return }
            self.perms = perms
            apply(spaces, active)
        }
        // A palette open across a reconnect lists again, against the layout the new daemon sent.
        for pane in palettes.keys { askArtifacts(pane) }
        if settings.open { askSettings() }
        askUsage(dueOnly: true)
        // `apply` attached the sessions it added; the rest were attached on an earlier connection.
        for chat in known where chats[chat.name] === chat { attach(chat) }
        for terminal in terminals.values { attach(terminal) }
        Task {
            do {
                pageDaemon.token = try await endpoint.token()
                pageDaemon.url = endpoint.url
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
        rawContextPanes.formIntersection(held)
        paneFrames = paneFrames.filter { held.contains($0.key) }
        // A palette is for the pane as it was when it opened: a pane that left, or that something
        // else (an agent, the CLI) changed to another kind, closes it.
        let kinds = Dictionary(uniqueKeysWithValues: spaces.flatMap { $0.root.panes }.map { ($0.id, $0.kind.name) })
        for (pane, palette) in palettes where kinds[pane] != palette.openedOver {
            palettes[pane] = nil
            Log.info("palette \(pane): its pane \(kinds[pane] == nil ? "left the layout" : "became \(kinds[pane] ?? "")"); closed")
        }
        keepSettingsProject()
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
        let stores = WebPanes.stores(spaces)
        for (pane, web) in webPanes where artifacts[pane] != web.artifact {
            webPanes[pane] = nil
            Log.info("web pane \(pane) left the layout")
        }
        for (pane, artifact) in artifacts where webPanes[pane] == nil {
            guard let store = stores[pane] else { continue }
            let web = WebPane(pane: pane, artifact: artifact, store: store, daemon: pageDaemon)
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
        Log.info("spaces: \(spaces.map { "\($0.id)(\($0.root.panes.map { "\($0.id):\($0.kind.name)" }.joined(separator: ",")))" }.joined(separator: " ")) active=\(active ?? "none")")
        focusKey = plan.focusKey
        if let term = plan.focus { terminals[term]?.view.requestFocus() }
        return added
    }

    private func handle(_ msg: ServerMsg) {
        switch msg {
        case let .status(_, _, _, _, _, perms, spaces, active, agents, shellIntegration, loginPath):
            self.agents = agents
            settings.commandLine.shellProblem = shellIntegration
            settings.commandLine.loginPath = loginPath
            // An unsolicited status means this connection missed daemon-wide events.
            self.perms = perms
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
            if layoutReqs.remove(req) != nil { Log.info("created \(id)") }
        case let .started(req, pane, session):
            if layoutReqs.remove(req) != nil { Log.info("started \(pane) session \(session ?? "-")") }
        case let .paneView(req, pane, view, conversation):
            if layoutReqs.remove(req) != nil { Log.info("pane \(pane) view \(view?.rawValue ?? "-") conversation \(conversation ?? "-")") }
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
        case let .usage(_, board), let .usageChanged(board):
            usage.received(board)
            Log.info("usage board agents=\(board.agents.map { "\($0.id):shown=\($0.shown):readable=\($0.readable):accounts=\($0.accounts.map { "\($0.name)/\($0.usage.kind.rawValue)" }.joined(separator: "+"))" }.joined(separator: ",")) pins=\(board.pins.joined(separator: ","))")
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
            Log.info("session \(session) attached, \(events.count) events, \(config.count) config options, context \(context?.helpText ?? "unknown")")
        case let .conversation(req, session, id):
            sessionReqs[req] = nil
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

    /// Whether `pane` is the focused pane of the space on screen: the one a ⌘↩ answers for.
    func paneIsFocused(_ pane: String) -> Bool {
        spaces.first { $0.id == active }?.focus == pane
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

    func setSidebar(open: Bool) {
        guard open != sidebarOpen else { return }
        sidebarOpen = open
        UserDefaults.standard.set(open, forKey: "sidebarOpen")
        Log.info("sidebar \(open ? "opened" : "closed")")
    }

    func createSpace(cwd: String) { layout { .spaceCreate(req: $0, cwd: cwd) } }
    func closeSpace(_ id: String) { layout { .spaceClose(req: $0, id: id) } }
    func focus(space: String) { layout(clearsError: false) { .focus(req: $0, space: space, pane: nil) } }
    func focus(pane: String) { layout(clearsError: false) { .focus(req: $0, space: nil, pane: pane) } }
    func split(_ pane: String, _ dir: Dir) { layout { .paneSplit(req: $0, pane: pane, dir: dir) } }
    /// A click on a pane's context meter: flip that pane between the percent and the raw figures.
    func toggleContextLabel(_ pane: String) {
        if rawContextPanes.remove(pane) == nil { rawContextPanes.insert(pane) }
        Log.info("pane \(pane) context label: \(rawContextPanes.contains(pane) ? "raw" : "percent")")
    }

    func closePane(_ pane: String) { layout { .paneClose(req: $0, pane: pane) } }
    func setKind(_ pane: String, _ kind: NewPaneKind, artifact: String? = nil, scope: ArtifactScope? = nil) {
        layout { .paneSetKind(req: $0, pane: pane, kind: kind, artifact: artifact, scope: scope) }
    }
    /// Show the pane's Claude Code conversation as `view`; a refusal shows above the grid.
    func switchView(_ pane: String, to view: ConversationView) {
        layout { .paneView(req: $0, pane: pane, to: view) }
    }
    func startPane(_ pane: String, _ kind: StartKind, message: String? = nil, agent: String? = nil) {
        layout { .paneStart(req: $0, pane: pane, kind: kind, message: message, agent: agent) }
    }
    /// Whether the daemon found the program of agent `name`.
    func agentFound(_ name: String) -> Bool { agents.contains { $0.name == name && $0.found } }
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

    // MARK: Pane drag — a header drag swaps a pane or docks it beside another.

    /// Start dragging `pane`; Escape cancels until the pointer is released.
    /// The gesture calls this on every move; after Escape it does nothing until the pointer is
    /// released. `fresh` (the debug endpoint, which has no gesture) forgets an earlier Escape.
    func beginPaneDrag(_ pane: String, fresh: Bool = false) {
        if fresh { dragCancelled = false }
        guard paneDrag == nil, !dragCancelled, connection == .connected, space(holding: pane) != nil else { return }
        paneDrag = PaneDrag(pane: pane)
        dragMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard event.keyCode == 53, let self, self.paneDrag != nil else { return event }
            MainActor.assumeIsolated { self.cancelPaneDrag(escape: true) }
            return nil
        }
        Log.info("pane drag \(pane): started")
    }

    /// The pointer is over `drop`, or over no other pane. The layout animates to show the result.
    func hoverPaneDrag(_ drop: PaneDrop?) {
        guard var drag = paneDrag, !drag.committed, drag.drop != drop else { return }
        if drop?.target == drag.pane { return hoverPaneDrag(nil) }
        drag.drop = drop
        withAnimation(.smooth(duration: 0.22)) { paneDrag = drag }
        Log.info("pane drag \(drag.pane): over \(drop.map { "\($0.target) \($0.zone.label)" } ?? "nothing")")
    }

    /// Pointer position in the grid's space: hover whatever pane is there.
    func dragPane(to point: CGPoint) {
        guard let drag = paneDrag, !dragCancelled, let id = space(holding: drag.pane)?.id, let base = baseLayouts[id] else { return }
        hoverPaneDrag(PaneDragging.drop(at: point, among: base.panes, dragging: drag.pane))
    }

    /// Release: send the drop the pointer is over, or cancel when it is over none.
    func endPaneDrag() {
        let cancelled = dragCancelled
        dragCancelled = false
        guard var drag = paneDrag, !drag.committed else { return }
        removeDragMonitor()
        guard !cancelled, let drop = drag.drop, connection == .connected else {
            withAnimation(.smooth(duration: 0.22)) { paneDrag = nil }
            Log.info("pane drag \(drag.pane): cancelled")
            return
        }
        drag.committed = true
        paneDrag = drag
        Log.info("pane drag \(drag.pane): dropped on \(drop.target) \(drop.zone.label)")
        switch drop.zone {
        case .center: layout { .paneSwap(req: $0, pane: drag.pane, with: drop.target) }
        case let .edge(dir): layout { .paneMove(req: $0, pane: drag.pane, target: drop.target, dir: dir) }
        }
    }

    func cancelPaneDrag(escape: Bool = false) {
        guard let drag = paneDrag, !drag.committed else { return }
        if escape { dragCancelled = true }
        removeDragMonitor()
        withAnimation(.smooth(duration: 0.22)) { paneDrag = nil }
        Log.info("pane drag \(drag.pane): cancelled\(escape ? " by escape" : "")")
    }

    private func removeDragMonitor() {
        if let dragMonitor { NSEvent.removeMonitor(dragMonitor) }
        dragMonitor = nil
    }

    /// The tree `space` is drawn with: the daemon's, or what the drop under the pointer would make of it.
    func shownRoot(of space: SpaceInfo) -> Node {
        guard let drag = paneDrag, let drop = drag.drop, space.root.panes.contains(where: { $0.id == drag.pane }) else { return space.root }
        return space.root.applying(drop, dragging: drag.pane)
    }

    private func space(holding pane: String) -> SpaceInfo? {
        spaces.first { $0.root.panes.contains { $0.id == pane } }
    }

    /// The daemon answered: drop the overrides of every divider whose ratio it has heard.
    private func settle() {
        if paneDrag?.committed == true { withAnimation(.smooth(duration: 0.22)) { paneDrag = nil } }
        for split in settling { dragRatios[split] = nil }
        settling = []
    }

    /// A terminal or an agent pane took keyboard focus (the person clicked into it): tell the
    /// daemon, unless it already has that pane focused.
    func paneFocused(pane: String) {
        guard let space = space(holding: pane), space.focus != pane || space.id != active else { return }
        select(pane: pane)
    }

    /// Show a pane's space, focus the pane, and move the keyboard to it: a click in the pane.
    func select(pane: String) {
        guard space(holding: pane) != nil else { return }
        focusKey = SurfacePlan.key(pane: pane, in: spaces)
        focus(pane: pane)
    }

    // MARK: Sessions — every agent pane

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

    /// Ask the daemon for the usage board; it answers `usage`. `dueOnly` reads the endpoint only
    /// when the last read is older than the daemon's interval.
    func askUsage(dueOnly: Bool) {
        guard connection == .connected else { return }
        if !dueOnly { usage.refreshStarted() }
        Log.info("usage ask dueOnly=\(dueOnly)")
        client.send(.usage(req: client.req(), dueOnly: dueOnly))
    }

    /// Pin or unpin a usage row (`<agent>:<limit>`) on the strip: write the `usage.pins` setting
    /// machine-wide; the daemon's `usageChanged` brings the new board back.
    func togglePin(_ row: String) {
        guard connection == .connected else { return }
        let value = usage.board.pins(toggling: row)
        Log.info("usage pin \(row) -> usage.pins = \(value)")
        client.send(.settingSet(req: nil, key: "usage.pins", value: value, space: nil, machine: true))
    }

    /// Show or hide an agent's usage: write the `usage.agents` setting machine-wide.
    func toggleUsageAgent(_ agent: String) {
        guard connection == .connected else { return }
        let value = usage.board.agentsSetting(toggling: agent)
        Log.info("usage agent \(agent) -> usage.agents = \(value)")
        client.send(.settingSet(req: nil, key: "usage.agents", value: value, space: nil, machine: true))
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
        settings.commandLine.refresh()
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
        guard pageDaemon.token != nil, let url = WebPanes.pageURL(pane: web.pane) else { return }
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

/// One attached session's conversation: an agent pane's. Each is its own
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
    /// The selection that spans this transcript's rows.
    let selection = TranscriptSelection()
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

    /// The agent's own categories say what an option is for, so the row can order them without this
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
