import Foundation
import GhosttyTerminal
import SwiftUI
import WheelKit

/// Everything the window shows, fed by one connection to wheeld. The daemon holds the real
/// state (ADR-0001); this model only mirrors it, so quitting and relaunching loses nothing.
@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var connection: DaemonClient.Connection = .connecting
    @Published private(set) var termId: String?
    @Published private(set) var attached = false
    @Published private(set) var leadState: LeadState?
    @Published private(set) var transcript = Transcript()
    /// Permission requests waiting on a person, from every session.
    @Published private(set) var perms: [PermInfo] = []
    /// The last thing the daemon refused, shown under the composer until the next send.
    @Published private(set) var leadError: String?

    /// Ghostty's default light/dark theme pair, so the terminal follows the app's appearance,
    /// with padding inside the terminal's own background.
    let terminal = TerminalViewState(controller: TerminalController {
        $0.withWindowPaddingX(10)
        $0.withWindowPaddingY(8)
    })
    private(set) var session: InMemoryTerminalSession!
    let endpoint: DaemonEndpoint
    private let client: DaemonClient
    private var openReq: UInt64?
    /// Requests from the lead composer, so only their failures show under it.
    private var leadReqs: Set<UInt64> = []
    /// The daemon token, which also guards the debug endpoint.
    let tokenFile: URL
    private var viewport = (cols: 100, rows: 30)

    /// The terminal this window shows, remembered across launches so a relaunch reattaches.
    private var storedTermId: String? {
        get { UserDefaults.standard.string(forKey: "termId") }
        set { UserDefaults.standard.set(newValue, forKey: "termId") }
    }

    init() {
        let env = ProcessInfo.processInfo.environment
        let url = env["WHEEL_URL"].flatMap(URL.init(string:)) ?? URL(string: "ws://127.0.0.1:7420/ws")!
        tokenFile = env["WHEEL_TOKEN_FILE"].map { URL(filePath: $0) }
            ?? FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Application Support/wheel/token")
        let endpoint = DaemonEndpoint(url: url, token: DaemonEndpoint.tokenFile(tokenFile))
        self.endpoint = endpoint
        client = DaemonClient(endpoint: endpoint)
        session = InMemoryTerminalSession(
            write: { [weak self] data in
                Task { @MainActor in self?.typed(data) }
            },
            resize: { [weak self] vp in
                Task { @MainActor in self?.resized(cols: Int(vp.columns), rows: Int(vp.rows)) }
            }
        )
        client.log = { Log.info($0) }
        client.onConnection = { [weak self] state in
            self?.connection = state
            if state != .connected { self?.attached = false }
        }
        client.onConnect = { [weak self] status in self?.connected(status) }
        client.onMessage = { [weak self] msg in self?.handle(msg) }
    }

    func start() {
        Log.info("app start, daemon \(endpoint.url.absoluteString)")
        client.start()
    }

    /// The terminal's visible text, for the debug state endpoint.
    var viewportText: String? { session.readViewportText() }

    /// The daemon's status at connect decides between reattaching and opening a terminal.
    private func connected(_ status: ServerMsg) {
        client.send(.leadAttach(req: nil))
        guard case let .status(_, _, _, terms, lead, perms) = status else { return }
        leadState = lead
        self.perms = perms
        if let id = storedTermId, terms.contains(where: { $0.id == id && $0.alive }) {
                Log.info("reattaching to \(id)")
                attach(id)
            } else {
                let req = client.req()
                openReq = req
                Log.info("opening a terminal (stored id \(storedTermId ?? "none") is gone)")
                client.send(.termOpen(req: req, cwd: FileManager.default.homeDirectoryForCurrentUser.path, cols: viewport.cols, rows: viewport.rows))
            }
    }

    private func handle(_ msg: ServerMsg) {
        switch msg {
        case let .status(_, _, _, _, lead, perms):
            // An unsolicited status means this connection missed daemon-wide events.
            leadState = lead
            self.perms = perms
        case let .termOpened(req, id, pid):
            guard req == openReq else { return }
            openReq = nil
            Log.info("terminal \(id) opened, pid \(pid.map(String.init) ?? "?")")
            storedTermId = id
            attach(id)
        case let .termAttached(_, id, replayBytes):
            guard id == termId else { return }
            // Every attach (and every resync) replays from scratch: reset the screen first.
            session.receive(Data("\u{1b}c".utf8))
            attached = true
            Log.info("attached to \(id), replay \(replayBytes) bytes")
        case let .termOutput(id, data):
            if id == termId { session.receive(data) }
        case let .termExited(id, code):
            guard id == termId else { return }
            Log.info("terminal \(id) exited with \(code.map(String.init) ?? "?")")
            session.receive(Data("\r\n[shell exited — relaunch Wheel for a new one]\r\n".utf8))
            storedTermId = nil
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
        case let .error(req, message):
            if let req, leadReqs.remove(req) != nil { leadError = message }
            Log.info("daemon error: \(message)")
        default:
            break
        }
    }

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

    private func attach(_ id: String) {
        termId = id
        client.send(.termAttach(req: client.req(), id: id))
        client.send(.termResize(req: nil, id: id, cols: viewport.cols, rows: viewport.rows))
    }

    private func typed(_ data: Data) {
        guard attached, let termId else { return }
        client.send(.termInput(req: nil, id: termId, data: data))
    }

    private func resized(cols: Int, rows: Int) {
        guard cols > 0, rows > 0, (cols, rows) != viewport else { return }
        viewport = (cols, rows)
        if attached, let termId { client.send(.termResize(req: nil, id: termId, cols: cols, rows: rows)) }
    }
}
