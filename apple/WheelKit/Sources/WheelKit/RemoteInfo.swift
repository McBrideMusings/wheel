import Foundation

/// How remote clients reach the daemon: the `remote` reply. `listen` is the `remote.listen`
/// setting wheeld started with, nil when remote access is off; `addresses` are the machine's
/// interface addresses and hostname, any of which a client may dial on `port`; `fingerprint` is
/// the SHA-256 of the daemon's certificate, lowercase hex, which a client pins.
public struct RemoteInfo: Sendable, Equatable {
    public let listen: String?
    public let addresses: [String]
    public let port: UInt16
    public let fingerprint: String
    /// Why the listener is not running although `listen` is set.
    public let problem: String?

    public init(listen: String?, addresses: [String], port: UInt16, fingerprint: String, problem: String?) {
        self.listen = listen
        self.addresses = addresses
        self.port = port
        self.fingerprint = fingerprint
        self.problem = problem
    }

    /// The listener is running: it is set and nothing went wrong starting it.
    public var isOn: Bool { listen != nil && problem == nil }

    /// The address a pairing code names unless a person picks another: a Tailscale address
    /// (`100.64.0.0/10`) when the machine has one, since it reaches the Mac from anywhere, else
    /// the first address that is not a hostname.
    public var defaultAddress: String? {
        addresses.first(where: Self.isTailscale) ?? addresses.first(where: Self.isIP) ?? addresses.first
    }

    static func isIP(_ address: String) -> Bool {
        address.contains(":") || address.split(separator: ".").count == 4 && address.split(separator: ".").allSatisfy { UInt8($0) != nil }
    }

    static func isTailscale(_ address: String) -> Bool {
        let parts = address.split(separator: ".").compactMap { UInt8($0) }
        return parts.count == 4 && address.split(separator: ".").count == 4 && parts[0] == 100 && (64...127).contains(parts[1])
    }

    /// What the pairing code encodes for `address`: `{url, token, cert_sha256}` with sorted keys.
    public func pairing(address: String, token: String) -> String {
        let host = address.contains(":") ? "[\(address)]" : address
        let object: [String: String] = ["url": "wss://\(host):\(port)/ws", "token": token, "cert_sha256": fingerprint]
        let data = (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }
}
