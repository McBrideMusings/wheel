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

    /// Tool-call ids of the lead's transcript rows the lazy stack has built and not released: the
    /// rows in or near the viewport. A row scrolled well away is absent from this set.
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

    func scrollLead(toTop: Bool) {
        leadScroll = LeadScroll(serial: leadScroll.serial + 1, toTop: toTop)
    }

    /// The waiting request that gates this tool call of this session, if any, and its row is on
    /// screen. Only the lead's transcript draws one: it is always on screen, where an agent pane may
    /// be closed or hidden.
    func perm(session: String, toolCall: String) -> PermInfo? {
        guard session == Self.lead, visibleLeadCalls.contains(toolCall) else { return nil }
        return perms.first { $0.session == session && $0.toolCall == toolCall }
    }

    /// Requests the Waiting box above the composer shows: every request the transcript does not
    /// draw on a row — an agent's, a `type_into_pane`, and a lead request whose row is off screen.
    var boxPerms: [PermInfo] {
        perms.filter { perm in
            guard perm.session == Self.lead, let call = perm.toolCall else { return true }
            return !visibleLeadCalls.contains(call)
        }
    }

    /// One Ghostty surface per terminal pane, keyed by the term id the tree names.
    private(set) var terminals: [String: PaneTerminal] = [:]
    /// One page per web pane, keyed by pane id.
    private(set) var webPanes: [String: WebPane] = [:]
    /// The daemon token, for the web panes' page addresses; read at each connect.
    private var pageToken: String?
    /// Ghostty's default light/dark theme pair, shared by every pane's surface, with padding
    /// inside the terminal's own background.
    private let controller = TerminalController {
        $0.withWindowPaddingX(10)
        $0.withWindowPaddingY(8)
    }
    let endpoint: DaemonEndpoint
    private let client: DaemonClient
    /// Session requests (a composer's send, an attach), by session, so a failure shows under
    /// that session's composer.
    private var sessionReqs: [UInt64: String] = [:]
    /// Layout ops, so only their failures show above the grid.
    private var layoutReqs: Set<UInt64> = []
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

    /// The daemon's status at connect carries the whole layout; every terminal pane and every
    /// session reattaches.
    private func connected(_ status: ServerMsg) {
        // Answers to the last connection's requests never come.
        sessionReqs = [:]
        let known = Array(chats.values)
        if case let .status(_, _, _, uptime, _, leadState, perms, spaces, active) = status {
            // The restarted daemon closes this connection; the client reconnects to the new one.
            if service?.restartIfOlder(uptimeSecs: uptime) == true { return }
            lead.state = leadState
            self.perms = perms
            apply(spaces, active)
        }
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
        let sessions = Set(AgentPanes.sessions(spaces).values)
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
        Log.info("spaces: \(spaces.map { "\($0.id)(\($0.root.panes.map { "\($0.id):\($0.kind.name)" }.joined(separator: ",")))" }.joined(separator: " ")) active=\(active ?? "none")")
        focusKey = plan.focusKey
        if let term = plan.focus { terminals[term]?.view.requestFocus() }
        return added
    }

    private func handle(_ msg: ServerMsg) {
        switch msg {
        case let .status(_, _, _, _, _, leadState, perms, spaces, active):
            // An unsolicited status means this connection missed daemon-wide events.
            lead.state = leadState
            self.perms = perms
            apply(spaces, active).forEach(attach)
        case let .spaces(spaces, active):
            apply(spaces, active).forEach(attach)
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
        case let .created(req, id):
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
            terminal.session.receive(Data("\r\n[shell exited — choose Terminal from this pane's menu for a new one]\r\n".utf8))
        case let .sessionStateChanged(session, state):
            chats[session]?.state = state
        case let .sessionAttached(req, session, state, events, config):
            if let req { sessionReqs[req] = nil }
            guard let chat = chats[session] else { return }
            chat.state = state
            chat.reset(events)
            chat.setConfig(config)
            chat.error = nil
            Log.info("session \(session) attached, \(events.count) events, \(config.count) config options")
        case let .sessionConfig(session, options):
            guard let chat = chats[session] else { return }
            chat.setConfig(options)
            Log.info("session \(session) config: \(options.map { "\($0.id)=\($0.currentLabel ?? "?")" }.joined(separator: " "))")
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
        case let .ok(req):
            layoutReqs.remove(req)
        case let .error(req, term, message):
            if let req, let session = sessionReqs.removeValue(forKey: req) { chats[session]?.error = message }
            if let req, layoutReqs.remove(req) != nil {
                layoutError = message
                settle()
            }
            Log.info("daemon error\(term.map { " on \($0)" } ?? ""): \(message)")
        default:
            break
        }
    }

    // MARK: Layout ops — the tab bar, pane menus, dividers and the debug endpoint all call these.

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

    /// Allow or deny a waiting request; the daemon broadcasts the result to every client.
    func answer(_ perm: PermInfo, _ choice: String) {
        Log.info("answering \(perm.id): \(choice)")
        client.send(.permAnswer(req: client.req(), id: perm.id, choice: choice))
    }

    // MARK: Web panes

    /// Load a web pane's page, once the token for its address is known.
    private func open(_ web: WebPane) {
        guard let token = pageToken, let url = WebPanes.url(daemon: endpoint.url, pane: web.pane, token: token) else { return }
        web.load(url)
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

    fileprivate func resized(_ term: String, cols: Int, rows: Int) {
        guard cols > 0, rows > 0, let terminal = terminals[term] else { return }
        if let size = terminal.size, size == (cols, rows) { return }
        terminal.size = (cols, rows)
        client.send(.termResize(req: nil, id: term, cols: cols, rows: rows))
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
    /// How many times `rows` was reassigned and how many row objects were told to redraw, for the
    /// state query: a streamed chunk adds one to `rowUpdates` and nothing to `listUpdates`.
    private(set) var listUpdates = 0
    private(set) var rowUpdates = 0
    /// The last thing the daemon refused for this session, shown under its composer until the
    /// next send or attach.
    @Published var error: String?
    /// The text in this session's composer, not yet sent.
    @Published var draft = ""
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

    func reset(_ events: [SessionEvent]) {
        transcript.reset(events)
        syncRows()
    }

    func apply(_ event: SessionEvent) {
        transcript.apply(event)
        syncRows()
    }

    /// Bring `rows` in line with the transcript, keeping each row's object by its id.
    private func syncRows() {
        let source = transcript.rows
        if source.count == rows.count, zip(source, rows).allSatisfy({ $0.id == $1.id }) {
            for (row, model) in zip(source, rows) where model.update(row) { rowUpdates += 1 }
            return
        }
        let existing = Dictionary(rows.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        rows = source.map { row in
            if let model = existing[row.id] {
                if model.update(row) { rowUpdates += 1 }
                return model
            }
            return RowModel(row)
        }
        listUpdates += 1
    }
}

/// One transcript row as a view observes it. Its views redraw when this row changes, not when
/// another row does.
@MainActor
final class RowModel: ObservableObject, Identifiable {
    let id: Int
    @Published private(set) var row: TranscriptRow
    /// Times a view built this row's body, for the state query.
    private(set) var draws = 0

    init(_ row: TranscriptRow) {
        id = row.id
        self.row = row
    }

    /// Take the row's new contents; false (and no redraw) when nothing changed.
    func update(_ new: TranscriptRow) -> Bool {
        guard new != row else { return false }
        row = new
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
    /// The grid size the surface last reported.
    var size: (cols: Int, rows: Int)?

    init(term: String, controller: TerminalController, model: AppModel) {
        self.term = term
        view = TerminalViewState(controller: controller)
        // Only columns and rows reach the daemon, so a divider drag's pixel-only updates are noise.
        session = InMemoryTerminalSession(
            write: { [weak model] data in
                Task { @MainActor in model?.typed(term, data) }
            },
            resize: { [weak model] vp in
                Task { @MainActor in model?.resized(term, cols: Int(vp.columns), rows: Int(vp.rows)) }
            },
            suppressesPixelOnlyResizes: true
        )
        view.configuration = TerminalSurfaceOptions(backend: .inMemory(session))
    }

    /// The terminal's visible text, for the debug state endpoint.
    var viewportText: String? { session.readViewportText() }
}
