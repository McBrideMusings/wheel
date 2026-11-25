import Foundation
import Testing
@testable import WheelKit

@Suite struct PairingTests {
    static let fp = String(repeating: "ab", count: 32)

    @Test func theMacsPairingJSONParses() throws {
        let info = RemoteInfo(listen: "0.0.0.0:7430", addresses: ["10.0.0.2"], port: 7430, fingerprint: Self.fp, problem: nil)
        let p = try Pairing.parse(info.pairing(address: "10.0.0.2", token: "t0k"))
        #expect(p.url.absoluteString == "wss://10.0.0.2:7430/ws")
        #expect(p.token == "t0k")
        #expect(p.certSha256 == Self.fp)
    }

    @Test func badCodesAreRefusedWithTheirReason() {
        #expect(throws: Pairing.Invalid.notJSON) { try Pairing.parse("hello") }
        #expect(throws: Pairing.Invalid.notSecure) { try Pairing.parse(#"{"url":"ws://h:1/ws","token":"t","cert_sha256":"\#(Self.fp)"}"#) }
        #expect(throws: Pairing.Invalid.noToken) { try Pairing.parse(#"{"url":"wss://h:1/ws","token":"","cert_sha256":"\#(Self.fp)"}"#) }
        #expect(throws: Pairing.Invalid.badFingerprint) { try Pairing.parse(#"{"url":"wss://h:1/ws","token":"t","cert_sha256":"ab12"}"#) }
    }

    @Test func aPinMatchesOnlyItsOwnCertificate() {
        let der = Data("certificate".utf8)
        let p = Pairing(url: URL(string: "wss://h:1/ws")!, token: "t", certSha256: Pairing.fingerprint(of: der).uppercased())
        #expect(p.pins(certificate: der))
        #expect(!p.pins(certificate: Data("other".utf8)))
    }

    @Test func aGridReadsLeftToRightThenDown() {
        func pane(_ id: String) -> Node { .pane(id: id, kind: .empty) }
        let tree = Node.split(id: "s", axis: .horizontal, ratio: 0.5,
                              a: .split(id: "l", axis: .vertical, ratio: 0.5, a: pane("tl"), b: pane("bl")),
                              b: .split(id: "r", axis: .vertical, ratio: 0.5, a: pane("tr"), b: pane("br")))
        #expect(tree.panes.map(\.id) == ["tl", "bl", "tr", "br"])
        #expect(tree.readingOrder.map(\.id) == ["tl", "tr", "bl", "br"])
    }

    /// The phone's list of paired wheelds: a second server is added, pairing one again replaces it
    /// by its id even at a new address, and one with no id yet is matched by address.
    @Test func keepsOneEntryPerServer() {
        let fp = String(repeating: "a", count: 64)
        let mac = Pairing(url: URL(string: "wss://10.0.0.1:7433/ws")!, token: "c1", certSha256: fp, server: "mac")
        let dev = Pairing(url: URL(string: "wss://10.0.0.2:7433/ws")!, token: "c2", certSha256: fp, server: "dev")
        let list = [mac].keeping(dev)
        #expect(list.map(\.server) == ["mac", "dev"])
        let moved = Pairing(url: URL(string: "wss://100.64.0.9:7433/ws")!, token: "c3", certSha256: fp, server: "mac")
        #expect(list.keeping(moved) == [moved, dev])
        let unnamed = Pairing(url: dev.url, token: "c4", certSha256: fp)
        #expect(list.keeping(unnamed) == [mac, unnamed])
    }
}
