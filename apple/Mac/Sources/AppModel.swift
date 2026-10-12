import Foundation
import GhosttyTerminal
import SwiftUI
import WheelKit

/// Everything the window shows, fed by one connection to wheeld. The daemon holds the real
/// state (ADR-0001): the spaces and their split trees arrive whole in each `spaces` broadcast,
/// and this model changes them only by sending ops. Quitting and relaunching loses nothing.
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
    @Published private(set) var leadState: LeadState?
    @Published private(set) var transcript = Transcript()
    /// Permission requests waiting on a person, from every session.
    @Published private(set) var perms: [PermInfo] = []
    /// The last thing the daemon refused, shown under the composer until the next send.
    @Published private(set) var leadError: String?

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
    /// Requests from the lead composer, so only their failures show under it.
    private var leadReqs: Set<UInt64> = []
    /// Layout ops, so only their failures show above the grid.
    private var layoutReqs: Set<UInt64> = []
    /// `<pane>/<term>` of the focus this window last acted on, so a broadcast that leaves it
    /// unchanged does not pull the keyboard back from wherever the person put it.
    private(set) var focusKey: String?
    /// The daemon token, which also guards the debug endpoint.
    let tokenFile: URL

    init() {
        let env = ProcessInfo.processInfo.environment
        let url = env["WHEEL_URL"].flatMap(URL.init(string:)) ?? URL(string: "ws://127.0.0.1:7420/ws")!
        tokenFile = env["WHEEL_TOKEN_FILE"].map { URL(filePath: $0) }
            ?? FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Application Support/wheel/token")
        let endpoint = DaemonEndpoint(url: url, token: DaemonEndpoint.tokenFile(tokenFile))
        self.endpoint = endpoint
        client = DaemonClient(endpoint: endpoint)
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
        client.start()
    }

    var activeSpace: SpaceInfo? { spaces.first { $0.id == active } }

    /// The daemon's status at connect carries the whole layout; every terminal pane reattaches.
    private func connected(_ status: ServerMsg) {
        client.send(.leadAttach(req: nil))
        guard case let .status(_, _, _, _, lead, perms, spaces, active) = status else { return }
        leadState = lead
        self.perms = perms
        apply(spaces, active)
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
        let folders = WebPanes.folders(spaces)
        for (pane, web) in webPanes where folders[pane] != web.dir {
            webPanes[pane] = nil
            Log.info("web pane \(pane) left the layout")
        }
        for (pane, dir) in folders where webPanes[pane] == nil {
            let web = WebPane(pane: pane, dir: dir)
            webPanes[pane] = web
            open(web)
        }
        Log.info("spaces: \(spaces.map { "\($0.id)(\($0.root.panes.map { "\($0.id):\($0.kind.name)" }.joined(separator: ",")))" }.joined(separator: " ")) active=\(active ?? "none")")
        focusKey = plan.focusKey
        if let term = plan.focus { terminals[term]?.view.requestFocus() }
        return added
    }

    private func handle(_ msg: ServerMsg) {
        switch msg {
        case let .status(_, _, _, _, lead, perms, spaces, active):
            // An unsolicited status means this connection missed daemon-wide events.
            leadState = lead
            self.perms = perms
            apply(spaces, active).forEach(attach)
        case let .spaces(spaces, active):
            apply(spaces, active).forEach(attach)
        case let .paneReload(pane):
            webPanes[pane]?.reload()
        case let .created(req, id):
            if layoutReqs.remove(req) != nil { Log.info("created \(id)") }
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
        case let .leadStateChanged(state):
            leadState = state
        case let .leadAttached(_, state, events):
            leadState = state
            transcript.reset(events)
            Log.info("lead attached, \(events.count) events")
        case let .lead(event):
            transcript.apply(event)
        case let .permRequested(perm):
            if !perms.contains(where: { $0.id == perm.id }) { perms.append(perm) }
            Log.info("permission \(perm.id) waiting: \(perm.title)")
        case let .permResolved(id, choice):
            perms.removeAll { $0.id == id }
            Log.info("permission \(id) resolved: \(choice)")
        case let .leadQueued(req, _):
            leadReqs.remove(req)
        case let .ok(req):
            layoutReqs.remove(req)
        case let .error(req, message):
            if let req, leadReqs.remove(req) != nil { leadError = message }
            if let req, layoutReqs.remove(req) != nil {
                layoutError = message
                settle()
            }
            Log.info("daemon error: \(message)")
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
    func setKind(_ pane: String, _ kind: NewPaneKind, dir: String? = nil) {
        layout { .paneSetKind(req: $0, pane: pane, kind: kind, dir: dir) }
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

    /// A terminal took keyboard focus (the person clicked into it): tell the daemon, unless it
    /// already has that pane focused.
    func terminalFocused(pane: String) {
        guard let space = spaces.first(where: { $0.root.panes.contains { $0.id == pane } }),
              space.focus != pane || space.id != active else { return }
        focusKey = SurfacePlan.key(pane: pane, in: spaces)
        focus(pane: pane)
    }

    // MARK: Lead

    /// Give the lead a message. Returns false, sending nothing, when it can't go out now; the
    /// composer keeps the draft in that case.
    @discardableResult
    func sendToLead(_ text: String) -> Bool {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return false }
        guard connection == .connected else {
            leadError = "Not connected to wheeld; your message is still in the box."
            return false
        }
        leadError = nil
        let req = client.req()
        leadReqs.insert(req)
        Log.info("lead send, \(text.count) chars")
        client.send(.leadSend(req: req, text: text))
        return true
    }

    func cancelLead() {
        client.send(.leadCancel(req: client.req()))
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
