#if DEBUG
import Foundation
import Network
import WheelKit

/// A loopback HTTP endpoint (Debug builds only) that answers with the app's state as JSON, so a
/// script can check what the window shows. It carries the terminal's screen, so it is guarded
/// like the daemon: the daemon's token as a bearer header, no browser Origin, loopback Host only.
///
///     curl -H "Authorization: Bearer $(cat <token file>)" http://127.0.0.1:7421/state
///
/// It also drives the UI's own code paths, so a script exercises what the buttons do:
/// `POST /lead` with the message as the body (the composer's send), and
/// `POST /perm?id=<id>&choice=allow|deny` (the Waiting list's buttons).
///
/// Port from WHEEL_DEBUG_PORT, default 7421.
@MainActor
final class DebugServer {
    private var listener: NWListener?
    private let model: AppModel
    private var port: UInt16 = 7421

    init(model: AppModel) {
        self.model = model
    }

    func start() {
        port = UInt16(ProcessInfo.processInfo.environment["WHEEL_DEBUG_PORT"] ?? "") ?? 7421
        do {
            let params = NWParameters.tcp
            params.requiredLocalEndpoint = NWEndpoint.hostPort(host: "127.0.0.1", port: NWEndpoint.Port(rawValue: port)!)
            let listener = try NWListener(using: params)
            listener.stateUpdateHandler = { [port] state in
                switch state {
                case .ready: Log.info("debug state at http://127.0.0.1:\(port)/state")
                case let .failed(error): Log.info("debug server on port \(port) failed: \(error)")
                default: break
                }
            }
            listener.newConnectionHandler = { [weak self] connection in
                Task { @MainActor in self?.serve(connection) }
            }
            listener.start(queue: .main)
            self.listener = listener
        } catch {
            Log.info("debug server failed to start: \(error)")
        }
    }

    private func serve(_ connection: NWConnection) {
        connection.start(queue: .main)
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, _, _ in
            Task { @MainActor in
                guard let self else { return }
                let request = String(decoding: data ?? Data(), as: UTF8.self)
                let (status, body) = self.respond(to: request)
                let head = "HTTP/1.1 \(status)\r\nContent-Type: application/json\r\nContent-Length: \(body.count)\r\nConnection: close\r\n\r\n"
                connection.send(content: Data(head.utf8) + body, completion: .contentProcessed { _ in connection.cancel() })
            }
        }
    }

    private func respond(to request: String) -> (String, Data) {
        var headers: [String: String] = [:]
        for line in request.split(separator: "\r\n").dropFirst() {
            guard let colon = line.firstIndex(of: ":") else { continue }
            headers[line[..<colon].lowercased()] = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        }
        func refuse(_ status: String, _ why: String) -> (String, Data) {
            Log.info("debug request refused: \(why)")
            return (status, Data(#"{"error":"\#(why)"}"#.utf8))
        }
        if headers["origin"] != nil { return refuse("403 Forbidden", "browser origin") }
        let host = headers["host"] ?? ""
        guard host == "127.0.0.1:\(port)" || host == "localhost:\(port)" else { return refuse("403 Forbidden", "host must be loopback") }
        let token = (try? String(contentsOf: model.tokenFile, encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let token, !token.isEmpty, headers["authorization"] == "Bearer \(token)" else {
            return refuse("401 Unauthorized", "missing or wrong token")
        }
        let requestLine = request.prefix { $0 != "\r" }.split(separator: " ")
        let method = requestLine.first.map(String.init) ?? "GET"
        let target = URLComponents(string: requestLine.count > 1 ? String(requestLine[1]) : "/") ?? URLComponents()
        let query = Dictionary((target.queryItems ?? []).map { ($0.name, $0.value ?? "") }, uniquingKeysWith: { _, b in b })
        switch (method, target.path) {
        case ("POST", "/lead"):
            let text = request.range(of: "\r\n\r\n").map { String(request[$0.upperBound...]) } ?? ""
            Log.info("debug: lead send")
            model.sendToLead(text)
        case ("POST", "/perm"):
            guard let id = query["id"], let choice = query["choice"], let perm = model.perms.first(where: { $0.id == id }) else {
                return refuse("404 Not Found", "no waiting permission with that id")
            }
            Log.info("debug: answer \(id) \(choice)")
            model.answer(perm, choice)
        case ("GET", _):
            break
        default:
            return refuse("404 Not Found", "unknown route")
        }
        let body = (try? JSONSerialization.data(withJSONObject: state(), options: [.prettyPrinted, .sortedKeys])) ?? Data()
        return ("200 OK", body)
    }

    private func rowText(_ kind: TranscriptRow.Kind) -> String {
        switch kind {
        case let .user(text): "user: \(text)"
        case let .agent(text): "agent: \(text)"
        case let .tool(title, _, status): "tool: \(title) (\(status))"
        case let .plan(entries): "plan: \(entries.map(\.content).joined(separator: "; "))"
        case let .notice(text): "notice: \(text)"
        case let .error(text): "error: \(text)"
        }
    }

    private func state() -> [String: Any] {
        let connection: String = switch model.connection {
        case .connected: "connected"
        case .connecting: "connecting"
        case let .unreachable(reason): "unreachable: \(reason)"
        }
        return [
            "daemon_url": model.endpoint.url.absoluteString,
            "connection": connection,
            "term_id": model.termId ?? NSNull(),
            "attached": model.attached,
            "lead": model.leadState?.label ?? NSNull(),
            "viewport_text": model.viewportText ?? NSNull(),
            "transcript": model.transcript.rows.suffix(20).map { rowText($0.kind) },
            "perms": model.perms.map { ["id": $0.id, "session": $0.session, "title": $0.title] },
            "lead_error": model.leadError ?? NSNull(),
            "log_path": Log.path.path,
        ]
    }
}
#endif
