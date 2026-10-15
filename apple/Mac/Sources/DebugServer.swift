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
/// Return does), `POST /agent?session=<s>` the same for an agent pane's composer,
/// `POST /agent/focus?session=<s>` (the keyboard goes into that composer, as a click would put it),
/// `POST /perm?id=<id>&choice=allow|deny` (the Waiting list's buttons), and the grid's controls:
/// `POST /space?cwd=<dir>` (the tab bar's `+` after the folder picker), `/space/close?id=`,
/// `/focus?space=` or `/focus?pane=`, `/split?pane=&dir=left|right|up|down`,
/// `/start?pane=&kind=terminal|agent[&message=]` (what the picker's Terminal and Agent send), `/kind?pane=&kind=empty|terminal|web[&artifact=][&scope=repo|local|shared]` (`web` takes the artifact's name, as the picker sends it), `/ratio?split=&ratio=` (a divider drag's end; `&held=1` for one still in progress) and
/// `/pane/close?pane=`. A browser pane's chrome: `/browser?pane=&action=back|forward|reload|stop|home`,
/// `&action=edit[&text=]` (a click into the address field, then typing), `&action=open&text=`
/// (typing into the field and pressing Return) and `&action=cancel` (Escape in the field). The
/// Settings window: `/settings?action=open|close`, `&action=tab&tab=global|project`,
/// `&action=project&id=<space>` (a row of the Project tab's list), and per field
/// `&action=edit&layer=global|project&key=&text=`, `&action=save&layer=&key=` (Return),
/// `&action=cancel&layer=&key=` (Esc) and `&action=clear&layer=&key=` (the ✕ button);
/// `/window/front?which=settings` uncovers it. Each
/// replies with the state as it was before the daemon answered.
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

    /// Most a request may be, head and body: the debug endpoint's bodies are chat messages.
    private static let maxRequest = 1 << 20

    private func serve(_ connection: NWConnection) {
        connection.start(queue: .main)
        read(connection, buffered: Data())
    }

    /// Reads until the head and `Content-Length` bytes of body have arrived, however many TCP
    /// segments that takes, then answers. A connection that ends first gets what it sent.
    private func read(_ connection: NWConnection, buffered: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, isComplete, error in
            Task { @MainActor in
                guard let self else { return }
                let buffer = buffered + (data ?? Data())
                var parsed: (head: String, body: Data, length: Int)?
                if let end = buffer.range(of: Data("\r\n\r\n".utf8)) {
                    let head = String(decoding: buffer[..<end.lowerBound], as: UTF8.self)
                    let length = Self.headers(head)["content-length"].flatMap(Int.init) ?? 0
                    parsed = (head, Data(buffer[end.upperBound...].prefix(max(length, 0))), length)
                }
                if buffer.count > Self.maxRequest || (parsed?.length ?? 0) > Self.maxRequest {
                    Log.info("debug request refused: \(buffer.count) bytes read, body of \(parsed?.length ?? 0) declared")
                    return self.reply(connection, "413 Payload Too Large", Data(#"{"error":"request too large"}"#.utf8))
                }
                if let parsed, parsed.body.count >= parsed.length {
                    let (status, body) = self.respond(head: parsed.head, body: String(decoding: parsed.body, as: UTF8.self))
                    return self.reply(connection, status, body)
                }
                if isComplete || error != nil {
                    Log.info("debug request refused: connection ended before the head and body arrived")
                    return self.reply(connection, "400 Bad Request", Data(#"{"error":"incomplete request"}"#.utf8))
                }
                self.read(connection, buffered: buffer)
            }
        }
    }

    private func reply(_ connection: NWConnection, _ status: String, _ body: Data) {
        let head = "HTTP/1.1 \(status)\r\nContent-Type: application/json\r\nContent-Length: \(body.count)\r\nConnection: close\r\n\r\n"
        connection.send(content: Data(head.utf8) + body, completion: .contentProcessed { _ in connection.cancel() })
    }

    /// The header lines of a request head (everything before the blank line), lowercased by name.
    private static func headers(_ head: String) -> [String: String] {
        var headers: [String: String] = [:]
        for line in head.split(separator: "\r\n").dropFirst() {
            guard let colon = line.firstIndex(of: ":") else { continue }
            headers[line[..<colon].lowercased()] = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        }
        return headers
    }

    private func respond(head request: String, body text: String) -> (String, Data) {
        let headers = Self.headers(request)
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
            Log.info("debug: lead send")
            model.lead.draft = text
            model.send(model.lead)
        case ("POST", "/lead/expand"):
            // Open or close a tool line, tool group or thought by its id from `transcript_items`:
            // the click on its line.
            guard let id = query["id"].flatMap(Int.init) else { return refuse("400 Bad Request", "id is required") }
            model.lead.toggle(id)
        case ("POST", "/lead/draft"):
            // Text in the lead's composer, not sent: what a person typing sees.
            Log.info("debug: lead draft, \(text.count) chars")
            model.lead.draft = text
        case ("POST", "/agent"):
            guard let session = query["session"], session != AppModel.lead, let chat = model.chats[session] else {
                return refuse("404 Not Found", "no agent pane shows that session")
            }
            Log.info("debug: send to \(session)")
            chat.draft = text
            model.send(chat)
        case ("POST", "/agent/focus"):
            guard let session = query["session"], session != AppModel.lead, let chat = model.chats[session] else {
                return refuse("404 Not Found", "no agent pane shows that session")
            }
            Log.info("debug: focus the composer of \(session)")
            chat.composerFocusRequest += 1
        case ("POST", "/perm"):
            guard let id = query["id"], let choice = query["choice"], let perm = model.perms.first(where: { $0.id == id }) else {
                return refuse("404 Not Found", "no waiting permission with that id")
            }
            Log.info("debug: answer \(id) \(choice)")
            model.answer(perm, choice)
        case ("POST", "/lead/scroll"):
            guard let to = query["to"], to == "top" || to == "bottom" else {
                return refuse("400 Bad Request", "to (top, bottom) is required")
            }
            Log.info("debug: scroll lead transcript to \(to)")
            model.scrollLead(toTop: to == "top")
        case ("POST", "/scroll"):
            // Put a transcript's view a given distance above its end by moving its scroll view's
            // clip view: the scroll a person's wheel makes, which no SwiftUI call can place.
            // `session` is `lead` or an agent pane's session.
            guard let session = query["session"], let fromEnd = query["from_end"].flatMap(Double.init) else {
                return refuse("400 Bad Request", "session and from_end are required")
            }
            guard let chat = session == AppModel.lead ? model.lead : model.chats[session] else {
                return refuse("404 Not Found", "no transcript shows that session")
            }
            guard let window = NSApp.windows.first(where: { $0.title == "Wheel" }), let content = window.contentView,
                  chat.frameInWindow != .zero else {
                return refuse("404 Not Found", "that transcript is not on screen")
            }
            let frame = chat.frameInWindow
            let point = NSPoint(x: frame.midX, y: content.isFlipped ? frame.midY : content.bounds.height - frame.midY)
            // SwiftUI content is not made of views, so a hit test stops at the hosting view: look
            // through the window's scroll views for the smallest one under the transcript's middle.
            func scrollViews(in view: NSView) -> [NSScrollView] {
                view.subviews.flatMap { ($0 as? NSScrollView).map { [$0] } ?? [] + scrollViews(in: $0) }
                    + view.subviews.compactMap { $0 as? NSScrollView }.flatMap { scrollViews(in: $0) }
            }
            let under = scrollViews(in: content).filter { $0.convert($0.bounds, to: content).contains(point) }
            guard let scroll = under.min(by: { $0.bounds.width * $0.bounds.height < $1.bounds.width * $1.bounds.height }),
                  let document = scroll.documentView else {
                return refuse("404 Not Found", "no scroll view under that transcript (\(scrollViews(in: content).count) in the window)")
            }
            let clip = scroll.contentView
            let top = max(0, document.frame.height - clip.bounds.height - fromEnd)
            let y = document.isFlipped ? top : document.frame.height - clip.bounds.height - top
            // Marked as a person's scroll while it lands, so the transcript decides whether it
            // still follows its end the way it does for one.
            chat.userScrolling = true
            clip.scroll(to: NSPoint(x: clip.bounds.origin.x, y: y))
            scroll.reflectScrolledClipView(clip)
            DispatchQueue.main.async { chat.userScrolling = false }
            Log.info("debug: scroll \(session) to \(Int(fromEnd)) from its end (content \(Int(document.frame.height)), visible \(Int(clip.bounds.height)))")
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
        case ("POST", "/browser"):
            guard let pane = query["pane"], let browser = model.browserPanes[pane] else {
                return refuse("404 Not Found", "no browser pane with that id")
            }
            let action = query["action"] ?? ""
            Log.info("debug: browser \(pane) \(action)")
            switch action {
            case "back": browser.back()
            case "forward": browser.forward()
            case "reload": browser.reload()
            case "stop": browser.stop()
            case "home": model.goHome(pane)
            case "edit": browser.editing = query["text"] ?? AddressField.editable(browser.address)
            case "cancel": browser.editing = nil
            case "open":
                guard let text = query["text"] else { return refuse("400 Bad Request", "open takes text") }
                browser.open(typed: text)
            default:
                return refuse("400 Bad Request", "action must be back, forward, reload, stop, home, edit, cancel or open")
            }
        case ("POST", "/settings"):
            // The Settings window's controls: open (the app menu's Settings…), close (its close
            // button), tab (the toolbar), project (a row of the Project tab's list), and per field
            // edit (typing), save (Return), cancel (Esc) and clear (the ✕ button).
            let action = query["action"] ?? ""
            Log.info("debug: settings \(action)")
            switch action {
            case "open": model.settings.openRequest += 1
            case "close": Self.settingsWindow?.close()
            case "tab":
                guard let tab = query["tab"].flatMap(SettingLayer.init(rawValue:)) else {
                    return refuse("400 Bad Request", "tab takes tab=global or tab=project")
                }
                model.settings.tab = tab
            case "project":
                guard let id = query["id"], model.spaces.contains(where: { $0.id == id }) else {
                    return refuse("404 Not Found", "no space with that id")
                }
                model.selectSettingsProject(id)
            case "edit", "save", "cancel", "clear":
                guard let layer = query["layer"].flatMap(SettingLayer.init(rawValue:)) else {
                    return refuse("400 Bad Request", "layer=global or layer=project is required")
                }
                guard let key = query["key"], model.settings.setting(key) != nil else {
                    return refuse("404 Not Found", "the window shows no setting with that key")
                }
                let field = model.settings.field(layer, key)
                switch action {
                case "edit":
                    guard let text = query["text"] else { return refuse("400 Bad Request", "edit takes text") }
                    model.settings.type(field, text)
                case "save": model.saveSetting(field)
                case "clear": model.clearSetting(field)
                default: model.settings.revert(field)
                }
            default:
                return refuse("400 Bad Request", "action must be open, close, tab, project, edit, save, cancel or clear")
            }
        case ("POST", "/web/snapshot"):
            // WebKit's own render of the page, apart from whether the window shows it.
            guard let pane = query["pane"], let path = query["path"],
                  let view = model.webPanes[pane]?.view ?? model.browserPanes[pane]?.view else {
                return refuse("400 Bad Request", "pane (a web or browser pane) and path are required")
            }
            view.takeSnapshot(with: nil) { image, error in
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
            // `which=settings` names the Settings window instead of the main one.
            let found = query["which"] == "settings" ? Self.settingsWindow : NSApp.windows.first(where: { $0.title == "Wheel" })
            guard let window = found else { return refuse("404 Not Found", "no such window") }
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

    /// A row as one line; a tool row a request gates ends with `· waiting perm <id>`.
    private func rowText(_ row: TranscriptRow, session: String) -> String {
        var text = rowText(row.kind)
        // What the tool or thought line shows of its time: running, and for how long.
        switch row.kind {
        case .tool, .thought:
            if row.open { text += " · running" }
            if let seconds = row.elapsed(now: .now) { text += " · \(formatElapsed(seconds))" }
        default: break
        }
        guard let call = row.toolCall, let perm = model.perm(session: session, toolCall: call) else { return text }
        return "\(text) · waiting perm \(perm.id)"
    }

    /// The transcript as laid out, with the ids `/lead/expand` takes: `<kind> <id>` for a row drawn
    /// alone, `tools <n> @<id> [<ids>]` for a group of n calls; `open` after one the person opened.
    private func itemsText(_ chat: SessionChat) -> [String] {
        let open = { (id: Int) in chat.expanded.contains(id) ? " open" : "" }
        return chat.items.map { item in
            switch item {
            case let .row(row): "\(rowText(row.row.kind).prefix { $0 != ":" && $0 != " " }) \(row.id)\(open(row.id))"
            case let .tools(group):
                "tools \(group.rows.count) @\(group.openKey)\(open(group.openKey)) [\(group.rows.map { "\($0.id)\(open($0.id))" }.joined(separator: ","))]"
            }
        }
    }

    private func rowText(_ kind: TranscriptRow.Kind) -> String {
        switch kind {
        case let .user(text, queued): queued ? "user (queued): \(text)" : "user: \(text)"
        case let .agent(text): "agent: \(text)"
        case let .thought(text): "thought: \(text)"
        case let .block(kind, name): "block: \(kind)" + (name.map { " \($0)" } ?? "")
        case let .tool(title, _, status, content): (["tool: \(title) (\(status))"] + content.map(contentText)).joined(separator: " · ")
        case let .plan(entries): "plan: \(entries.map(\.content).joined(separator: "; "))"
        case let .mode(mode): "mode: \(mode)"
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
    /// reloaded, the load's HTTP status, and the page's title and visible text; for a browser pane,
    /// the page the daemon holds and the page the view is on, its title, load progress, history
    /// buttons, whether the chrome is narrow, the field's text while edited and any failure; for an agent pane, its session, that
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
                    o["state"] = chat.map { model.stateLabel($0) } ?? NSNull()
                    o["transcript"] = chat.map { chat in chat.transcript.rows.suffix(20).map { rowText($0, session: session) } } ?? NSNull()
                    o["transcript_items"] = chat.map { Array(itemsText($0).suffix(20)) } ?? NSNull()
                    o["composer"] = chat?.draft ?? NSNull()
                    o["from_end"] = chat.map { Int($0.fromEnd) } ?? NSNull()
                    o["pinned"] = chat?.pinned ?? NSNull()
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
                if case let .browser(daemonURL) = pane.kind, let browser = model.browserPanes[pane.id] {
                    let time = { (date: Date?) -> Any in date.map { Self.iso8601.string(from: $0) } ?? NSNull() }
                    o["daemon_url"] = daemonURL
                    o["url"] = browser.url
                    o["title"] = browser.title
                    o["loading"] = browser.loading
                    o["progress"] = browser.progress
                    o["can_go_back"] = browser.canGoBack
                    o["can_go_forward"] = browser.canGoForward
                    o["narrow"] = browser.narrow
                    o["editing"] = browser.editing ?? NSNull()
                    o["failure"] = browser.failure.map { ["address": $0.address, "message": $0.message] } ?? NSNull()
                    o["status"] = browser.status ?? NSNull()
                    o["loaded_at"] = time(browser.loadedAt)
                    o["text"] = browser.text ?? NSNull()
                    o["in_window"] = browser.view.window != nil
                    o["view_size"] = [browser.view.frame.width, browser.view.frame.height]
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
            "lead": model.stateLabel(model.lead),
            "transcript": model.lead.transcript.rows.suffix(20).map { rowText($0, session: AppModel.lead) },
            "transcript_items": Array(itemsText(model.lead).suffix(20)),
            "transcript_redraws": [
                "rows": model.lead.rows.count,
                "list_updates": model.lead.listUpdates,
                "row_updates": model.lead.rowUpdates,
                "row_draws": model.lead.rows.reduce(0) { $0 + $1.draws },
            ],
            "perms": model.perms.map {
                ["id": $0.id, "session": $0.session, "title": $0.title, "tool_call": $0.toolCall ?? NSNull()] as [String: Any]
            },
            "waiting_count": model.perms.count,
            "waiting_box": model.boxPerms.map { $0.id },
            "request_card": model.requestCardShown.map {
                ["tool_call": $0.call, "content": $0.content.map(contentText)] as [String: Any]
            } ?? NSNull(),
            "inline_perm": model.inlinePerm?.id ?? NSNull(),
            "lead_rows_visible": model.visibleLeadCalls.sorted(),
            "composer": model.lead.draft,
            "lead_error": model.lead.error ?? NSNull(),
            "lead_pinned": model.lead.pinned,
            "lead_from_end": Int(model.lead.fromEnd),
            "lead_config": configJSON(model.lead.config),
            "settings": settingsJSON(),
            "log_path": Log.path.path,
        ]
    }

    /// The SwiftUI Settings scene's window, found by the identifier SwiftUI gives it.
    static var settingsWindow: NSWindow? {
        NSApp.windows.first { $0.identifier?.rawValue.contains("Settings") == true }
    }

    /// The Settings window as it draws: whether it is up and on which tab, which project the
    /// Project tab edits and which space the rows were resolved for, and per key what the daemon
    /// reported with, per layer, the field's text, the value it shows greyed when its file sets
    /// nothing, whether it is unsaved, the refusal under it and whether a change is out.
    private func settingsJSON() -> [String: Any] {
        let s = model.settings
        return [
            "open": s.open,
            "window_title": Self.settingsWindow?.title ?? NSNull(),
            "tab": s.tab.rawValue,
            "project": s.project ?? NSNull(),
            "space": s.space ?? NSNull(),
            "loaded": s.loaded,
            "list_error": s.listError ?? NSNull(),
            "rows": s.settings.map { setting in
                var fields: [String: Any] = [:]
                for layer in SettingLayer.allCases {
                    let field = s.field(layer, setting.key)
                    fields[layer.rawValue] = [
                        "text": s.text(field),
                        "placeholder": s.inherited(field),
                        "set": s.stored(field) != nil,
                        "dirty": s.dirty(field),
                        "refusal": s.refusals[field] ?? NSNull(),
                        "saving": s.saving.contains(field),
                    ] as [String: Any]
                }
                return [
                    "key": setting.key,
                    "value": setting.value,
                    "origin": setting.origin.rawValue,
                    "default": setting.default,
                    "machine": setting.machine ?? NSNull(),
                    "project": setting.project ?? NSNull(),
                    "help": setting.help,
                    "problem": setting.problem ?? NSNull(),
                    "fields": fields,
                ] as [String: Any]
            },
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
