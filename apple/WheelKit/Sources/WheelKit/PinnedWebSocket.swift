import Foundation
import Network
import Security

/// A WebSocket to the paired Mac through Network.framework, trusting exactly the certificate the
/// pairing names. URLSession's `wss://` is subject to App Transport Security, which refuses any
/// non-loopback address with a self-signed certificate (reported as -1200); a Network.framework
/// connection is not, and its verify block makes the fingerprint comparison the whole trust check.
public final class PinnedWebSocket: DaemonSocket, @unchecked Sendable {
    private let url: URL
    private let pairing: Pairing
    private let onMismatch: @Sendable () -> Void
    private let queue = DispatchQueue(label: "wheel.pinned-websocket")
    private var connection: NWConnection?
    private var failure: Error?
    private var pendingReceive: (@Sendable (Result<URLSessionWebSocketTask.Message, Error>) -> Void)?
    private var size = 1 << 20

    public init(url: URL, pairing: Pairing, onMismatch: @escaping @Sendable () -> Void) {
        self.url = url
        self.pairing = pairing
        self.onMismatch = onMismatch
    }

    public var maximumMessageSize: Int {
        get { queue.sync { size } }
        set { queue.sync { size = newValue } }
    }

    public func resume() {
        queue.async { [self] in
            let tls = NWProtocolTLS.Options()
            let pin = pairing.certSha256
            let mismatch = onMismatch
            sec_protocol_options_set_verify_block(tls.securityProtocolOptions, { _, trust, complete in
                let ref = sec_trust_copy_ref(trust).takeRetainedValue()
                let leaf = (SecTrustCopyCertificateChain(ref) as? [SecCertificate])?.first
                let seen = leaf.map { Pairing.fingerprint(of: SecCertificateCopyData($0) as Data) }
                if seen != pin { mismatch() }
                complete(seen == pin)
            }, queue)
            let parameters = NWParameters(tls: tls)
            let socket = NWProtocolWebSocket.Options()
            socket.maximumMessageSize = size
            parameters.defaultProtocolStack.applicationProtocols.insert(socket, at: 0)
            let connection = NWConnection(to: .url(url), using: parameters)
            self.connection = connection
            connection.stateUpdateHandler = { [weak self] state in
                switch state {
                case let .failed(error), let .waiting(error): self?.fail(error)
                default: break
                }
            }
            connection.start(queue: queue)
        }
    }

    public func send(_ message: URLSessionWebSocketTask.Message, completionHandler: @escaping @Sendable (Error?) -> Void) {
        queue.async { [self] in
            guard let connection else { completionHandler(URLError(.networkConnectionLost)); return }
            let opcode: NWProtocolWebSocket.Opcode
            let payload: Data
            switch message {
            case let .string(text): (opcode, payload) = (.text, Data(text.utf8))
            case let .data(data): (opcode, payload) = (.binary, data)
            @unknown default: completionHandler(URLError(.cannotParseResponse)); return
            }
            let context = NWConnection.ContentContext(identifier: "wheel", metadata: [NWProtocolWebSocket.Metadata(opcode: opcode)])
            connection.send(content: payload, contentContext: context, isComplete: true, completion: .contentProcessed(completionHandler))
        }
    }

    public func receive(completionHandler: @escaping @Sendable (Result<URLSessionWebSocketTask.Message, Error>) -> Void) {
        queue.async { [self] in
            if let failure { completionHandler(.failure(failure)); return }
            guard let connection else { completionHandler(.failure(URLError(.networkConnectionLost))); return }
            pendingReceive = completionHandler
            let queue = queue
            connection.receiveMessage { data, context, _, error in
                queue.async { self.delivered(data, context, error) }
            }
        }
    }

    public func cancel(with closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?) {
        queue.async { [self] in
            failure = URLError(.cancelled)
            connection?.cancel()
            connection = nil
            pendingReceive = nil
        }
    }

    private func delivered(_ data: Data?, _ context: NWConnection.ContentContext?, _ error: NWError?) {
        guard let done = pendingReceive else { return }
        pendingReceive = nil
        if let error { done(.failure(error)); return }
        let metadata = context?.protocolMetadata(definition: NWProtocolWebSocket.definition) as? NWProtocolWebSocket.Metadata
        switch metadata?.opcode {
        case .text: done(.success(.string(String(decoding: data ?? Data(), as: UTF8.self))))
        case .binary: done(.success(.data(data ?? Data())))
        default: done(.failure(URLError(.networkConnectionLost)))
        }
    }

    private func fail(_ error: NWError) {
        guard failure == nil else { return }
        failure = error
        connection?.cancel()
        if let done = pendingReceive {
            pendingReceive = nil
            done(.failure(error))
        }
    }
}
