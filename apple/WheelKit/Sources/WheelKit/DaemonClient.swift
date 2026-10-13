import Foundation

/// Where a client finds wheeld, and how it gets the token. The token comes from a provider the
/// app supplies: a file on the Mac running the daemon, the Keychain after pairing on iOS.
public struct DaemonEndpoint: Sendable {
    public var url: URL
    public var token: @Sendable () async throws -> String

    public init(url: URL, token: @escaping @Sendable () async throws -> String) {
        self.url = url
        self.token = token
    }

    /// A token read from a file, as the daemon writes it.
    public static func tokenFile(_ file: URL) -> @Sendable () async throws -> String {
        { try String(contentsOf: file, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines) }
    }
}

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

    private let endpoint: DaemonEndpoint
    private let session: URLSession
    private var task: URLSessionWebSocketTask?
    private var nextReq: UInt64 = 0
    private var generation = 0

    public init(endpoint: DaemonEndpoint, session: URLSession = .shared) {
        self.endpoint = endpoint
        self.session = session
    }

    public func start() {
        connect()
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
            components.queryItems = [URLQueryItem(name: "token", value: token)]
            let task = session.webSocketTask(with: components.url!)
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

    private func receive(_ task: URLSessionWebSocketTask, gen: Int, probe: UInt64) {
        task.receive { [weak self] result in
            Task { @MainActor in
                guard let self, gen == self.generation else { return }
                switch result {
                case let .failure(error):
                    self.fail(gen, error.localizedDescription)
                case let .success(message):
                    if case let .string(text) = message {
                        do {
                            let msg = try ServerMsg.decode(text)
                            if self.connection != .connected, case let .status(req, _, _, _, _, _, _, _, _) = msg, req == probe {
                                self.connection = .connected
                                self.log?("connected to \(self.endpoint.url.absoluteString)")
                                self.onConnect?(msg)
                                self.receive(task, gen: gen, probe: probe)
                                return
                            }
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

    private func fail(_ gen: Int, _ reason: String) {
        guard gen == generation else { return }
        if connection != .unreachable(reason) { log?("daemon unreachable: \(reason)") }
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        connection = .unreachable(reason)
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(2))
            guard let self, gen == self.generation else { return }
            self.connect()
        }
    }
}
