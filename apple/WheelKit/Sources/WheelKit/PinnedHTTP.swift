import Foundation
import Network
import Security

/// One HTTP/1.1 response read off a byte stream: its head, then its body as framed by
/// `Content-Length`, chunked encoding, or the end of the connection.
public struct HTTPResponseReader: Sendable {
    public enum Event: Equatable, Sendable {
        case head(status: Int, headers: [String: String])
        case body(Data)
        case end
    }

    public struct Malformed: Error, Equatable, LocalizedError {
        public let reason: String
        public var errorDescription: String? { "The Mac sent a malformed HTTP response (\(reason))" }
    }

    private enum Phase {
        case head
        case fixed(remaining: Int)
        case chunkSize
        case chunkData(remaining: Int)
        case chunkEnd
        case trailers
        case untilClose
        case done
    }

    private var phase = Phase.head
    private var buffer = Data()

    public init() {}

    private static let crlf = Data("\r\n".utf8)
    private static let blankLine = Data("\r\n\r\n".utf8)

    /// Takes the next bytes off the connection and returns what they complete.
    public mutating func feed(_ data: Data) throws -> [Event] {
        buffer.append(data)
        var events: [Event] = []
        while try step(&events) {}
        return events
    }

    /// The connection ended: a body framed by that is now whole; any other framing is cut short.
    public mutating func finish() throws -> [Event] {
        switch phase {
        case .done: return []
        case .untilClose:
            phase = .done
            return [.end]
        case .head: throw Malformed(reason: "the connection ended before a response head")
        default: throw Malformed(reason: "the connection ended before the body was complete")
        }
    }

    private mutating func step(_ events: inout [Event]) throws -> Bool {
        switch phase {
        case .head:
            guard let end = buffer.range(of: Self.blankLine) else { return false }
            let text = String(decoding: buffer[buffer.startIndex..<end.lowerBound], as: UTF8.self)
            buffer.removeSubrange(buffer.startIndex..<end.upperBound)
            let (status, headers) = try Self.parseHead(text)
            events.append(.head(status: status, headers: headers))
            let lowered = Dictionary(headers.map { ($0.key.lowercased(), $0.value) }, uniquingKeysWith: { $1 })
            if status >= 100 && status < 200 && status != 101 {
                // An interim response (100 Continue, 103 Early Hints) precedes the real one.
                phase = .head
                events.removeLast()
            } else if status < 200 || status == 204 || status == 304 {
                phase = .done
                events.append(.end)
            } else if lowered["transfer-encoding"]?.lowercased().contains("chunked") == true {
                phase = .chunkSize
            } else if let length = lowered["content-length"] {
                guard let count = Int(length.trimmingCharacters(in: .whitespaces)), count >= 0 else {
                    throw Malformed(reason: "Content-Length \(length)")
                }
                if count == 0 {
                    phase = .done
                    events.append(.end)
                } else {
                    phase = .fixed(remaining: count)
                }
            } else {
                phase = .untilClose
            }
            return true
        case let .fixed(remaining):
            guard !buffer.isEmpty else { return false }
            let take = min(remaining, buffer.count)
            events.append(.body(Data(buffer.prefix(take))))
            buffer.removeFirst(take)
            if take == remaining {
                phase = .done
                events.append(.end)
            } else {
                phase = .fixed(remaining: remaining - take)
            }
            return true
        case .chunkSize:
            guard let line = buffer.range(of: Self.crlf) else { return false }
            let text = String(decoding: buffer[buffer.startIndex..<line.lowerBound], as: UTF8.self)
            buffer.removeSubrange(buffer.startIndex..<line.upperBound)
            let size = text.split(separator: ";", maxSplits: 1).first.map { $0.trimmingCharacters(in: .whitespaces) } ?? ""
            guard let count = Int(size, radix: 16), count >= 0 else { throw Malformed(reason: "chunk size \(text)") }
            phase = count == 0 ? .trailers : .chunkData(remaining: count)
            return true
        case let .chunkData(remaining):
            guard !buffer.isEmpty else { return false }
            let take = min(remaining, buffer.count)
            events.append(.body(Data(buffer.prefix(take))))
            buffer.removeFirst(take)
            phase = take == remaining ? .chunkEnd : .chunkData(remaining: remaining - take)
            return true
        case .chunkEnd:
            guard buffer.count >= 2 else { return false }
            guard buffer.prefix(2) == Self.crlf else { throw Malformed(reason: "no CRLF after a chunk") }
            buffer.removeFirst(2)
            phase = .chunkSize
            return true
        case .trailers:
            guard let line = buffer.range(of: Self.crlf) else { return false }
            let empty = line.lowerBound == buffer.startIndex
            buffer.removeSubrange(buffer.startIndex..<line.upperBound)
            if empty {
                phase = .done
                events.append(.end)
            }
            return true
        case .untilClose:
            guard !buffer.isEmpty else { return false }
            events.append(.body(buffer))
            buffer.removeAll()
            return false
        case .done:
            buffer.removeAll()
            return false
        }
    }

    private static func parseHead(_ text: String) throws -> (Int, [String: String]) {
        var lines = text.components(separatedBy: "\r\n")
        let first = lines.removeFirst()
        let parts = first.split(separator: " ", maxSplits: 2)
        guard parts.count >= 2, parts[0].hasPrefix("HTTP/"), let status = Int(parts[1]) else {
            throw Malformed(reason: "status line \(first)")
        }
        var headers: [String: String] = [:]
        for line in lines {
            guard let colon = line.firstIndex(of: ":") else { throw Malformed(reason: "header line \(line)") }
            let name = String(line[line.startIndex..<colon])
            let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            if let held = headers.first(where: { $0.key.lowercased() == name.lowercased() }) {
                headers[held.key] = held.value + ", " + value
            } else {
                headers[name] = value
            }
        }
        return (status, headers)
    }
}

/// The head of a response from `PinnedHTTP`.
public struct PinnedHTTPHead: Sendable {
    public let status: Int
    public let headers: [String: String]
}

/// An HTTPS client for the paired Mac that trusts exactly the certificate the pairing names.
/// URLSession `https://` is subject to App Transport Security, which refuses any non-loopback
/// address with a self-signed certificate (reported as -1200); a Network.framework connection is
/// not, and its verify block makes the fingerprint comparison the whole trust check. One
/// connection per request, closed when the response ends.
public final class PinnedHTTP: Sendable {
    private let pairing: Pairing
    private let onMismatch: @Sendable () -> Void

    public init(pairing: Pairing, onMismatch: @escaping @Sendable () -> Void) {
        self.pairing = pairing
        self.onMismatch = onMismatch
    }

    public var pinnedTo: Pairing { pairing }

    /// Sends `request` and returns the response's head once it arrives, with its body as the
    /// connection delivers it. Cancelling the consuming task closes the connection.
    public func open(_ request: URLRequest) async throws -> (head: PinnedHTTPHead, body: AsyncThrowingStream<Data, Error>) {
        let call = PinnedHTTPCall(request: request, pairing: pairing, onMismatch: onMismatch)
        return try await withTaskCancellationHandler {
            try await call.start()
        } onCancel: {
            call.cancel()
        }
    }
}

/// One request on one connection. Every mutable field is touched only on `queue`.
private final class PinnedHTTPCall: @unchecked Sendable {
    private let request: URLRequest
    private let pairing: Pairing
    private let onMismatch: @Sendable () -> Void
    private let queue = DispatchQueue(label: "wheel.pinned-http")
    private var connection: NWConnection?
    private var reader = HTTPResponseReader()
    private var headContinuation: CheckedContinuation<PinnedHTTPHead, Error>?
    private var bodyContinuation: AsyncThrowingStream<Data, Error>.Continuation?
    private var finished = false

    init(request: URLRequest, pairing: Pairing, onMismatch: @escaping @Sendable () -> Void) {
        self.request = request
        self.pairing = pairing
        self.onMismatch = onMismatch
    }

    func start() async throws -> (head: PinnedHTTPHead, body: AsyncThrowingStream<Data, Error>) {
        let body = AsyncThrowingStream<Data, Error> { continuation in
            queue.sync { bodyContinuation = continuation }
            continuation.onTermination = { [self] _ in cancel() }
        }
        let head = try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<PinnedHTTPHead, Error>) in
            queue.async { [self] in
                if finished {
                    continuation.resume(throwing: URLError(.cancelled))
                    return
                }
                headContinuation = continuation
                connect()
            }
        }
        return (head, body)
    }

    func cancel() {
        queue.async { [self] in conclude(URLError(.cancelled)) }
    }

    private func connect() {
        guard let url = request.url, let host = url.host(), let wire = Self.wire(request) else {
            conclude(URLError(.badURL))
            return
        }
        let port = NWEndpoint.Port(rawValue: UInt16(url.port ?? 443)) ?? .https
        let tls = NWProtocolTLS.Options()
        let pairing = pairing
        let mismatch = onMismatch
        sec_protocol_options_set_verify_block(tls.securityProtocolOptions, { _, trust, complete in
            let ref = sec_trust_copy_ref(trust).takeRetainedValue()
            let leaf = (SecTrustCopyCertificateChain(ref) as? [SecCertificate])?.first
            let pinned = leaf.map { pairing.pins(certificate: SecCertificateCopyData($0) as Data) } ?? false
            if !pinned { mismatch() }
            complete(pinned)
        }, queue)
        let connection = NWConnection(host: NWEndpoint.Host(host), port: port, using: NWParameters(tls: tls))
        self.connection = connection
        // The handlers hold the call until `conclude` cancels the connection: nothing else does.
        connection.stateUpdateHandler = { [self] state in
            switch state {
            case .ready:
                connection.send(content: wire, completion: .contentProcessed { error in
                    if let error { self.conclude(error) }
                })
                receive()
            case let .failed(error): conclude(error)
            case let .waiting(error): waiting(error)
            default: break
            }
        }
        connection.start(queue: queue)
    }

    /// The request as it goes on the wire: the identity encoding keeps the body readable as sent.
    private static func wire(_ request: URLRequest) -> Data? {
        guard let url = request.url, let host = url.host() else { return nil }
        let path = url.path(percentEncoded: true)
        var target = path.isEmpty ? "/" : path
        if let query = url.query(percentEncoded: true) { target += "?" + query }
        let body = request.httpBody ?? Data()
        var lines = ["\(request.httpMethod ?? "GET") \(target) HTTP/1.1"]
        lines.append("Host: " + (url.port.map { "\(host):\($0)" } ?? host))
        lines.append("Connection: close")
        lines.append("Accept-Encoding: identity")
        for (name, value) in request.allHTTPHeaderFields ?? [:] {
            switch name.lowercased() {
            case "host", "connection", "accept-encoding", "content-length", "transfer-encoding": continue
            default: lines.append("\(name): \(value)")
            }
        }
        if !body.isEmpty || ["POST", "PUT", "PATCH"].contains(request.httpMethod ?? "GET") {
            lines.append("Content-Length: \(body.count)")
        }
        return Data((lines.joined(separator: "\r\n") + "\r\n\r\n").utf8) + body
    }

    private func receive() {
        connection?.receive(minimumIncompleteLength: 1, maximumLength: 1 << 16) { [self] data, _, complete, error in
            guard !finished else { return }
            if let error { conclude(error); return }
            do {
                var events: [HTTPResponseReader.Event] = []
                if let data, !data.isEmpty { events += try reader.feed(data) }
                if complete { events += try reader.finish() }
                for event in events { deliver(event) }
            } catch {
                conclude(error)
                return
            }
            if complete { conclude(nil) } else if !finished { receive() }
        }
    }

    private func deliver(_ event: HTTPResponseReader.Event) {
        switch event {
        case let .head(status, headers):
            headContinuation?.resume(returning: PinnedHTTPHead(status: status, headers: headers))
            headContinuation = nil
        case let .body(data): bodyContinuation?.yield(data)
        case .end: conclude(nil)
        }
    }

    /// A connection waits while the network is down or the Local Network prompt is open, and
    /// recovers on its own; one still waiting after ten seconds counts as unreachable.
    private func waiting(_ error: NWError) {
        queue.asyncAfter(deadline: .now() + 10) { [self] in
            guard !finished, connection?.state != .ready else { return }
            conclude(error)
        }
    }

    private func conclude(_ error: Error?) {
        guard !finished else { return }
        finished = true
        connection?.stateUpdateHandler = nil
        connection?.cancel()
        connection = nil
        if let headContinuation {
            self.headContinuation = nil
            headContinuation.resume(throwing: error ?? URLError(.badServerResponse))
        }
        bodyContinuation?.finish(throwing: error)
        bodyContinuation = nil
    }
}

