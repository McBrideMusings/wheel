import Foundation
import Testing
@testable import WheelKit

@Suite struct RemoteInfoTests {
    static let reply = #"{"ev":"remote","req":4,"listen":"0.0.0.0:7430","addresses":["192.168.1.20","100.101.102.103","studio-mac.local","fd7a::1"],"port":7430,"fingerprint":"ab12"}"#

    private func info() throws -> RemoteInfo {
        guard case let .remote(req, info) = try ServerMsg.decode(Self.reply) else {
            throw DecodingError.dataCorrupted(.init(codingPath: [], debugDescription: "not remote"))
        }
        #expect(req == 4)
        return info
    }

    @Test func aTailscaleAddressIsTheDefault() throws {
        let info = try info()
        #expect(info.isOn)
        #expect(info.defaultAddress == "100.101.102.103")
    }

    @Test func withoutOneTheFirstIPBeatsAHostname() {
        let info = RemoteInfo(listen: "0.0.0.0:1", addresses: ["host.local", "10.0.0.2"], port: 1, fingerprint: "f", problem: nil)
        #expect(info.defaultAddress == "10.0.0.2")
    }

    @Test func thePairingCodeCarriesUrlTokenAndFingerprint() throws {
        let info = try info()
        #expect(info.pairing(address: "100.101.102.103", token: "t0k")
            == #"{"cert_sha256":"ab12","token":"t0k","url":"wss://100.101.102.103:7430/ws"}"#)
        #expect(info.pairing(address: "fd7a::1", token: "t").contains("wss://[fd7a::1]:7430/ws"))
    }

    @Test func offReportsNoListener() throws {
        guard case let .remote(_, off) = try ServerMsg.decode(#"{"ev":"remote","req":1,"listen":null,"addresses":[],"port":0,"fingerprint":""}"#) else { return }
        #expect(!off.isOn)
        #expect(off.defaultAddress == nil)
    }
}
