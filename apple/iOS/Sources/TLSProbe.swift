import Foundation
import Network
import Security
import WheelKit

/// A one-off TLS connection to the paired Mac through Network.framework, logging the exact state
/// and error it ends in, which URLSession reports only as -1200.
enum TLSProbe {
    static func run(_ pairing: Pairing) {
        guard let host = pairing.url.host,
              let port = pairing.url.port.flatMap({ UInt16(exactly: $0) }).flatMap({ NWEndpoint.Port(rawValue: $0) }) else { return }
        let tls = NWProtocolTLS.Options()
        let pin = pairing.certSha256
        sec_protocol_options_set_verify_block(tls.securityProtocolOptions, { _, trust, complete in
            let ref = sec_trust_copy_ref(trust).takeRetainedValue()
            let leaf = (SecTrustCopyCertificateChain(ref) as? [SecCertificate])?.first
            let seen = leaf.map { Pairing.fingerprint(of: SecCertificateCopyData($0) as Data) }
            Log.info("probe verify: leaf=\(seen ?? "none") matches pin=\(seen == pin)")
            complete(seen == pin)
        }, DispatchQueue.global())
        let connection = NWConnection(host: NWEndpoint.Host(host), port: port, using: NWParameters(tls: tls))
        connection.stateUpdateHandler = { Log.info("probe state: \($0)") }
        connection.start(queue: .global())
        DispatchQueue.global().asyncAfter(deadline: .now() + 10) { connection.cancel() }
    }
}
