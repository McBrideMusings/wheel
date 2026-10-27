import Foundation
import Testing
@testable import WheelKit

@Suite struct PinnedHTTPTests {
    private func read(_ pieces: [String], closed: Bool = false) throws -> [HTTPResponseReader.Event] {
        var reader = HTTPResponseReader()
        var events: [HTTPResponseReader.Event] = []
        for piece in pieces { events += try reader.feed(Data(piece.utf8)) }
        if closed { events += try reader.finish() }
        return events
    }

    private func body(_ events: [HTTPResponseReader.Event]) -> String {
        var data = Data()
        for case let .body(chunk) in events { data.append(chunk) }
        return String(decoding: data, as: UTF8.self)
    }

    @Test func aContentLengthBodyEndsAfterThatManyBytesWhateverTheSplit() throws {
        let events = try read(["HTTP/1.1 200 OK\r\nContent-Le", "ngth: 11\r\nX-A: b\r\n\r\nhello", " world"])
        #expect(events.first == .head(status: 200, headers: ["Content-Length": "11", "X-A": "b"]))
        #expect(body(events) == "hello world")
        #expect(events.last == .end)
    }

    @Test func aChunkedBodyIsJoinedAndItsTrailersAreDropped() throws {
        let events = try read([
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel", "lo\r\n6;x=y\r\n world\r\n0\r\nT: 1\r\n\r\n",
        ])
        #expect(body(events) == "hello world")
        #expect(events.last == .end)
    }

    @Test func aBodyWithNoLengthEndsWhenTheConnectionDoes() throws {
        var reader = HTTPResponseReader()
        var events = try reader.feed(Data("HTTP/1.1 200 OK\r\n\r\nabc".utf8))
        #expect(!events.contains(.end))
        events += try reader.finish()
        #expect(body(events) == "abc")
        #expect(events.last == .end)
    }

    @Test func aStatusWithNoBodyEndsAtItsHead() throws {
        let events = try read(["HTTP/1.1 304 Not Modified\r\nETag: x\r\n\r\n"])
        #expect(events == [.head(status: 304, headers: ["ETag": "x"]), .end])
    }

    @Test func aCutShortBodyIsAnError() throws {
        #expect(throws: HTTPResponseReader.Malformed.self) {
            _ = try read(["HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc"], closed: true)
        }
    }

    @Test func aBadStatusLineIsAnError() throws {
        #expect(throws: HTTPResponseReader.Malformed.self) { _ = try read(["garbage\r\n\r\n"]) }
    }
}

extension PinnedHTTPTests {
    @Test func anInterimResponseIsSkippedForTheRealOne() throws {
        let events = try read(["HTTP/1.1 103 Early Hints\r\nLink: x\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi"])
        #expect(events.first == .head(status: 200, headers: ["Content-Length": "2"]))
        #expect(body(events) == "hi")
    }
}
