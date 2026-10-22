import Foundation
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
        case let .status(_, _, _, _, _, _, spaces, active, _, _, _), let .spaces(spaces, active):
            self.spaces = spaces
            self.active = active
            Log.info("layout: \(spaces.count) spaces, active \(active ?? "none"), panes \(activeSpace?.root.readingOrder.map(\.id) ?? [])")
        case let .paneUrl(pane, url):
            spaces = spaces.map { space in
                var space = space
                space.root = space.root.replacing(pane: pane, with: .browser(url: url))
                return space
            }
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
}
