import Foundation

/// Where a client finds wheeld, and how it gets the token. The token comes from a provider the
/// app supplies: a file on the Mac running the daemon, the Keychain after pairing on iOS.
public struct DaemonEndpoint: Sendable {
    public var url: URL
    public var token: @Sendable () async throws -> String
    /// Set while the token is a pairing code's one-time token: the device's name, sent with it so
    /// the daemon can spend the token and answer with the device's own credential (ADR-0015).
    public var pairingAs: String?

    public init(url: URL, token: @escaping @Sendable () async throws -> String, pairingAs: String? = nil) {
        self.url = url
        self.token = token
        self.pairingAs = pairingAs
    }

    /// Whether the daemon's address is a loopback one: `localhost`, `::1` or an IPv4 address in
    /// 127.0.0.0/8. A file page names a path on the daemon's machine, so only a loopback daemon's
    /// file pages are files this client has; any other address counts as another machine.
    public var isLoopback: Bool {
        guard let host = url.host(percentEncoded: false)?.lowercased() else { return false }
        if host == "localhost" || host == "::1" { return true }
        let octets = host.split(separator: ".", omittingEmptySubsequences: false)
        return octets.count == 4 && octets.allSatisfy { UInt8($0) != nil } && octets[0] == "127"
    }

    /// A token read from a file, as the daemon writes it.
    public static func tokenFile(_ file: URL) -> @Sendable () async throws -> String {
        { try String(contentsOf: file, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines) }
    }
}

/// The part of a WebSocket task `DaemonClient` uses, so a test can stand in a fake for the network.
public protocol DaemonSocket: AnyObject, Sendable {
    var maximumMessageSize: Int { get set }
    func resume()
    func send(_ message: URLSessionWebSocketTask.Message, completionHandler: @escaping @Sendable (Error?) -> Void)
    func receive(completionHandler: @escaping @Sendable (Result<URLSessionWebSocketTask.Message, Error>) -> Void)
    func cancel(with closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?)
}

extension URLSessionWebSocketTask: DaemonSocket {}

/// One WebSocket connection to wheeld that reconnects on its own. Every event arrives on the main
/// actor through `onMessage`; `onConnect` runs after each successful (re)connect, which is where a
/// client re-attaches to what it shows.
@MainActor
public final class DaemonClient {
    public enum Connection: Equatable, Sendable {
        case connecting
        case connected
        case unreachable(String)
    }

    public private(set) var connection: Connection = .connecting {
        didSet { if connection != oldValue { onConnection?(connection) } }
    }
    public var onMessage: ((ServerMsg) -> Void)?
    /// Runs after each successful (re)connect with the daemon's status at that moment.
    public var onConnect: ((ServerMsg) -> Void)?
    public var onConnection: ((Connection) -> Void)?
    public var log: ((String) -> Void)?
    /// The app's log file, whose lines this client sends to wheeld while connected.
    public var appLog: AppLog? {
        didSet {
            appLog?.onWrite = { [weak self] in Task { @MainActor in self?.logsWritten() } }
            shipLogs()
        }
    }

    /// Where this client connects; a phone replaces its one-time token with the credential it is given.
    public var endpoint: DaemonEndpoint
    private let makeSocket: (URL) -> any DaemonSocket
    private let retryDelay: Duration
    private var task: (any DaemonSocket)?
    private var nextReq: UInt64 = 0
    private var generation = 0
    /// The `log` request wheeld has not answered yet, and where the log stands once it does.
    private var logInFlight: (req: UInt64, end: AppLog.Position)?
    /// A batch is being read off the log's queue.
    private var logReading = false
    private var logShipQueued = false
    /// Set when wheeld refused a batch; nothing more is sent until the next connection.
    private var logRefused = false

    /// `makeSocket` opens the socket for a URL (not yet resumed); `retryDelay` is how long a failed
    /// connection waits before the next attempt.
    public init(endpoint: DaemonEndpoint, retryDelay: Duration = .seconds(2), makeSocket: @escaping (URL) -> any DaemonSocket) {
        self.endpoint = endpoint
        self.retryDelay = retryDelay
        self.makeSocket = makeSocket
    }

    public convenience init(endpoint: DaemonEndpoint, session: URLSession = .shared) {
        self.init(endpoint: endpoint, makeSocket: { session.webSocketTask(with: $0) })
    }

    public func start() {
        connect()
    }

    /// Closes the socket and stops reconnecting, for a client that was unpaired or whose
    /// certificate did not match.
    public func stop() {
        generation += 1
        logInFlight = nil
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
    }

    /// A fresh request id.
    public func req() -> UInt64 {
        nextReq += 1
        return nextReq
    }

    public func send(_ msg: ClientMsg) {
        guard let task, connection == .connected else { return }
        do {
            let text = try msg.json()
            task.send(.string(text)) { [weak self] error in
                if let error {
                    Task { @MainActor in self?.log?("send failed: \(error.localizedDescription)") }
                }
            }
        } catch {
            log?("encode failed: \(error)")
        }
    }

    private func connect() {
        generation += 1
        let gen = generation
        logInFlight = nil
        logRefused = false
        connection = .connecting
        Task { @MainActor in
            let token: String
            do {
                token = try await endpoint.token()
            } catch {
                fail(gen, "no daemon token: \(error.localizedDescription)")
                return
            }
            guard gen == generation else { return }
            var components = URLComponents(url: endpoint.url, resolvingAgainstBaseURL: false)!
            if let name = endpoint.pairingAs {
                components.queryItems = [URLQueryItem(name: "pair", value: token), URLQueryItem(name: "name", value: name)]
            } else {
                components.queryItems = [URLQueryItem(name: "token", value: token)]
            }
            let task = makeSocket(components.url!)
            task.maximumMessageSize = 64 << 20
            self.task = task
            task.resume()
            // The first status reply proves the socket and token are good.
            let probe = req()
            do {
                task.send(.string(try ClientMsg.status(req: probe).json())) { _ in }
            } catch {
                fail(gen, "encode failed: \(error)")
                return
            }
            receive(task, gen: gen, probe: probe)
        }
    }

    private func receive(_ task: any DaemonSocket, gen: Int, probe: UInt64) {
        task.receive { [weak self] result in
            Task { @MainActor in
                guard let self, gen == self.generation else { return }
                switch result {
                case let .failure(error):
                    let detail = error as NSError
                    self.log?("socket error: domain=\(detail.domain) code=\(detail.code)")
                    self.fail(gen, error.localizedDescription)
                case let .success(message):
                    if case let .string(text) = message {
                        do {
                            let msg = try ServerMsg.decode(text)
                            if self.connection != .connected, case let .status(req, _, _, _, _, _, _, _, _, _, _, _) = msg, req == probe {
                                self.connection = .connected
                                self.log?("connected to \(self.endpoint.url.absoluteString)")
                                self.onConnect?(msg)
                                self.shipLogs()
                                self.receive(task, gen: gen, probe: probe)
                                return
                            }
                            if self.logAnswered(msg) { self.receive(task, gen: gen, probe: probe); return }
                            self.onMessage?(msg)
                        } catch {
                            self.log?("bad frame: \(error)")
                        }
                    }
                    self.receive(task, gen: gen, probe: probe)
                }
            }
        }
    }

    /// A line was written: send it within a second, with whatever else is written by then.
    private func logsWritten() {
        guard !logShipQueued else { return }
        logShipQueued = true
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1))
            self?.logShipQueued = false
            self?.shipLogs()
        }
    }

    /// Sends the oldest batch wheeld has not accepted, one request at a time.
    private func shipLogs() {
        guard connection == .connected, logInFlight == nil, !logReading, !logRefused, let appLog else { return }
        logReading = true
        let gen = generation
        appLog.pending { batch in
            Task { @MainActor [weak self] in
                guard let self else { return }
                logReading = false
                guard gen == generation, connection == .connected, let batch else { return }
                let id = req()
                logInFlight = (id, batch.end)
                send(.log(req: id, client: appLog.client, lines: batch.lines))
            }
        }
    }

    /// Whether `msg` answers the `log` request in flight. Neither answer is logged on success, since
    /// that line would itself be sent.
    private func logAnswered(_ msg: ServerMsg) -> Bool {
        guard let inFlight = logInFlight else { return false }
        switch msg {
        case let .ok(req) where req == inFlight.req:
            logInFlight = nil
            appLog?.acknowledge(inFlight.end)
            shipLogs()
            return true
        case let .error(req, _, message) where req == inFlight.req:
            logInFlight = nil
            logRefused = true
            log?("wheeld refused this app's log lines: \(message)")
            return true
        default:
            return false
        }
    }

    private func fail(_ gen: Int, _ reason: String) {
        guard gen == generation else { return }
        logInFlight = nil
        if connection != .unreachable(reason) { log?("daemon unreachable: \(reason)") }
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        connection = .unreachable(reason)
        let delay = retryDelay
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: delay)
            guard let self, gen == self.generation else { return }
            self.connect()
        }
    }
}
