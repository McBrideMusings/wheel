import Foundation

/// A file the Mac stored for an agent pane: the absolute path the agent reads, the name it is kept
/// under in the pane's upload directory, and its size.
public struct UploadedFile: Codable, Equatable, Sendable {
    public let path: String
    public let name: String
    public let bytes: Int

    public init(path: String, name: String, bytes: Int) {
        self.path = path
        self.name = name
        self.bytes = bytes
    }
}

/// Why an upload did not reach the Mac, in words for the composer's chip.
public struct UploadFailed: Error, LocalizedError, Equatable {
    public let reason: String
    public init(_ reason: String) { self.reason = reason }
    public var errorDescription: String? { reason }
}

/// Sends files to the paired Mac for an agent pane (protocol 3): `PUT /uploads/<pane>/<name>`
/// with the body, `DELETE /uploads/<pane>/<stored name>` to take one back. The connection pins the
/// pairing's certificate (`PinnedHTTP`) and carries the device's credential as a bearer token.
public struct Uploads: Sendable {
    /// The largest file the Mac takes: `UPLOAD_MAX_BYTES` in `proto/src/lib.rs`.
    public static let maxBytes = 100 << 20

    private let client: PinnedHTTP
    private let base: URL
    private let token: String

    /// `daemon` is the pairing's WebSocket address (`wss://host:port/ws`); uploads go to the same
    /// host and port over HTTPS.
    public init(client: PinnedHTTP, daemon: URL, token: String) {
        self.client = client
        self.token = token
        var parts = URLComponents(url: daemon, resolvingAgainstBaseURL: false) ?? URLComponents()
        parts.scheme = parts.scheme == "ws" ? "http" : "https"
        parts.path = ""
        parts.query = nil
        self.base = parts.url ?? daemon
    }

    /// The address of `name` under `pane`'s uploads, each part percent-encoded as one path segment.
    func address(pane: String, name: String) -> URL {
        base.appending(path: "uploads").appending(path: pane).appending(path: name)
    }

    /// Send `data` as `name` to `pane`. `progress` is told the fraction sent as the body goes out.
    public func upload(
        _ data: Data,
        name: String,
        pane: String,
        progress: (@Sendable (Double) -> Void)? = nil
    ) async throws -> UploadedFile {
        guard data.count <= Self.maxBytes else { throw UploadFailed("over \(Self.maxBytes >> 20) MB") }
        var request = URLRequest(url: address(pane: pane, name: name))
        request.httpMethod = "PUT"
        request.httpBody = data
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        let total = max(data.count, 1)
        let (head, body) = try await client.open(request) { sent in progress?(Double(sent) / Double(total)) }
        let reply = try await Self.collect(body)
        guard head.status == 201 else { throw Self.failure(status: head.status, body: reply) }
        do {
            return try JSONDecoder().decode(UploadedFile.self, from: reply)
        } catch {
            throw UploadFailed("the Mac's answer was not readable")
        }
    }

    /// Take back `name`, a file `upload` stored for `pane`. A file already gone counts as taken back.
    public func delete(name: String, pane: String) async throws {
        var request = URLRequest(url: address(pane: pane, name: name))
        request.httpMethod = "DELETE"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let (head, body) = try await client.open(request)
        let reply = try await Self.collect(body)
        guard head.status == 204 || head.status == 404 else { throw Self.failure(status: head.status, body: reply) }
    }

    private static func collect(_ body: AsyncThrowingStream<Data, Error>) async throws -> Data {
        var data = Data()
        for try await chunk in body { data.append(chunk) }
        return data
    }

    /// The words for a refused upload: the daemon's own reason when it sent one.
    static func failure(status: Int, body: Data) -> UploadFailed {
        let said = String(decoding: body, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        switch status {
        case 401: return UploadFailed("the Mac refused this phone's credential")
        case 404: return UploadFailed(said.isEmpty ? "the agent pane is gone" : said)
        case 413: return UploadFailed(said.isEmpty ? "over \(maxBytes >> 20) MB" : said)
        default: return UploadFailed(said.isEmpty ? "the Mac answered \(status)" : said)
        }
    }
}
