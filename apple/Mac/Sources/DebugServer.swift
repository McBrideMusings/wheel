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
/// `POST /perm?id=<id>&choice=allow|deny` (the Waiting list's buttons), the lead header menu's
/// `POST /lead/new` and `/lead/resume?id=`, `/lead/sessions` (list again) and
/// `/lead/menu?hold=<secs>[&part=resume]` (opens that menu for a capture), and the grid's controls:
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
/// `/window/front?which=settings` uncovers it. A managed pane (ADR-0010): `POST /managed?pane=&action=`
/// `transcript` (the header's Transcript toggle), `unlock` (its Unlock… button, which opens the confirm
/// sheet), `confirm` / `cancel` (the sheet's buttons), `hand_back` (the unlocked header's button),
/// `hover` / `unhover` (the manager's badge) and `badge` (a click on it).
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
        case ("POST", "/lead"):
            Log.info("debug: lead send")
            model.lead.draft = text
            model.send(model.lead)
        case ("POST", "/lead/expand"):
            // Open or close a tool line, tool group or thought by its id from `transcript_items`:
            // the click on its line.
            guard let id = query["id"].flatMap(Int.init) else { return refuse("400 Bad Request", "id is required") }
            model.lead.toggle(id)
        case ("POST", "/lead/board"):
            // The header's "▾ Board" and "▴ Collapse".
            switch query["action"] {
            case "expand": model.leadBoardExpanded = true
            case "collapse": model.leadBoardExpanded = false
            default: return refuse("400 Bad Request", "action is expand or collapse")
            }
            Log.info("debug: lead board \(query["action"] ?? "-")")
        case ("POST", "/lead/draft"):
            // Text in the lead's composer, not sent: what a person typing sees.
            Log.info("debug: lead draft, \(text.count) chars")
            model.lead.draft = text
        case ("POST", "/lead/new"):
            // The header menu's New Conversation.
            Log.info("debug: lead new")
            model.newLeadConversation()
        case ("POST", "/lead/resume"):
            // A Resume row: the daemon is asked even for an id it does not list, so a check can
            // see a refusal reach the composer.
            guard let id = query["id"], !id.isEmpty else { return refuse("400 Bad Request", "id is required") }
            Log.info("debug: lead resume \(id)")
            model.resumeLeadConversation(id)
        case ("POST", "/lead/sessions"):
            model.refreshLeadSessions()
        case ("POST", "/lead/menu"):
            // Drop the header's menu from its own button, as a click does, and close it after
            // `hold` seconds so a script can capture it meanwhile. The menu runs its own event
            // loop until then, so it opens after this reply goes out.
            guard let anchor = LeadMenu.anchor else { return refuse("404 Not Found", "the lead card is not on screen") }
            let hold = query["hold"].flatMap(Double.init) ?? 2
            let resumeOnly = query["part"] == "resume"
            DispatchQueue.main.async { [model] in
                let timer = Timer(timeInterval: hold, repeats: false) { _ in
                    MainActor.assumeIsolated { LeadMenu.open?.cancelTracking() }
                }
                RunLoop.main.add(timer, forMode: .common)
                LeadMenu.show(model, from: anchor, resumeOnly: resumeOnly)
            }
        case ("POST", "/agent"):
            guard let session = query["session"], session != AppModel.lead, let chat = model.chats[session] else {
                return refuse("404 Not Found", "no agent pane shows that session")
            }
            if let pane = lockedPane(showing: session) { return refuse("409 Conflict", "pane \(pane) is locked; unlock it to type") }
            Log.info("debug: send to \(session)")
            chat.draft = text
            model.send(chat)
        case ("POST", "/agent/focus"):
            guard let session = query["session"], session != AppModel.lead, let chat = model.chats[session] else {
                return refuse("404 Not Found", "no agent pane shows that session")
            }
            if let pane = lockedPane(showing: session) { return refuse("409 Conflict", "pane \(pane) is locked; unlock it to type") }
            Log.info("debug: focus the composer of \(session)")
            chat.composerFocusRequest += 1
        case ("POST", "/supervisor"):
            // A supervisor pane's own controls: `action` is expand | collapse (the board's
            // buttons), pause | resume (the play/pause button), workers | cap with `n` (the menus;
            // a cap with no `n` removes it), or type (the composer's text, which is not sent).
            guard let pane = query["pane"], model.supervisorPanes[pane] != nil else {
                return refuse("404 Not Found", "no supervisor pane with that id")
            }
            Log.info("debug: supervisor \(pane) \(query["action"] ?? "-")")
            switch query["action"] {
            case "expand": model.supervisorPanes[pane]?.expanded = true
            case "collapse": model.supervisorPanes[pane]?.expanded = false
            case "pause": model.pauseSupervisor(pane)
            case "resume": model.resumeSupervisor(pane)
            case "workers":
                guard let n = query["n"].flatMap(Int.init) else { return refuse("400 Bad Request", "n is required") }
                model.setSupervisorWorkers(pane, n)
            case "cap": model.setSupervisorCap(pane, query["n"].flatMap(Int.init))
            case "type":
                guard model.lock(of: pane) != .locked else { return refuse("409 Conflict", "pane \(pane) is locked; unlock it to type") }
                model.supervisorTyped(pane: pane, draft: text)
            case "open", "push":
                // Open and Push to pane name the worker's session; push acts on the open worker
                // when none is named, as the drill-in's own button does.
                guard let session = query["worker"] ?? model.supervisorPanes[pane]?.openWorker else {
                    return refuse("400 Bad Request", "worker is required")
                }
                if query["action"] == "open" { model.openWorker(pane, session: session) } else { model.pushWorker(pane, session: session) }
            case "back": model.closeWorker(pane)
            default: return refuse("400 Bad Request", "action is expand, collapse, pause, resume, workers, cap, type, open, push or back")
            }
        case ("POST", "/managed"):
            guard let pane = query["pane"], model.managers(of: pane) != nil else {
                return refuse("404 Not Found", "no pane with that id")
            }
            let action = query["action"] ?? ""
            Log.info("debug: managed \(pane) \(action)")
            let by = model.managers(of: pane)?.createdBy
            switch action {
            case "transcript":
                guard model.lock(of: pane) == .locked else { return refuse("409 Conflict", "that pane is not locked") }
                model.toggleTranscript(pane)
            case "unlock":
                guard model.lock(of: pane) == .locked else { return refuse("409 Conflict", "that pane is not locked") }
                model.requestUnlock(pane)
            case "confirm":
                guard model.unlockPrompt == pane else { return refuse("409 Conflict", "the sheet is not asking about that pane") }
                model.confirmUnlock()
            case "cancel": model.cancelUnlock()
            case "hand_back":
                guard model.lock(of: pane) == .unlocked else { return refuse("409 Conflict", "that pane is not unlocked") }
                model.handBack(pane)
            case "hover": model.highlight(manager: by)
            case "unhover": model.highlight(manager: nil)
            case "badge": if let by { model.focus(manager: by) }
            default: return refuse("400 Bad Request", "action is transcript, unlock, confirm, cancel, hand_back, hover, unhover or badge")
            }
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
        case ("POST", "/lead/key"):
            // Press a scrolling key the way a keyboard does: a key event through the application's
            // event path, so the transcript's key monitor sees it, delivered to the first responder
            // that is the transcript's scroll view. Unlike /scroll, it does not mark a scroll
            // under way, so the unpin it causes is the key path's own.
            guard let key = query["key"].flatMap(ScrollKey.init(rawValue:)) else {
                return refuse("400 Bad Request", "key (\(ScrollKey.allCases.map(\.rawValue).joined(separator: ", "))) is required")
            }
            let found = transcriptScrollView(model.lead)
            guard case let .success((window, scroll)) = found else {
                if case let .failure(refusal) = found { return refuse(refusal.status, refusal.message) }
                return refuse("404 Not Found", "no scroll view under the lead transcript")
            }
            guard let event = NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: window.windowNumber, context: nil, characters: key.characters, charactersIgnoringModifiers: key.characters,
                isARepeat: false, keyCode: key.keyCode) else {
                return refuse("500 Internal Server Error", "could not make a key event")
            }
            let focused = window.makeFirstResponder(scroll.documentView ?? scroll)
            Log.info("debug: key \(key.rawValue) to lead transcript (first responder taken: \(focused))")
            NSApp.sendEvent(event)
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
            if model.lock(of: pane) == .locked { return refuse("409 Conflict", "pane \(pane) is locked; unlock it to use it") }
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
        case ("POST", "/window/snapshot"):
            // The app captures its own window by its own window number, so a check never has to
            // work out where the window sits. `screencapture -R` takes global coordinates and
            // clamps a negative one, so a window on a display left of the main one captures the
            // wrong region entirely; this has no such failure.
            guard let path = query["path"] else { return refuse("400 Bad Request", "path is required") }
            // `which=sheet` captures the sheet over the window (the unlock confirmation), which the
            // window's own content view does not draw.
            let sheet = query["which"] == "sheet"
            guard let window = sheet ? NSApp.windows.first(where: \.isSheet) : NSApp.windows.first(where: { $0.title == "Wheel" }) else {
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

    struct Refusal: Error { let status: String; let message: String }

    /// The locked pane that shows `session`, if one does: a composer is not drawn there, so nothing may type into it.
    private func lockedPane(showing session: String) -> String? {
        for space in model.spaces {
            for pane in space.root.panes where model.lock(of: pane.id) == .locked {
                switch pane.kind {
                case let .agent(s), let .worker(s): if s == session { return pane.id }
                case let .supervisor(supervisor): if supervisor.session == session { return pane.id }
                default: break
                }
            }
        }
        return nil
    }

    /// The window and scroll view behind a transcript. SwiftUI content is not made of views, so a
    /// hit test stops at the hosting view: look through the window's scroll views for the smallest
    /// one under the transcript's middle.
    private func transcriptScrollView(_ chat: SessionChat) -> Result<(NSWindow, NSScrollView), Refusal> {
        guard let window = NSApp.windows.first(where: { $0.title == "Wheel" }), let content = window.contentView,
              chat.frameInWindow != .zero else {
            return .failure(Refusal(status: "404 Not Found", message: "that transcript is not on screen"))
        }
        let frame = chat.frameInWindow
        let point = NSPoint(x: frame.midX, y: content.isFlipped ? frame.midY : content.bounds.height - frame.midY)
        func scrollViews(in view: NSView) -> [NSScrollView] {
            view.subviews.flatMap { ($0 as? NSScrollView).map { [$0] } ?? [] + scrollViews(in: $0) }
                + view.subviews.compactMap { $0 as? NSScrollView }.flatMap { scrollViews(in: $0) }
        }
        let under = scrollViews(in: content).filter { $0.convert($0.bounds, to: content).contains(point) }
        guard let scroll = under.min(by: { $0.bounds.width * $0.bounds.height < $1.bounds.width * $1.bounds.height }) else {
            return .failure(Refusal(status: "404 Not Found",
                                    message: "no scroll view under that transcript (\(scrollViews(in: content).count) in the window)"))
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

    /// What a supervisor pane draws while worker `session` is open in it.
    private func drillIn(_ session: String, board: SupervisorPaneModel) -> [String: Any] {
        let line = board.board?.line(session: session, now: Date())
        let chat = model.chats[session]
        return [
            "session": session,
            "back": line?.backText ?? "← Supervisor",
            "lane": line?.laneText ?? NSNull(),
            "bead": line?.worker.bead ?? NSNull(),
            "word": line?.word ?? NSNull(),
            "glyph": line?.glyph ?? NSNull(),
            "stage": line?.stageText ?? NSNull(),
            "stage_filled": line?.stageFilled ?? NSNull(),
            "push": WorkerLine.pushText,
            "push_enabled": line.map { !$0.inPane } ?? false,
            "composer_placeholder": WorkerLine.composerPlaceholder,
            "composer": chat?.draft ?? NSNull(),
            "transcript": chat.map { chat in chat.transcript.rows.suffix(20).map { rowText($0, session: session) } } ?? NSNull(),
            "requests": requestCards(session),
            "error": chat?.error ?? NSNull(),
        ]
    }

    /// The request cards a worker's conversation draws above its composer, with the buttons each
    /// carries and the daemon option each button answers with.
    private func requestCards(_ session: String) -> [[String: Any]] {
        WorkerRequest.pending(model.perms, session: session).map { perm in
            let bead = model.workerLine(session: session)?.line.worker.bead ?? session
            return [
                "id": perm.id, "title": perm.title, "heading": "\(bead) \(WorkerRequest.wantsToRun)",
                "policy_note": WorkerRequest.outsidePolicy,
                "options": perm.options.map { ["id": $0.id, "kind": $0.kind, "name": $0.name] },
                "always_label": "Always allow \(WorkerRequest.subject(perm.title)) here",
            ] as [String: Any]
        }
    }

    /// The tree as the daemon sent it; an unknown pane carries every field it came with.
    private func node(_ node: Node) -> [String: Any] {
        switch node {
        case let .split(id, axis, ratio, a, b):
            return ["node": "split", "id": id, "axis": axis.rawValue, "ratio": ratio, "a": self.node(a), "b": self.node(b)]
        case let .pane(id, kind, createdBy, prior):
            var o: [String: Any] = switch kind {
            case .empty: ["kind": "empty"]
            case let .terminal(term): ["kind": "terminal", "term": term]
            case let .web(artifact): ["kind": "web", "scope": artifact.scope.rawValue, "name": artifact.name]
            case let .browser(url): ["kind": "browser", "url": url]
            case let .agent(session): ["kind": "agent", "session": session]
            case let .supervisor(supervisor): ["kind": "supervisor", "id": supervisor.id, "state": supervisor.state.rawValue, "session": supervisor.session]
            case let .worker(session): ["kind": "worker", "session": session]
            case let .unknown(_, raw): raw.mapValues(\.foundation)
            }
            o["node"] = "pane"
            o["id"] = id
            o["created_by"] = createdBy.label
            o["prior"] = prior?.label ?? NSNull()
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
                for (key, value) in managedJSON(id: pane.id, kind: pane.kind) { o[key] = value }
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
                if case let .supervisor(supervisor) = pane.kind, let board = model.supervisorPanes[pane.id] {
                    let chat = model.chats[supervisor.session]
                    o["session"] = supervisor.session
                    o["supervisor"] = supervisor.id
                    o["goal"] = supervisor.goal ?? NSNull()
                    o["supervisor_state"] = supervisor.state.rawValue
                    o["workers_allowed"] = supervisor.workers
                    o["cap"] = supervisor.cap ?? NSNull()
                    o["expanded"] = board.expanded
                    o["composer"] = chat?.draft ?? NSNull()
                    o["transcript"] = chat.map { chat in chat.transcript.rows.suffix(20).map { rowText($0, session: supervisor.session) } } ?? NSNull()
                    o["error"] = board.error ?? NSNull()
                    o["progress"] = board.board?.progressText ?? NSNull()
                    o["workers"] = board.board?.lines(now: Date()).map {
                        ["lane": $0.lane, "session": $0.worker.session, "bead": $0.worker.bead, "title": $0.worker.title, "model": $0.worker.model,
                         "word": $0.word, "glyph": $0.glyph, "stage": $0.stageText, "pane": $0.worker.pane ?? NSNull(),
                         "in_a_pane": $0.inPane ? WorkerLine.inAPaneText : NSNull(), "open_button": !$0.inPane] as [String: Any]
                    } ?? NSNull()
                    o["open_worker"] = board.openWorker ?? NSNull()
                    o["drill_in"] = board.openWorker.map { drillIn($0, board: board) } ?? NSNull()
                    o["queued"] = board.board?.queued.map { ["bead": $0.bead, "model": $0.model] } ?? NSNull()
                    o["not_refilled"] = board.board?.notRefilling ?? NSNull()
                    o["landed"] = board.board?.landed.map { ["bead": $0.bead, "detail": $0.detail] } ?? NSNull()
                    o["events"] = board.events.suffix(40).map { ["id": $0.id, "kind": $0.kind.name, "bead": $0.bead ?? NSNull(), "detail": $0.detail] as [String: Any] }
                    o["inbox"] = board.inbox.map { ["id": $0.id, "bead": $0.bead, "kind": $0.kind.name] }
                    o["hold_text"] = board.board?.holdText ?? NSNull()
                }
                if case let .worker(session) = pane.kind {
                    let chat = model.chats[session]
                    let found = model.workerLine(session: session)
                    o["session"] = session
                    o["lane"] = found?.line.laneText ?? NSNull()
                    o["bead"] = found?.line.worker.bead ?? NSNull()
                    o["word"] = found?.line.word ?? NSNull()
                    o["stage"] = found?.line.stageText ?? NSNull()
                    o["closing_text"] = WorkerLine.closingText
                    o["composer_placeholder"] = WorkerLine.composerPlaceholder
                    o["composer"] = chat?.draft ?? NSNull()
                    o["transcript"] = chat.map { chat in chat.transcript.rows.suffix(20).map { rowText($0, session: session) } } ?? NSNull()
                    o["requests"] = requestCards(session)
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
                if let palette = model.palettes[pane.id] { o["palette"] = paletteState(palette) }
                out[pane.id] = o
            }
        }
        return out
    }

    /// What a managed pane draws around its content: the lock, the manager's badge, the toggles,
    /// whether a composer is drawn, the dashboard's values and the unlocked header's hand-back.
    private func managedJSON(id: String, kind: PaneKind) -> [String: Any] {
        guard let managers = model.managers(of: id) else { return [:] }
        let pane = (id: id, kind: kind, createdBy: managers.createdBy, prior: managers.prior)
        let lock = ManagedPane.lock(createdBy: pane.createdBy, prior: pane.prior, locking: model.paneLocking)
        let supervisors = model.supervisorsById
        var o: [String: Any] = [
            "created_by": pane.createdBy.label,
            "prior": pane.prior?.label ?? NSNull(),
            "lock": lock == .locked ? "locked" : lock == .unlocked ? "unlocked" : "yours",
        ]
        guard lock != .yours else { return o }
        let sessionOf: String? = switch pane.kind {
        case let .worker(session), let .agent(session): session
        case let .supervisor(supervisor): supervisor.session
        default: nil
        }
        if lock == .locked {
            o["managed_by"] = ManagedPane.badge(pane.createdBy, supervisors: supervisors)
            o["manager_pane"] = model.pane(of: pane.createdBy) ?? NSNull()
            o["manager_highlighted"] = model.highlightedManager == pane.createdBy
            o["unlock_button"] = ManagedPane.unlockText
            o["unlock_prompt"] = model.unlockPrompt == pane.id
            o["transcript_toggle"] = model.lockedHasDashboard(pane.kind)
            o["transcript_on"] = model.transcriptPanes.contains(pane.id)
            o["showing"] = model.lockedShowing(pane.id).rawValue
            // A locked pane draws no text box, and a page or browser in it takes no click or key.
            o["composer_present"] = false
            o["interactive"] = false
            o["locked_composer"] = ManagedPane.lockedComposerHint
            o["read_only_notice"] = ManagedPane.readOnlyNotice(ManagedPane.managerName(pane.createdBy, supervisors: supervisors), isWorker: { if case .worker = pane.kind { true } else { false } }())
        } else {
            o["yours_badge"] = "\(ManagedPane.yoursBadge) · created_by person"
            o["banner"] = ManagedPane.unlockedBanner(at: model.unlockedAt[pane.id].map { $0.formatted(date: .omitted, time: .shortened) })
            o["composer_present"] = sessionOf != nil
            let canBack = ManagedPane.canHandBack(prior: pane.prior, supervisors: supervisors)
            o["hand_back_button"] = canBack ? pane.prior.map { ManagedPane.handBackText($0, supervisors: supervisors) } ?? NSNull() : NSNull()
        }
        if case let .worker(session) = pane.kind, let d = model.workerDashboard(session: session) {
            o["dashboard"] = [
                "bead": d.bead, "title": d.title, "status": d.statusWord, "tone": "\(d.tone)", "stage": d.stageText, "stage_filled": d.stageFilled,
                "elapsed": d.elapsed ?? NSNull(), "continues": d.continuesText, "restarts": d.restartsText, "model": d.model, "branch": d.branch,
                "actions": d.actions.map { ["title": $0.title, "age": $0.age] },
                "traffic": d.traffic.map { ["direction": $0.direction == .up ? "up" : "down", "route": $0.route, "kind": $0.kind, "new": $0.isNew, "detail": $0.detail] as [String: Any] },
            ] as [String: Any]
        }
        return o
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
            "pane_locking": model.paneLocking,
            "spaces": model.spaces.map { space in
                ["id": space.id, "name": space.name, "cwd": space.cwd, "focus": space.focus, "root": node(space.root)] as [String: Any]
            },
            "active": model.active ?? NSNull(),
            "space_marks": Dictionary(uniqueKeysWithValues: model.spaces.map { space -> (String, Any) in
                let marks = SpaceMarks.of(space, infos: model.supervisorInfos)
                return (space.id, ["state": marks.stateText ?? NSNull(), "for_you": marks.forYouText ?? NSNull()] as [String: Any])
            }),
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
                ["id": $0.id, "session": $0.session, "title": $0.title, "tool_call": $0.toolCall ?? NSNull(),
                 "options": $0.options.map { ["id": $0.id, "kind": $0.kind, "name": $0.name] }] as [String: Any]
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
            "lead_sessions": model.leadSessions.map {
                ["id": $0.id, "cwd": $0.cwd, "title": $0.title ?? NSNull(), "updated_at": $0.updatedAt ?? NSNull(),
                 "age": $0.age() ?? NSNull(), "current": model.isLeadConversation($0.id)] as [String: Any]
            },
            "lead_sessions_error": model.leadSessionsError ?? NSNull(),
            "lead_conversation": model.leadConversation ?? NSNull(),
            "lead_menu": LeadMenu.outline(model),
            "lead_board": leadBoardJSON(),
            "log_path": Log.path.path,
        ]
    }

    /// The lead card's board as it draws: the header's "Inbox N" and toggle, one pinned line per
    /// supervisor, and the For you rows (the expanded board draws the same data as cards and rows).
    private func leadBoardJSON() -> [String: Any] {
        let board = model.leadBoard
        return [
            "expanded": model.leadBoardExpanded,
            "toggle": model.leadBoardExpanded ? "▴ Collapse" : "▾ Board",
            "inbox": board.inboxLabel,
            "supervisors": board.supervisors.map { row in
                [
                    "pane": row.pane, "space": row.spaceName, "title": row.title, "state": row.stateWord, "glyph": row.glyph,
                    "landed": row.progressText, "workers": row.workersText, "for_you": row.forYou,
                ] as [String: Any]
            },
            "empty": board.supervisors.isEmpty ? LeadBoard.noSupervisors : NSNull(),
            "for_you": board.forYou.map { item in
                [
                    "id": item.id, "space": model.spaces.first { $0.id == item.source.space }?.name ?? item.source.space,
                    "kind": item.kind.rawValue, "state": item.state.rawValue, "headline": item.headline, "detail": item.shownDetail ?? NSNull(),
                    "supervisor": item.supervisor ?? NSNull(), "supervisor_goal": board.supervisorTitle(for: item) ?? NSNull(),
                ] as [String: Any]
            },
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
