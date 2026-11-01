import Foundation
import Testing
@testable import WheelKit

@Suite struct UploadsTests {
    private func uploads() throws -> Uploads {
        let pairing = Pairing(url: URL(string: "wss://192.168.1.5:7430/ws")!, token: "t", certSha256: String(repeating: "a", count: 64))
        return Uploads(client: PinnedHTTP(pairing: pairing) {}, daemon: pairing.url, token: "t")
    }

    @Test func anUploadGoesToTheDaemonsHostOverHTTPSWithTheNameAsOneSegment() throws {
        let url = try uploads().address(pane: "p3", name: "My Photo #1?.jpg")
        #expect(url.absoluteString == "https://192.168.1.5:7430/uploads/p3/My%20Photo%20%231%3F.jpg")
    }

    @Test func thePutHeadCarriesTheBodyLengthAndTheToken() throws {
        var request = URLRequest(url: try uploads().address(pane: "p3", name: "a.txt"))
        request.httpMethod = "PUT"
        request.httpBody = Data(count: 1234)
        request.setValue("Bearer t", forHTTPHeaderField: "Authorization")
        let head = String(decoding: try #require(PinnedHTTPCallHead.of(request)), as: UTF8.self)
        #expect(head.hasPrefix("PUT /uploads/p3/a.txt HTTP/1.1\r\n"))
        #expect(head.contains("\r\nContent-Length: 1234\r\n"))
        #expect(head.contains("\r\nAuthorization: Bearer t\r\n"))
        #expect(head.hasSuffix("\r\n\r\n"))
    }

    @Test func aRefusalSaysWhyInWords() {
        #expect(Uploads.failure(status: 413, body: Data()).reason == "over 100 MB")
        #expect(Uploads.failure(status: 404, body: Data("no agent pane p9".utf8)).reason == "no agent pane p9")
        #expect(Uploads.failure(status: 401, body: Data()).reason == "the Mac refused this phone's credential")
    }

    @Test func theStoredFileDecodesFromTheDaemonsAnswer() throws {
        let json = #"{"path":"/Users/x/Library/Application Support/wheel/uploads/p3/1-a.jpg","name":"1-a.jpg","bytes":42}"#
        let file = try JSONDecoder().decode(UploadedFile.self, from: Data(json.utf8))
        #expect(file == UploadedFile(path: "/Users/x/Library/Application Support/wheel/uploads/p3/1-a.jpg", name: "1-a.jpg", bytes: 42))
    }
}
