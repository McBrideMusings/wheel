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
/// It also drives the UI's own code paths, so a script exercises what the buttons do. An agent
/// pane, by `session=<s>`: `POST /agent` with the message as the body (it replaces the composer's
/// draft, then sends it as Return does), `/agent/draft` (the text, not sent), `/agent/focus` (the
/// keyboard goes into that composer, as a click would put it), `/agent/expand?id=` (a click on a
/// tool line, tool group or thought), `/agent/notice/dismiss` (the banner's close button),
/// `/agent/key?key=` (a scrolling key) and `/agent/config?config=&value=` (a control's menu);
/// `POST /perm?id=<id>&choice=allow|deny` (the request card's buttons), and the grid's controls:
/// `POST /space?cwd=<dir>` (the tab bar's `+` after the folder picker), `/space/close?id=`,
/// `/focus?space=` or `/focus?pane=`, `/split?pane=&dir=left|right|up|down`,
/// `/start?pane=&kind=terminal|agent[&message=]` (what the picker's Terminal and Agent send), `/kind?pane=&kind=empty|terminal|web[&artifact=][&scope=project|machine]` (`web` takes the artifact's name, as the picker sends it), `/ratio?split=&ratio=` (a divider drag's end; `&held=1` for one still in progress) and
/// `/pane/close?pane=`. A browser pane's chrome: `/browser?pane=&action=back|forward|reload|stop|home`,
/// `&action=edit[&text=]` (a click into the address field, then typing), `&action=open&text=`
/// (typing into the field and pressing Return) and `&action=cancel` (Escape in the field). The
/// Settings window: `/settings?action=open|close`, `&action=tab&tab=global|project`,
/// `&action=project&id=<space>` (a row of the Project tab's list), and per field
/// `&action=edit&layer=global|project&key=&text=`, `&action=save&layer=&key=` (Return),
/// `&action=cancel&layer=&key=` (Esc) and `&action=clear&layer=&key=` (the ✕ button);
/// `/window/front?which=settings` uncovers it. A pane's artifact palette: `/palette?pane=&action=open|type|move|enter|escape|click|confirm|close`
/// (`&text=`, `&by=`, `&scope=&name=`); an open one shows in that pane's `palette` entry of the state. Each
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
        case ("POST", "/agent"), ("POST", "/agent/draft"), ("POST", "/agent/focus"), ("POST", "/agent/expand"),
             ("POST", "/agent/notice/dismiss"), ("POST", "/agent/key"), ("POST", "/agent/config"):
            guard let session = query["session"], let chat = model.chats[session] else {
                return refuse("404 Not Found", "no agent pane shows that session")
            }
            switch target.path {
            case "/agent":
                Log.info("debug: send to \(session)")
                chat.draft = text
                model.send(chat)
            case "/agent/draft":
                // Text in the composer, not sent: what a person typing sees.
                Log.info("debug: draft for \(session), \(text.count) chars")
                chat.draft = text
            case "/agent/focus":
                Log.info("debug: focus the composer of \(session)")
                chat.composerFocusRequest += 1
            case "/agent/expand":
                // Open or close a tool line, tool group or thought by its id from `transcript_items`:
                // the click on its line.
                guard let id = query["id"].flatMap(Int.init) else { return refuse("400 Bad Request", "id is required") }
                chat.toggle(id)
            case "/agent/notice/dismiss":
                Log.info("debug: notice dismiss over \(session)")
                model.dismissNotice(chat)
            case "/agent/key":
                // Press a scrolling key the way a keyboard does: a key event through the
                // application's event path, so the transcript's key monitor sees it, delivered to
                // the first responder that is the transcript's scroll view. Unlike /scroll, it does
                // not mark a scroll under way, so the unpin it causes is the key path's own.
                guard let key = query["key"].flatMap(ScrollKey.init(rawValue:)) else {
                    return refuse("400 Bad Request", "key (\(ScrollKey.allCases.map(\.rawValue).joined(separator: ", "))) is required")
                }
                let found = transcriptScrollView(chat)
                guard case let .success((window, scroll)) = found else {
                    if case let .failure(refusal) = found { return refuse(refusal.status, refusal.message) }
                    return refuse("404 Not Found", "no scroll view under that transcript")
                }
                guard let event = NSEvent.keyEvent(
                    with: .keyDown, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                    windowNumber: window.windowNumber, context: nil, characters: key.characters, charactersIgnoringModifiers: key.characters,
                    isARepeat: false, keyCode: key.keyCode) else {
                    return refuse("500 Internal Server Error", "could not make a key event")
                }
                let focused = window.makeFirstResponder(scroll.documentView ?? scroll)
                Log.info("debug: key \(key.rawValue) to \(session)'s transcript (first responder taken: \(focused))")
                NSApp.sendEvent(event)
            default:
                // The same call the control's menu makes, so a check drives the pane's own path.
                guard let id = query["config"], let value = query["value"] else {
                    return refuse("400 Bad Request", "config and value are required")
                }
                guard let option = chat.config.first(where: { $0.id == id }) else {
                    return refuse("404 Not Found", "\(session) has no config option \(id)")
                }
                Log.info("debug: set \(session) \(id) to \(value)")
                model.setConfig(chat, option, to: value)
            }
        case ("POST", "/perm"):
            guard let id = query["id"], let choice = query["choice"], let perm = model.perms.first(where: { $0.id == id }) else {
                return refuse("404 Not Found", "no waiting permission with that id")
            }
            Log.info("debug: answer \(id) \(choice)")
            model.answer(perm, choice)
        case ("POST", "/scroll"):
            // Put a transcript's view a given distance above its end by moving its scroll view's
            // clip view: the scroll a person's wheel makes, which no SwiftUI call can place.
            // `session` is an agent pane's session.
            guard let session = query["session"], let fromEnd = query["from_end"].flatMap(Double.init) else {
                return refuse("400 Bad Request", "session and from_end are required")
            }
            guard let chat = model.chats[session] else {
                return refuse("404 Not Found", "no transcript shows that session")
            }
            let found = transcriptScrollView(chat)
            guard case let .success((_, scroll)) = found, let document = scroll.documentView else {
                if case let .failure(refusal) = found { return refuse(refusal.status, refusal.message) }
                return refuse("404 Not Found", "no scroll view under that transcript")
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
        case ("POST", "/transcript/select"):
            // Select text across rows and copy it. `session` is an agent pane's session; `from_row`
            // and `to_row` are row ids from `transcript_items`, `from_offset` and `to_offset`
            // characters into the row's plain text. Rows need not be on screen; `mouse=1` drives the
            // transcript's mouse handlers at the offsets' window points instead (both rows built).
            // The answer is a JSON object, not the state.
            guard let session = query["session"],
                  let fromRow = query["from_row"].flatMap(Int.init), let fromOffset = query["from_offset"].flatMap(Int.init),
                  let toRow = query["to_row"].flatMap(Int.init), let toOffset = query["to_offset"].flatMap(Int.init) else {
                return refuse("400 Bad Request", "session, from_row, from_offset, to_row and to_offset are required")
            }
            guard let chat = model.chats[session] else { return refuse("404 Not Found", "no agent pane shows that session") }
            return selectAndCopy(chat, from: (fromRow, fromOffset), to: (toRow, toOffset), mouse: query["mouse"] == "1")
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
            // A scope that is not one of the two is refused rather than dropped: sending it on
            // unscoped would resolve a different root and look like a success.
            var scope: ArtifactScope?
            if let raw = query["scope"] {
                guard let parsed = ArtifactScope(rawValue: raw) else {
                    return refuse("400 Bad Request", "scope must be project or machine")
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
        case ("POST", "/palette"):
            // The artifact palette's controls, as the keys and clicks drive them: open (the
            // "Artifact…" button), type (text into the field), move (by=±n, the arrows), enter
            // (Return: open the selected row, or list again after a failure), escape, click (a row,
            // by scope and name), confirm (the yes asked before a shell or agent is replaced) and
            // close (a click on the dimmed body).
            guard let pane = query["pane"], model.spaces.contains(where: { $0.root.panes.contains { $0.id == pane } }) else {
                return refuse("404 Not Found", "no pane with that id")
            }
            let action = query["action"] ?? ""
            Log.info("debug: palette \(pane) \(action)")
            if action != "open", model.palettes[pane] == nil { return refuse("409 Conflict", "that pane has no palette open") }
            switch action {
            case "open": model.openPalette(pane)
            case "type":
                guard let text = query["text"] else { return refuse("400 Bad Request", "type takes text") }
                model.paletteTyped(pane, text)
            case "move":
                guard let by = query["by"].flatMap(Int.init) else { return refuse("400 Bad Request", "move takes by=<n>") }
                model.paletteMoved(pane, by: by)
            case "enter": model.paletteEntered(pane)
            case "escape": model.paletteEscaped(pane)
            case "close": model.closePalette(pane)
            case "confirm":
                guard let info = model.palettes[pane]?.confirming else { return refuse("409 Conflict", "nothing waits on a yes") }
                model.showArtifact(pane, info)
            case "click":
                guard let scope = query["scope"], let name = query["name"],
                      let info = model.palettes[pane]?.state.rows.first(where: { $0.info.scope.rawValue == scope && $0.info.name == name })?.info else {
                    return refuse("404 Not Found", "the palette lists no such row; click takes scope and name")
                }
                model.paletteChose(pane, info)
            default:
                return refuse("400 Bad Request", "action must be open, type, move, enter, escape, click, confirm or close")
            }
        case ("POST", "/usage"):
            // The Usage window and the strip: open and close the window, refresh, pin or unpin a
            // row (`row=claude:weekly`), show or hide an agent (`agent=codex`).
            let action = query["action"] ?? ""
            Log.info("debug: usage \(action)")
            switch action {
            case "open": model.usage.openRequest += 1
            case "close": NSApp.windows.first(where: { $0.title == "Usage" })?.close()
            case "refresh": model.askUsage(dueOnly: query["due_only"] == "true")
            case "pin", "unpin":
                guard let row = query["row"] else { return refuse("400 Bad Request", "row=<agent>:<limit> is required") }
                if model.usage.board.isPinned(row) == (action == "unpin") { model.togglePin(row) }
            case "agent":
                guard let agent = query["agent"], model.usage.board.agents.contains(where: { $0.id == agent }) else {
                    return refuse("404 Not Found", "agent takes the id of an agent the board lists")
                }
                let on = query["on"] != "false"
                if model.usage.board.agents.first(where: { $0.id == agent })?.shown != on { model.toggleUsageAgent(agent) }
            default:
                return refuse("400 Bad Request", "action must be open, close, refresh, pin, unpin or agent")
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
            case "install": model.settings.commandLine.install()
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
                return refuse("400 Bad Request", "action must be open, close, tab, project, install, edit, save, cancel or clear")
            }
        case ("POST", "/window/snapshot"):
            // The app captures its own window by its own window number, so a check never has to
            // work out where the window sits. `screencapture -R` takes global coordinates and
            // clamps a negative one, so a window on a display left of the main one captures the
            // wrong region entirely; this has no such failure.
            guard let path = query["path"] else { return refuse("400 Bad Request", "path is required") }
            // `which=sheet` captures a sheet over the window, which the window's own content view
            // does not draw.
            let sheet = query["which"] == "sheet"
            let title = query["which"] == "usage" ? "Usage" : "Wheel"
            let named = query["which"] == "settings" ? Self.settingsWindow : NSApp.windows.first(where: { $0.title == title })
            guard let window = sheet ? NSApp.windows.first(where: \.isSheet) : named else {
                return refuse("404 Not Found", sheet ? "no sheet is open" : "no Wheel window")
            }
            guard let view = window.contentView,
                  let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
                return refuse("500 Internal Server Error", "the window could not be rendered")
            }
            view.cacheDisplay(in: view.bounds, to: rep)
            var out = rep
            if sheet {
                // A sheet draws its background in the window, not in the content view: lay the
                // content over the window colour in the sheet's own appearance.
                let flat = NSImage(size: rep.size)
                flat.lockFocus()
                view.effectiveAppearance.performAsCurrentDrawingAppearance {
                    NSColor.windowBackgroundColor.setFill()
                    NSRect(origin: .zero, size: rep.size).fill()
                }
                let content = NSImage(size: rep.size)
                content.addRepresentation(rep)
                content.draw(in: NSRect(origin: .zero, size: rep.size), from: .zero, operation: .sourceOver, fraction: 1)
                flat.unlockFocus()
                if let tiff = flat.tiffRepresentation, let flatRep = NSBitmapImageRep(data: tiff) { out = flatRep }
            }
            guard let png = out.representation(using: .png, properties: [:]) else {
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
            // `which=usage` names the Usage window.
            let found = switch query["which"] {
            case "settings": Self.settingsWindow
            case "usage": NSApp.windows.first(where: { $0.title == "Usage" })
            default: NSApp.windows.first(where: { $0.title == "Wheel" })
            }
            guard let window = found else { return refuse("404 Not Found", "no such window") }
            if target.path == "/window/front" { window.orderFrontRegardless() } else { window.orderBack(nil) }
            Log.info("debug: window \(target.path == "/window/front" ? "in front" : "sent back")")
        case ("POST", "/pane/drag"):
            // A header drag without a pointer: `action` is start (`pane`), hover (`target` and
            // `zone`: center, left, right, up or down), drop or cancel. The same model calls the
            // pointer's gesture makes.
            Log.info("debug: pane drag \(query["action"] ?? "-")")
            switch query["action"] {
            case "start":
                let held = Set(model.spaces.flatMap { $0.root.panes.map(\.id) })
                guard let pane = query["pane"], held.contains(pane) else { return refuse("404 Not Found", "no pane with that id") }
                model.beginPaneDrag(pane, fresh: true)
            case "hover":
                guard model.paneDrag != nil else { return refuse("409 Conflict", "no drag in progress") }
                guard let target = query["target"], let zone = query["zone"] else { return refuse("400 Bad Request", "target and zone are required") }
                let parsed: DropZone? = zone == "center" ? .center : Dir(rawValue: zone).map { .edge($0) }
                guard let parsed else { return refuse("400 Bad Request", "zone is center, left, right, up or down") }
                model.hoverPaneDrag(PaneDrop(target: target, zone: parsed))
            case "drop": model.endPaneDrag()
            case "cancel": model.cancelPaneDrag()
            default: return refuse("400 Bad Request", "action is start, hover, drop or cancel")
            }
        case ("POST", "/sidebar"):
            // The sidebar toggle and its rows: `open` is true, false or toggle. `expand` (a space
            // id) with `expanded` folds or unfolds a space. `pane` or `space` clicks that row.
            Log.info("debug: sidebar \(query)")
            let open: Bool?
            switch query["open"] {
            case nil: open = nil
            case "true": open = true
            case "false": open = false
            case "toggle": open = !model.sidebarOpen
            default: return refuse("400 Bad Request", "open is true, false or toggle")
            }
            if let space = query["expand"], !model.spaces.contains(where: { $0.id == space }) {
                return refuse("404 Not Found", "no space with that id")
            }
            if let pane = query["pane"], !model.spaces.contains(where: { $0.root.panes.contains { $0.id == pane } }) {
                return refuse("404 Not Found", "no pane with that id")
            }
            if let space = query["space"], !model.spaces.contains(where: { $0.id == space }) {
                return refuse("404 Not Found", "no space with that id")
            }
            if let open { model.setSidebar(open: open) }
            if let space = query["expand"] { model.setSpace(space, expanded: query["expanded"] != "false") }
            if let pane = query["pane"] {
                model.select(pane: pane)
            } else if let space = query["space"] {
                model.focus(space: space)
            }
        case ("POST", "/context/toggle"):
            guard let pane = query["pane"] else { return refuse("400 Bad Request", "pane is required") }
            model.toggleContextLabel(pane)
        case ("POST", "/pane/close"):
            guard let pane = query["pane"] else { return refuse("400 Bad Request", "pane is required") }
            model.closePane(pane)
        case ("POST", "/app/quit"):
            // Quit the way ⌘Q does; the app may be gone before this reply reaches the caller.
            Log.info("debug: quitting")
            DispatchQueue.main.async { NSApp.terminate(nil) }
        case ("GET", _):
            break
        default:
            return refuse("404 Not Found", "unknown route")
        }
        let body = (try? JSONSerialization.data(withJSONObject: state(), options: [.prettyPrinted, .sortedKeys])) ?? Data()
        return ("200 OK", body)
    }

    /// Select text across rows the way a drag does, and copy it the way ⌘C does. The ends are
    /// character offsets into the rows' plain text; with `mouse` set and both rows on screen, the
    /// press, drag and release go through the transcript's own mouse handlers at the window points
    /// of those offsets, otherwise the selection is set on the model, which reaches rows scrolled
    /// out of view.
    private func selectAndCopy(_ chat: SessionChat, from: (row: Int, offset: Int), to: (row: Int, offset: Int), mouse: Bool) -> (String, Data) {
        func fail(_ status: String, _ why: String) -> (String, Data) {
            Log.info("debug request refused: \(why)")
            return (status, Data(#"{"error":"\#(why)"}"#.utf8))
        }
        guard let window = NSApp.windows.first(where: { $0.title == "Wheel" }), chat.frameInWindow != .zero else {
            return fail("404 Not Found", "that transcript is not on screen")
        }
        func text(_ id: Int) -> String? {
            chat.items.first { $0.id == id }.map { $0.plainText(expanded: chat.expanded) }.flatMap { $0.isEmpty ? nil : $0 }
        }
        guard let startText = text(from.row) else { return fail("404 Not Found", "row \(from.row) has no text in this transcript") }
        guard let endText = text(to.row) else { return fail("404 Not Found", "row \(to.row) has no text in this transcript") }
        let selection = chat.selection
        let start = TranscriptSelection.Point(row: from.row, offset: min(max(from.offset, 0), startText.utf16.count))
        let end = TranscriptSelection.Point(row: to.row, offset: min(max(to.offset, 0), endText.utf16.count))
        if mouse {
            guard let first = selection.targets[from.row], let last = selection.targets[to.row] else {
                return fail("404 Not Found", "rows \(from.row) and \(to.row) are not both on screen")
            }
            func point(_ target: TranscriptSelection.Target, _ text: String, _ offset: Int) -> CGPoint {
                let inside = TextGeometry(text, style: target.style, width: target.frame.width).point(of: offset)
                return CGPoint(x: target.frame.minX + inside.x, y: target.frame.minY + inside.y)
            }
            selection.mouseDown(at: point(first, startText, start.offset), extend: false, in: chat)
            selection.mouseDragged(to: point(last, endText, end.offset), in: chat, window: window)
            selection.mouseUp()
        } else {
            selection.select(from: start, to: end)
        }
        NSPasteboard.general.clearContents()
        selection.copy(in: chat)
        let copied = NSPasteboard.general.string(forType: .string) ?? ""
        let selected = selection.rows(in: chat).map(\.id)
        Log.info("debug: select \(chat.name) \(from) -> \(to)\(mouse ? " by mouse" : ""): rows \(selected), copied \(String(reflecting: copied))")
        let answer: [String: Any] = ["selected_rows": selected, "row_highlighted": !selected.isEmpty, "pasteboard": copied]
        return ("200 OK", (try? JSONSerialization.data(withJSONObject: answer, options: [.prettyPrinted, .sortedKeys])) ?? Data())
    }

    struct Refusal: Error { let status: String; let message: String }

    /// The window and scroll view behind a transcript.
    private func transcriptScrollView(_ chat: SessionChat) -> Result<(NSWindow, NSScrollView), Refusal> {
        guard let window = NSApp.windows.first(where: { $0.title == "Wheel" }), chat.frameInWindow != .zero else {
            return .failure(Refusal(status: "404 Not Found", message: "that transcript is not on screen"))
        }
        guard let scroll = chat.scrollView(in: window) else {
            return .failure(Refusal(status: "404 Not Found", message: "no scroll view under that transcript"))
        }
        return .success((window, scroll))
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
        guard let call = row.toolCall, let perm = model.perms.first(where: { $0.session == session && $0.toolCall == call }) else { return text }
        return "\(text) · waiting perm \(perm.id)"
    }

    /// The transcript as laid out, with the ids `/agent/expand` takes: `<kind> <id>` for a row drawn
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
        case let .pane(id, kind):
            var o: [String: Any] = switch kind {
            case .empty: ["kind": "empty"]
            case let .terminal(term, agent):
                ["kind": "terminal", "term": term, "agent": agent.map { ["name": $0.name, "session": $0.session, "context": contextJSON($0.context, pane: id)] as Any } ?? NSNull()]
            case let .web(artifact): ["kind": "web", "scope": artifact.scope.rawValue, "name": artifact.name]
            case let .browser(url): ["kind": "browser", "url": url]
            case let .agent(session): ["kind": "agent", "session": session]
            case let .unknown(_, raw): raw.mapValues(\.foundation)
            }
            o["node"] = "pane"
            o["id"] = id
            return o
        }
    }

    /// What each pane shows right now, keyed by pane id: for a terminal, whether it is attached,
    /// whether its surface draws, the grid size it reported and its visible text; for a web pane,
    /// the artifact's scope and name, its page address (without the token), when it last loaded and
    /// reloaded, the load's HTTP status, and the page's title and visible text; for a browser pane,
    /// the page the daemon holds and the page the view is on, its title, load progress, history
    /// buttons, whether the chrome is narrow, the field's text while edited and any failure; for an agent pane, its session, that
    /// session's state, its last 20 transcript rows, its composer's draft, the error and notice
    /// under it, the request card it draws and the controls under its composer.
    private func panes() -> [String: Any] {
        var out: [String: Any] = [:]
        for space in model.spaces {
            for pane in space.root.panes {
                var o: [String: Any] = ["space": space.id, "kind": pane.kind.name]
                if case let .terminal(term, _) = pane.kind, let terminal = model.terminals[term] {
                    o["term"] = term
                    o["attached"] = terminal.attached
                    o["visible"] = terminal.view.isSurfaceVisible
                    o["cols"] = terminal.size?.cols ?? NSNull()
                    o["rows"] = terminal.size?.rows ?? NSNull()
                    o["viewport_text"] = terminal.viewportText ?? NSNull()
                }
                if case let .terminal(_, agent) = pane.kind {
                    o["context"] = contextJSON(agent?.context, pane: pane.id)
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
                    o["notice"] = chat?.notice ?? NSNull()
                    o["context"] = contextJSON(chat?.context, pane: pane.id)
                    o["request"] = model.perms.first { $0.session == session }?.id ?? NSNull()
                    o["config"] = chat.map { configJSON($0.config) } ?? NSNull()
                    o["transcript_redraws"] = chat.map { chat -> [String: Any] in
                        ["rows": chat.rows.count, "list_updates": chat.listUpdates, "row_updates": chat.rowUpdates,
                         "row_draws": chat.rows.reduce(0) { $0 + $1.draws }]
                    } ?? NSNull()
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
                if let palette = model.palettes[pane.id] { o["palette"] = paletteState(palette) }
                out[pane.id] = o
            }
        }
        return out
    }

    /// An open palette as the window draws it: what the field says, every row with its scope,
    /// name, title and problems, which one is selected, the footer and the daemon's refusal.
    private func paletteState(_ palette: PanePalette) -> [String: Any] {
        let state = palette.state
        let listing: String = switch state.listing {
        case .loading: "loading"
        case .ready: state.all.isEmpty ? "empty" : state.rows.isEmpty ? "no_match" : "ready"
        case .failed: "failed"
        }
        return [
            "state": listing,
            "query": state.query,
            "placeholder": state.fieldPlaceholder,
            "footer": state.footer,
            "keys": state.keys.map { ["key": $0.symbol, "word": $0.word, "enabled": $0.enabled] as [String: Any] },
            "total": state.all.count,
            "project_rows": state.projectCount,
            "machine_rows": state.machineCount,
            "selected": state.selectedRow.map { ["scope": $0.info.scope.rawValue, "name": $0.info.name] } ?? NSNull(),
            "selected_index": state.rows.isEmpty ? NSNull() : state.selected,
            "rows": state.rows.map {
                ["scope": $0.info.scope.rawValue, "name": $0.info.name, "title": $0.info.title, "problems": $0.info.problems] as [String: Any]
            },
            "error": state.failure ?? NSNull(),
            "field_focused": palette.fieldFocused,
            "confirming": palette.confirming.map { ["scope": $0.scope.rawValue, "name": $0.name] } ?? NSNull(),
        ]
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
            "sidebar": [
                "open": model.sidebarOpen,
                // What each side draws: the rows while open, the tab names while closed.
                "rows": model.sidebarOpen ? model.spaces.map { space -> [String: Any] in
                    let expanded = !model.collapsedSpaces.contains(space.id)
                    return ["space": space.id, "name": space.name, "active": space.id == model.active, "expanded": expanded,
                            "panes": expanded ? space.root.panes.map { pane -> [String: Any] in
                                var row: [String: Any] = ["pane": pane.id, "kind": pane.kind.name, "title": model.paneTitle(pane.id, pane.kind),
                                                          "focused": space.id == model.active && pane.id == space.focus]
                                if case let .agent(session) = pane.kind {
                                    row["state"] = model.chats[session].map { model.stateLabel($0) } ?? NSNull()
                                }
                                return row
                            } : []]
                } : [],
                "tabs": model.sidebarOpen ? [] : model.spaces.map { ["space": $0.id, "name": $0.name, "active": $0.id == model.active] as [String: Any] },
            ] as [String: Any],
            "focus_key": model.focusKey ?? NSNull(),
            "panes": panes(),
            "layout_error": model.layoutError ?? NSNull(),
            "drag_ratios": model.dragRatios,
            "pane_drag": model.paneDrag.map { drag in
                ["pane": drag.pane, "target": drag.drop?.target ?? NSNull(), "zone": drag.drop.map { $0.zone.label } ?? NSNull(), "committed": drag.committed] as [String: Any]
            } ?? NSNull(),
            "perms": model.perms.map {
                ["id": $0.id, "session": $0.session, "title": $0.title, "tool_call": $0.toolCall ?? NSNull(),
                 "options": $0.options.map { ["id": $0.id, "kind": $0.kind, "name": $0.name] }] as [String: Any]
            },
            "waiting_count": model.perms.count,
            "request_card": model.requestCardShown.map {
                ["tool_call": $0.call, "content": $0.content.map(contentText)] as [String: Any]
            } ?? NSNull(),
            "settings": settingsJSON(),
            "usage": usageJSON(),
            "log_path": Log.path.path,
        ]
    }

    /// The SwiftUI Settings scene's window, found by the identifier SwiftUI gives it.
    static var settingsWindow: NSWindow? {
        NSApp.windows.first { $0.identifier?.rawValue.contains("Settings") == true }
    }

    /// The usage strip as drawn (`strip`: per shown agent its pinned rows, or the note shown in
    /// their place), the Usage window's state and every agent the board lists.
    private func usageJSON() -> [String: Any] {
        let u = model.usage
        let board = u.board
        return [
            "window_open": u.open,
            "loaded": u.loaded,
            "pins": board.pins,
            "strip": board.shown.flatMap { agent -> [[String: Any]] in
                if let note = agent.missingNote {
                    return [["agent": agent.id, "account": NSNull(), "note": note, "stale": false, "age_secs": NSNull(), "rows": []]]
                }
                return agent.accounts.map { account in
                    [
                        "agent": agent.id,
                        "account": account.name,
                        "note": account.missingNote ?? NSNull(),
                        "stale": account.isStale,
                        "age_secs": account.usage.ageSecs.map { Int($0) } ?? NSNull(),
                        "rows": board.stripRows(for: account, of: agent).map { ["id": $0.id, "label": $0.label, "tag": $0.tag, "pct": $0.pct] },
                    ]
                }
            },
            "agents": board.agents.map { agent -> [String: Any] in
                [
                    "id": agent.id, "name": agent.name, "shown": agent.shown, "readable": agent.readable,
                    "why": agent.why ?? NSNull(),
                    "accounts": agent.accounts.map { account -> [String: Any] in
                        [
                            "id": account.id, "name": account.name,
                            "kind": account.usage.kind.rawValue,
                            "why": account.usage.why ?? NSNull(),
                            "plan": account.usage.reading?.plan ?? NSNull(),
                            "email": account.usage.reading?.email ?? NSNull(),
                            "extra_usage": account.usage.reading?.extraUsage ?? NSNull(),
                            "rows": account.rows(agent: agent.id).map { ["id": $0.id, "label": $0.label, "pct": $0.pct, "resets": UsageText.resets($0.resetsAt) ?? NSNull(), "pinned": board.isPinned($0.id)] },
                        ]
                    },
                ]
            },
        ]
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
            "command_line": [
                "state": s.commandLine.state,
                "detail": s.commandLine.detail ?? NSNull(),
                "running": s.commandLine.running,
                "shell_problem": s.commandLine.shellProblem ?? NSNull(),
            ] as [String: Any],
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

    /// The context figure a header draws: its words and its hover, beside the numbers behind them.
    private func contextJSON(_ context: ContextUse?, pane: String) -> Any {
        guard let context else { return NSNull() }
        let raw = model.rawContextPanes.contains(pane)
        return ["used": context.used, "size": context.size, "percent": context.percent, "text": context.text(raw: raw),
                "label": raw ? "raw" : "percent", "help": context.helpText] as [String: Any]
    }

    /// An agent's config controls as its pane draws them: what each shows, and what it offers.
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
