import Foundation
import Testing
@testable import WheelKit

/// A socket that records what the client sends and lets a test play the daemon's side.
private final class FakeSocket: DaemonSocket, @unchecked Sendable {
    let url: URL
    var maximumMessageSize = 0
    private(set) var resumed = false
    private(set) var cancelled = false
    private(set) var sent: [String] = []
    private var waiting: (@Sendable (Result<URLSessionWebSocketTask.Message, Error>) -> Void)?

    init(url: URL) { self.url = url }

    func resume() { resumed = true }
    func send(_ message: URLSessionWebSocketTask.Message, completionHandler: @escaping @Sendable (Error?) -> Void) {
        if case let .string(text) = message { sent.append(text) }
        completionHandler(nil)
    }
    func receive(completionHandler: @escaping @Sendable (Result<URLSessionWebSocketTask.Message, Error>) -> Void) {
        waiting = completionHandler
    }
    func cancel(with closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?) { cancelled = true }

    /// Delivers a frame to the client's pending receive.
    func deliver(_ text: String) {
        let handler = waiting
        waiting = nil
        handler?(.success(.string(text)))
    }
    func fail(_ message: String) {
        let handler = waiting
        waiting = nil
        handler?(.failure(NSError(domain: "fake", code: 1, userInfo: [NSLocalizedDescriptionKey: message])))
    }
}

private struct TokenError: Error {}

@MainActor
private final class Harness {
    var sockets: [FakeSocket] = []
    var connects: [ServerMsg] = []
    var messages: [ServerMsg] = []
    var states: [DaemonClient.Connection] = []
    let client: DaemonClient

    init(token: @escaping @Sendable () async throws -> String = { "tok" }, pairingAs: String? = nil) {
        var made: ((FakeSocket) -> Void)?
        client = DaemonClient(
            endpoint: DaemonEndpoint(url: URL(string: "ws://127.0.0.1:7499/ws")!, token: token, pairingAs: pairingAs),
            retryDelay: .milliseconds(20),
            makeSocket: { url in
                let socket = FakeSocket(url: url)
                made?(socket)
                return socket
            }
        )
        made = { [unowned self] in sockets.append($0) }
        client.onConnect = { [unowned self] in connects.append($0) }
        client.onMessage = { [unowned self] in messages.append($0) }
        client.onConnection = { [unowned self] in states.append($0) }
    }

    /// Waits (up to 2s) for `condition`, letting the client's tasks run.
    func until(_ condition: () -> Bool) async {
        for _ in 0..<200 where !condition() { try? await Task.sleep(for: .milliseconds(10)) }
    }

    /// The status reply the daemon gives the probe on `socket`.
    func answerProbe(_ socket: FakeSocket, uptime: Int = 5) throws {
        let probe = try #require(socket.sent.first)
        let req = try #require(JSONSerialization.jsonObject(with: Data(probe.utf8)) as? [String: Any])["req"] as? Int
        socket.deliver(Self.status(req: req, uptime: uptime))
    }

    static func status(req: Int?, uptime: Int) -> String {
        let r = req.map { "\"req\":\($0)," } ?? ""
        return "{\(r)\"ev\":\"status\",\"version\":\"0\",\"log_path\":\"/l\",\"uptime_secs\":\(uptime),\"terms\":[],\"perms\":[],\"spaces\":[],\"active\":null,\"agents\":[]}"
    }
}

@MainActor
struct DaemonClientTests {
    @Test func connectsOnlyAfterTheProbeIsAnswered() async throws {
        let h = Harness()
        h.client.start()
        await h.until { h.sockets.first?.resumed == true && h.sockets.first?.sent.count == 1 }
        let socket = try #require(h.sockets.first)
        #expect(socket.url.absoluteString == "ws://127.0.0.1:7499/ws?token=tok")
        #expect(socket.maximumMessageSize == 64 << 20)
        // The first frame out is the status probe, and nothing is connected before its reply.
        #expect(socket.sent.first?.contains("\"op\":\"status\"") == true)
        #expect(h.client.connection == .connecting)
        #expect(h.connects.isEmpty)

        try h.answerProbe(socket)
        await h.until { h.client.connection == .connected }
        #expect(h.connects.count == 1)
        #expect(h.messages.isEmpty)
        // After the probe, frames go to onMessage, not onConnect.
        socket.deliver(#"{"ev":"perms","perms":[]}"#)
        await h.until { !h.messages.isEmpty }
        #expect(h.connects.count == 1)
        #expect(h.messages.count == 1)
    }

    /// A one-time pairing token goes up as `pair` with the device's name; once the client swaps in
    /// the credential it was handed, the next connection presents it as `token`.
    @Test func aPairingTokenIsPresentedOnceAndThenTheCredential() async throws {
        let h = Harness(pairingAs: "Test phone")
        h.client.start()
        await h.until { h.sockets.first?.sent.count == 1 }
        let first = try #require(h.sockets.first)
        #expect(first.url.query == "pair=tok&name=Test%20phone")
        h.client.endpoint = DaemonEndpoint(url: first.url, token: { "credential" })
        first.fail("closed")
        await h.until { h.sockets.count == 2 }
        #expect(h.sockets[1].url.query == "token=credential")
    }

    @Test func aStatusWithAnotherReqIsNotTheProbe() async throws {
        let h = Harness()
        h.client.start()
        await h.until { h.sockets.first?.sent.count == 1 }
        let socket = try #require(h.sockets.first)
        socket.deliver(Harness.status(req: 9999, uptime: 1))
        await h.until { !h.messages.isEmpty }
        #expect(h.client.connection == .connecting)
        #expect(h.connects.isEmpty)
        #expect(h.messages.count == 1)
    }

    @Test func sendIsDroppedUntilConnected() async throws {
        let h = Harness()
        h.client.start()
        await h.until { h.sockets.first?.sent.count == 1 }
        let socket = try #require(h.sockets.first)
        h.client.send(.termDetach(id: "t1"))
        #expect(socket.sent.count == 1)
        try h.answerProbe(socket)
        await h.until { h.client.connection == .connected }
        h.client.send(.termDetach(id: "t1"))
        #expect(socket.sent.count == 2)
        #expect(socket.sent.last?.contains("term_detach") == true)
    }

    @Test func aReceiveFailureMarksUnreachableAndReconnectsOnANewSocket() async throws {
        let h = Harness()
        h.client.start()
        await h.until { h.sockets.first?.sent.count == 1 }
        let first = try #require(h.sockets.first)
        try h.answerProbe(first)
        await h.until { h.client.connection == .connected }

        first.fail("boom")
        await h.until { h.client.connection == .unreachable("boom") }
        #expect(first.cancelled)
        // The retry opens a second socket and probes it.
        await h.until { h.sockets.count == 2 && h.sockets[1].sent.count == 1 }
        #expect(h.sockets.count == 2)
        #expect(h.client.connection == .connecting)
        try h.answerProbe(h.sockets[1])
        await h.until { h.client.connection == .connected }
        #expect(h.connects.count == 2)
        #expect(h.states == [.connected, .unreachable("boom"), .connecting, .connected])
    }

    @Test func anOlderSocketsFramesAndFailureAreIgnoredAfterARestart() async throws {
        let h = Harness()
        h.client.start()
        await h.until { h.sockets.first?.sent.count == 1 }
        let first = try #require(h.sockets.first)
        try h.answerProbe(first)
        await h.until { h.client.connection == .connected }

        // A second start() supersedes the first socket, whose receive is still pending.
        h.client.start()
        await h.until { h.sockets.count == 2 && h.sockets[1].sent.count == 1 }
        try h.answerProbe(h.sockets[1])
        await h.until { h.client.connection == .connected && h.connects.count == 2 }

        first.deliver(#"{"ev":"perms","perms":[]}"#)
        try await Task.sleep(for: .milliseconds(50))
        #expect(h.messages.isEmpty)
        first.fail("late")
        try await Task.sleep(for: .milliseconds(100))
        #expect(h.client.connection == .connected)
        #expect(h.sockets.count == 2)
    }

    @Test func aMissingTokenIsUnreachableAndRetries() async throws {
        actor Calls { var n = 0; func next() -> Int { n += 1; return n } }
        let calls = Calls()
        let h = Harness(token: {
            if await calls.next() == 1 { throw TokenError() }
            return "tok"
        })
        h.client.start()
        await h.until { if case .unreachable = h.client.connection { return true } else { return false } }
        guard case let .unreachable(reason) = h.client.connection else { Issue.record("not unreachable"); return }
        #expect(reason.hasPrefix("no daemon token"))
        #expect(h.sockets.isEmpty)
        // The retry reads the token again and opens a socket.
        await h.until { h.sockets.count == 1 && h.sockets[0].sent.count == 1 }
        #expect(h.sockets.count == 1)
    }

    @Test func aRestartDuringTokenReadAbandonsTheOlderAttempt() async throws {
        actor Gate {
            var waiting: [CheckedContinuation<Void, Never>] = []
            func wait() async { await withCheckedContinuation { waiting.append($0) } }
            func count() -> Int { waiting.count }
            func release() { waiting.forEach { $0.resume() }; waiting = [] }
        }
        let gate = Gate()
        let h = Harness(token: { await gate.wait(); return "tok" })
        h.client.start()
        h.client.start()
        for _ in 0..<200 where await gate.count() < 2 { try await Task.sleep(for: .milliseconds(10)) }
        await gate.release()
        await h.until { !h.sockets.isEmpty }
        try await Task.sleep(for: .milliseconds(50))
        // Two starts, two token reads, one socket: only the newest generation connects.
        #expect(h.sockets.count == 1)
    }
}

@MainActor
struct AppLogTests {
    private func tempLog(cap: UInt64 = 10 << 20) throws -> URL {
        let dir = FileManager.default.temporaryDirectory.appending(path: "applog-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appending(path: "app.log")
    }

    private func logFrames(_ socket: FakeSocket) -> [[String: Any]] {
        socket.sent.compactMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any] }.filter { $0["op"] as? String == "log" }
    }

    /// Lines written while wheeld is unreachable go out once it answers, and once acknowledged they
    /// are never sent again: not on the next connection, and not by the next launch.
    @Test func linesWrittenWhileDisconnectedGoOutOnceOnReconnect() async throws {
        let url = try tempLog()
        let log = AppLog(url: url, client: "iphone")
        let h = Harness()
        h.client.appLog = log
        for i in 1...5 { log.write("line \(i)\nsecond part") }
        h.client.start()
        await h.until { h.sockets.first?.sent.count == 1 }
        let first = try #require(h.sockets.first)
        try h.answerProbe(first)
        await h.until { !logFrames(first).isEmpty }
        let frame = try #require(logFrames(first).first)
        #expect(frame["client"] as? String == "iphone")
        let lines = try #require(frame["lines"] as? [[String: Any]])
        #expect(lines.map { $0["msg"] as? String } == (1...5).map { "line \($0)\nsecond part" })
        #expect(lines.allSatisfy { ($0["ts_ms"] as? Int ?? 0) > 1_700_000_000_000 })
        first.deliver(#"{"ev":"ok","req":\#(frame["req"] as! Int)}"#)
        await h.until { log.pending() == nil }

        first.fail("closed")
        await h.until { h.sockets.count == 2 && h.sockets[1].sent.count == 1 }
        try h.answerProbe(h.sockets[1])
        await h.until { h.client.connection == .connected }
        log.write("line 6")
        try await Task.sleep(for: .milliseconds(1300))
        let again = logFrames(h.sockets[1]).flatMap { $0["lines"] as? [[String: Any]] ?? [] }
        #expect(again.map { $0["msg"] as? String } == ["line 6"])
        #expect(h.messages.isEmpty)
    }

    /// The lines leave the device, so no URL in one keeps a password or a query, whoever logged it.
    @Test func aURLsCredentialsNeverReachTheFile() throws {
        let url = try tempLog()
        let log = AppLog(url: url, client: "mac")
        log.write("setting set logs.push_url = https://u:secret@loki.example/push in this machine's wheel.toml")
        log.write("browser pane p2 loaded \"http://127.0.0.1:7489/cb?token=abc#top\", status 200")
        let msgs = try #require(log.pending()).lines.map(\.msg)
        #expect(msgs == ["setting set logs.push_url = https://loki.example/push in this machine's wheel.toml",
                         "browser pane p2 loaded \"http://127.0.0.1:7489/cb\", status 200"])
        let file = try String(contentsOf: url, encoding: .utf8)
        #expect(!file.contains("secret") && !file.contains("token=abc"))
    }

    /// A roll before wheeld had everything sends the rest of the rolled file first, then the new one.
    @Test func linesInARolledFileStillGoOut() throws {
        let url = try tempLog()
        let log = AppLog(url: url, client: "mac", cap: 200)
        let long = String(repeating: "x", count: 60)
        for i in 1...3 { log.write("\(i) \(long)") }
        var sent: [String] = []
        while let batch = log.pending() {
            sent += batch.lines.map(\.msg)
            log.acknowledge(batch.end)
        }
        #expect(sent == (1...3).map { "\($0) \(long)" })
        #expect(AppLog(url: url, client: "mac").pending() == nil)
    }
}
