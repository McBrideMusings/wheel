#if DEBUG
import AppKit
import Network
import WheelKit

/// A loopback HTTP endpoint (Debug builds only) that answers with the app's state as JSON, so a
/// script can check what the window shows. It carries the terminal's screen, so it is guarded
/// like the daemon: the daemon's token as a bearer header, no browser Origin, loopback Host only.
///
///     curl -H "Authorization: Bearer $(cat <token file>)" http://127.0.0.1:7421/state
///
/// It also drives the UI's own code paths, so a script exercises what the buttons do:
/// `POST /lead` with the message as the body (it replaces the composer's draft, then sends it as
/// Return does), `POST /agent?session=<s>` the same for an agent pane's composer, and
/// `POST /perm?id=<id>&choice=allow|deny` (the Waiting list's buttons), and the grid's controls:
/// `POST /space?cwd=<dir>` (the tab bar's `+` after the folder picker), `/space/close?id=`,
/// `/focus?space=` or `/focus?pane=`, `/split?pane=&dir=left|right|up|down`,
/// `/start?pane=&kind=terminal|agent[&message=]` (what the picker's Terminal and Agent send), `/kind?pane=&kind=empty|terminal|web[&artifact=][&scope=repo|local|shared]` (`web` takes the artifact's name, as the picker sends it), `/ratio?split=&ratio=` (a divider drag's end; `&held=1` for one still in progress) and
/// `/pane/close?pane=`. Each replies with the state as it was before the daemon answered.
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
            model.lead.draft = text
            model.send(model.lead)
        case ("POST", "/agent"):
            guard let session = query["session"], session != AppModel.lead, let chat = model.chats[session] else {
                return refuse("404 Not Found", "no agent pane shows that session")
            }
            let text = request.range(of: "\r\n\r\n").map { String(request[$0.upperBound...]) } ?? ""
            Log.info("debug: send to \(session)")
            chat.draft = text
            model.send(chat)
        case ("POST", "/perm"):
            guard let id = query["id"], let choice = query["choice"], let perm = model.perms.first(where: { $0.id == id }) else {
                return refuse("404 Not Found", "no waiting permission with that id")
            }
            Log.info("debug: answer \(id) \(choice)")
            model.answer(perm, choice)
        case ("POST", "/config"):
            // The same call the control's menu makes, so a check drives the card's own path.
            guard let id = query["config"], let value = query["value"] else {
                return refuse("400 Bad Request", "config and value are required")
            }
            guard let option = model.lead.config.first(where: { $0.id == id }) else {
                return refuse("404 Not Found", "the lead has no config option \(id)")
            }
            Log.info("debug: set lead \(id) to \(value)")
            model.setConfig(model.lead, option, to: value)
        case ("POST", "/space"):
            guard let cwd = query["cwd"] else { return refuse("400 Bad Request", "cwd is required") }
            model.createSpace(cwd: cwd)
        case ("POST", "/space/close"):
            guard let id = query["id"] else { return refuse("400 Bad Request", "id is required") }
            model.closeSpace(id)
        case ("POST", "/focus"):
            if let pane = query["pane"] { model.focus(pane: pane) }
            else if let space = query["space"] { model.focus(space: space) }
            else { return refuse("400 Bad Request", "space or pane is required") }
        case ("POST", "/split"):
            guard let pane = query["pane"], let dir = query["dir"].flatMap(Dir.init(rawValue:)) else {
                return refuse("400 Bad Request", "pane and dir (left, right, up, down) are required")
            }
            model.split(pane, dir)
        case ("POST", "/start"):
            guard let pane = query["pane"], let kind = query["kind"].flatMap(StartKind.init(rawValue:)) else {
                return refuse("400 Bad Request", "pane and kind (terminal, agent) are required")
            }
            model.startPane(pane, kind, message: query["message"])
        case ("POST", "/kind"):
            guard let pane = query["pane"], let kind = query["kind"].flatMap(NewPaneKind.init(rawValue:)) else {
                return refuse("400 Bad Request", "pane and kind (empty, terminal, web, browser) are required")
            }
            // A scope that is not one of the three is refused rather than dropped: sending it on
            // unscoped would resolve a different root and look like a success.
            var scope: ArtifactScope?
            if let raw = query["scope"] {
                guard let parsed = ArtifactScope(rawValue: raw) else {
                    return refuse("400 Bad Request", "scope must be repo, local or shared")
                }
                scope = parsed
            }
            model.setKind(pane, kind, artifact: query["artifact"], scope: scope)
        case ("POST", "/ratio"):
            guard let split = query["split"], let ratio = query["ratio"].flatMap(Double.init) else {
                return refuse("400 Bad Request", "split and ratio are required")
            }
            // A drag's two calls: draw it there, then tell the daemon. `held=1` stops after the
            // first, as if the pointer were still down.
            model.dragRatio(split, ratio)
            if query["held"] != "1" { model.setRatio(split, ratio) }
        case ("POST", "/web/snapshot"):
            // WebKit's own render of the page, apart from whether the window shows it.
            guard let pane = query["pane"], let path = query["path"], let web = model.webPanes[pane] else {
                return refuse("400 Bad Request", "pane (a web pane) and path are required")
            }
            web.view.takeSnapshot(with: nil) { image, error in
                guard let tiff = image?.tiffRepresentation, let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) else {
                    Log.info("debug: snapshot of \(pane) failed: \(error?.localizedDescription ?? "no image")")
                    return
                }
                do {
                    try png.write(to: URL(filePath: path))
                    Log.info("debug: snapshot of \(pane) saved to \(path)")
                } catch {
                    Log.info("debug: snapshot of \(pane) not saved: \(error)")
                }
            }
        case ("POST", "/window/snapshot"):
            // The app captures its own window by its own window number, so a check never has to
            // work out where the window sits. `screencapture -R` takes global coordinates and
            // clamps a negative one, so a window on a display left of the main one captures the
            // wrong region entirely; this has no such failure.
            guard let path = query["path"] else { return refuse("400 Bad Request", "path is required") }
            guard let window = NSApp.windows.first(where: { $0.title == "Wheel" }) else {
                return refuse("404 Not Found", "no Wheel window")
            }
            guard let view = window.contentView,
                  let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
                return refuse("500 Internal Server Error", "the window could not be rendered")
            }
            view.cacheDisplay(in: view.bounds, to: rep)
            guard let png = rep.representation(using: .png, properties: [:]) else {
                return refuse("500 Internal Server Error", "the render could not be encoded")
            }
            do {
                try png.write(to: URL(filePath: path))
                Log.info("debug: window snapshot saved to \(path), \(Int(rep.size.width))x\(Int(rep.size.height))")
            } catch {
                return refuse("500 Internal Server Error", "could not write \(path): \(error)")
            }
        case ("POST", "/window/front"), ("POST", "/window/back"):
            // Uncover the window for a capture without activating the app or taking the keyboard:
            // WebKit draws nothing while the window is fully covered.
            guard let window = NSApp.windows.first(where: { $0.title == "Wheel" }) else { return refuse("404 Not Found", "no Wheel window") }
            if target.path == "/window/front" { window.orderFrontRegardless() } else { window.orderBack(nil) }
            Log.info("debug: window \(target.path == "/window/front" ? "in front" : "sent back")")
        case ("POST", "/pane/close"):
            guard let pane = query["pane"] else { return refuse("400 Bad Request", "pane is required") }
            model.closePane(pane)
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
        case let .user(text, queued): queued ? "user (queued): \(text)" : "user: \(text)"
        case let .agent(text): "agent: \(text)"
        case let .tool(title, _, status, content): (["tool: \(title) (\(status))"] + content.map(contentText)).joined(separator: " · ")
        case let .plan(entries): "plan: \(entries.map(\.content).joined(separator: "; "))"
        case let .notice(text): "notice: \(text)"
        case let .error(text): "error: \(text)"
        }
    }

    /// One tool content item with its text quoted and escaped, so a check can compare exact values.
    private func contentText(_ item: ToolContent) -> String {
        switch item {
        case let .text(text): "text \(String(reflecting: text))"
        case let .diff(path, old, new): "diff \(path): \(old.map { String(reflecting: $0) } ?? "new file") -> \(String(reflecting: new))"
        case let .terminal(id): "terminal \(id)"
        case let .other(type): "unknown \(type)"
        }
    }

    /// The tree as the daemon sent it; an unknown pane carries every field it came with.
    private func node(_ node: Node) -> [String: Any] {
        switch node {
        case let .split(id, axis, ratio, a, b):
            return ["node": "split", "id": id, "axis": axis.rawValue, "ratio": ratio, "a": self.node(a), "b": self.node(b)]
        case let .pane(id, kind, createdBy):
            var o: [String: Any] = switch kind {
            case .empty: ["kind": "empty"]
            case let .terminal(term): ["kind": "terminal", "term": term]
            case let .web(artifact): ["kind": "web", "scope": artifact.scope.rawValue, "name": artifact.name]
            case let .browser(url): ["kind": "browser", "url": url]
            case let .agent(session): ["kind": "agent", "session": session]
            case let .unknown(_, raw): raw.mapValues(\.foundation)
            }
            o["node"] = "pane"
            o["id"] = id
            o["created_by"] = createdBy.rawValue
            return o
        }
    }

    /// What each pane shows right now, keyed by pane id: for a terminal, whether it is attached,
    /// whether its surface draws, the grid size it reported and its visible text; for a web pane,
    /// the artifact's scope and name, its page address (without the token), when it last loaded and
    /// reloaded, the load's HTTP status, and the page's title and visible text; for an agent pane, its session, that
    /// session's state, its last 20 transcript rows, its composer's draft and the error under it.
    private func panes() -> [String: Any] {
        var out: [String: Any] = [:]
        for space in model.spaces {
            for pane in space.root.panes {
                var o: [String: Any] = ["space": space.id, "kind": pane.kind.name]
                if case let .terminal(term) = pane.kind, let terminal = model.terminals[term] {
                    o["term"] = term
                    o["attached"] = terminal.attached
                    o["visible"] = terminal.view.isSurfaceVisible
                    o["cols"] = terminal.size?.cols ?? NSNull()
                    o["rows"] = terminal.size?.rows ?? NSNull()
                    o["viewport_text"] = terminal.viewportText ?? NSNull()
                }
                if case let .agent(session) = pane.kind {
                    let chat = model.chats[session]
                    o["session"] = session
                    o["state"] = chat?.state?.label ?? NSNull()
                    o["transcript"] = chat.map { $0.transcript.rows.suffix(20).map { rowText($0.kind) } } ?? NSNull()
                    o["composer"] = chat?.draft ?? NSNull()
                    o["error"] = chat?.error ?? NSNull()
                }
                if case let .web(artifact) = pane.kind, let web = model.webPanes[pane.id] {
                    let time = { (date: Date?) -> Any in date.map { Self.iso8601.string(from: $0) } ?? NSNull() }
                    o["scope"] = artifact.scope.rawValue
                    o["artifact"] = artifact.name
                    o["url"] = web.address ?? NSNull()
                    o["last_reload_at"] = time(web.lastReloadAt)
                    o["loaded_at"] = time(web.loadedAt)
                    o["status"] = web.status ?? NSNull()
                    o["title"] = web.title
                    o["text"] = web.text ?? NSNull()
                    o["load_error"] = web.loadError ?? NSNull()
                    o["in_window"] = web.view.window != nil
                    o["view_size"] = [web.view.frame.width, web.view.frame.height]
                }
                out[pane.id] = o
            }
        }
        return out
    }

    private static let iso8601 = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    private func state() -> [String: Any] {
        let connection: String = switch model.connection {
        case .connected: "connected"
        case .connecting: "connecting"
        case let .unreachable(reason): "unreachable: \(reason)"
        }
        return [
            "daemon_url": model.endpoint.url.absoluteString,
            "daemon_service": model.service.map {
                ["label": $0.label, "instance": $0.instance, "listen": $0.listen, "status": $0.status] as [String: Any]
            } ?? NSNull(),
            "connection": connection,
            "spaces": model.spaces.map { space in
                ["id": space.id, "name": space.name, "cwd": space.cwd, "focus": space.focus, "root": node(space.root)] as [String: Any]
            },
            "active": model.active ?? NSNull(),
            "focus_key": model.focusKey ?? NSNull(),
            "panes": panes(),
            "layout_error": model.layoutError ?? NSNull(),
            "drag_ratios": model.dragRatios,
            "lead": model.lead.state?.label ?? NSNull(),
            "transcript": model.lead.transcript.rows.suffix(20).map { rowText($0.kind) },
            "perms": model.perms.map {
                ["id": $0.id, "session": $0.session, "title": $0.title, "tool_call": $0.toolCall ?? NSNull()] as [String: Any]
            },
            "composer": model.lead.draft,
            "lead_error": model.lead.error ?? NSNull(),
            "lead_config": configJSON(model.lead.config),
            "log_path": Log.path.path,
        ]
    }

    /// The lead's config controls as the card draws them: what each shows, and what it offers.
    private func configJSON(_ options: [ConfigOption]) -> [[String: Any]] {
        options.map { option in
            var o: [String: Any] = [
                "id": option.id,
                "name": option.name,
                "category": option.category ?? NSNull(),
                "shows": option.currentLabel ?? NSNull(),
            ]
            switch option.kind {
            case let .select(current, choices):
                o["type"] = "select"
                o["current"] = current
                o["choices"] = choices.map(\.value)
            case let .boolean(current):
                o["type"] = "boolean"
                o["current"] = current
            case .unknown:
                o["type"] = "unknown"
            }
            return o
        }
    }
}
#endif
