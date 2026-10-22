import CryptoKit
import Foundation

/// What a pairing code carries: where the Mac's remote listener is, the token that opens it, and
/// the SHA-256 of the certificate it must present. `RemoteInfo.pairing` builds the JSON.
public struct Pairing: Codable, Sendable, Equatable {
    public let url: URL
    public let token: String
    public let certSha256: String

    private enum CodingKeys: String, CodingKey {
        case url, token
        case certSha256 = "cert_sha256"
    }

    public enum Invalid: Error, Equatable, LocalizedError {
        case notJSON
        case notSecure
        case noToken
        case badFingerprint

        public var errorDescription: String? {
            switch self {
            case .notJSON: "That is not a pairing code."
            case .notSecure: "The address must start with wss://."
            case .noToken: "The pairing code has no token."
            case .badFingerprint: "The pairing code's certificate fingerprint is not 64 hex digits."
            }
        }
    }

    public init(url: URL, token: String, certSha256: String) {
        self.url = url
        self.token = token
        self.certSha256 = certSha256.lowercased()
    }

    /// Reads the JSON a pairing code or the Mac's Devices tab shows.
    public static func parse(_ text: String) throws -> Pairing {
        guard let data = text.trimmingCharacters(in: .whitespacesAndNewlines).data(using: .utf8),
              let raw = try? JSONDecoder().decode(Pairing.self, from: data) else { throw Invalid.notJSON }
        guard raw.url.scheme == "wss", raw.url.host() != nil else { throw Invalid.notSecure }
        guard !raw.token.isEmpty else { throw Invalid.noToken }
        guard raw.certSha256.count == 64, raw.certSha256.allSatisfy({ $0.isASCII && $0.isHexDigit }) else { throw Invalid.badFingerprint }
        return Pairing(url: raw.url, token: raw.token, certSha256: raw.certSha256)
    }

    /// Whether `certificate` (DER) is the one this pairing pins.
    public func pins(certificate: Data) -> Bool {
        Self.fingerprint(of: certificate) == certSha256
    }

    public static func fingerprint(of certificate: Data) -> String {
        SHA256.hash(data: certificate).map { String(format: "%02x", $0) }.joined()
    }
}

/// Trusts exactly the certificate a `Pairing` names and nothing else: the Mac's certificate is
/// self-signed, so the system's chain check is replaced by a fingerprint comparison on the leaf.
public final class PinnedSessionDelegate: NSObject, URLSessionTaskDelegate, Sendable {
    private let pairing: Pairing
    private let onMismatch: @Sendable () -> Void
    private let trace: @Sendable (String) -> Void

    public init(pairing: Pairing, trace: @escaping @Sendable (String) -> Void = { _ in }, onMismatch: @escaping @Sendable () -> Void) {
        self.pairing = pairing
        self.trace = trace
        self.onMismatch = onMismatch
    }

    public func urlSession(
        _ session: URLSession,
        didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping @Sendable (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust else {
            trace("challenge \(challenge.protectionSpace.authenticationMethod) is not a server trust: default handling")
            completionHandler(.performDefaultHandling, nil)
            return
        }
        let leaf = (SecTrustCopyCertificateChain(trust) as? [SecCertificate])?.first
        let seen = leaf.map { Pairing.fingerprint(of: SecCertificateCopyData($0) as Data) } ?? "no certificate"
        trace("server trust challenge from \(challenge.protectionSpace.host): leaf sha256=\(seen) pinned=\(pairing.certSha256)")
        if let leaf, pairing.pins(certificate: SecCertificateCopyData(leaf) as Data) {
            // The pinned certificate is the one trust anchor and no name is checked, so the
            // system's own evaluation agrees with the pin instead of rejecting a self-signed leaf.
            SecTrustSetPolicies(trust, SecPolicyCreateBasicX509())
            SecTrustSetAnchorCertificates(trust, [leaf] as CFArray)
            SecTrustSetAnchorCertificatesOnly(trust, true)
            var evaluation: CFError?
            let trusted = SecTrustEvaluateWithError(trust, &evaluation)
            trace("system trust evaluation against the pinned anchor: trusted=\(trusted) error=\(evaluation.map { String(describing: $0) } ?? "none")")
            completionHandler(.useCredential, URLCredential(trust: trust))
        } else {
            onMismatch()
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }

    public func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let error else { return }
        let detail = error as NSError
        let info = detail.userInfo.filter { $0.key != NSURLErrorFailingURLErrorKey && $0.key != NSURLErrorFailingURLStringErrorKey }
        trace("task ended: domain=\(detail.domain) code=\(detail.code) info=\(info)")
    }

    /// A session whose connections pin `pairing`'s certificate.
    public static func session(
        for pairing: Pairing,
        configuration: URLSessionConfiguration = .default,
        trace: @escaping @Sendable (String) -> Void = { _ in },
        onMismatch: @escaping @Sendable () -> Void
    ) -> URLSession {
        URLSession(configuration: configuration, delegate: PinnedSessionDelegate(pairing: pairing, trace: trace, onMismatch: onMismatch), delegateQueue: nil)
    }
}

extension DaemonEndpoint {
    /// A token held as a string, as the Keychain keeps it after pairing.
    public static func token(_ value: String) -> @Sendable () async throws -> String {
        { value }
    }
}

extension Node {
    /// Every pane in reading order: top row first, left to right within a row, by where the split
    /// tree places them. The tree's own order (`panes`) goes down a column before the next one.
    public var readingOrder: [(id: String, kind: PaneKind)] {
        return GridLayout(self, in: CGRect(x: 0, y: 0, width: 1000, height: 1000)).panes
            .sorted { ($0.frame.minY.rounded(), $0.frame.minX) < ($1.frame.minY.rounded(), $1.frame.minX) }
            .map { ($0.id, $0.kind) }
    }
}
