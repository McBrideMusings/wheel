#if DEBUG
import AppKit
import SwiftUI
import Network
import WheelKit
import WheelUI

/// A loopback HTTP endpoint (Debug builds only) that answers with the app's state as JSON, so a
/// script can check what the window shows. It carries the terminal's screen, so it is guarded
/// like the daemon: the daemon's token as a bearer header, no browser Origin, loopback Host only.
///
///     curl -H "Authorization: Bearer $(cat <token file>)" http://127.0.0.1:7421/state
///
/// It also drives the UI's own code paths, so a script exercises what the buttons do. An agent
/// pane, by `agent=<s>`: `POST /agent` with the message as the body (it replaces the composer's
/// draft, then sends it as Return does), `/agent/draft` (the text, not sent), `/agent/typing` (a person typing into the composer, in-app), `/agent/focus` (the
/// keyboard goes into that composer, as a click would put it), `/agent/expand?id=` (a click on a
/// tool line, tool group or thought), `/agent/notice/dismiss` (the banner's close button),
/// `/agent/key?key=` (a scrolling key) and `/agent/config?config=&value=` (a control's menu);
/// `POST /perm?id=<id>&choice=allow|deny` (the request card's buttons), and the grid's controls:
/// `POST /space?cwd=<dir>` (the tab bar's `+` after the folder picker), `/space/close?id=`,
/// `/focus?space=` or `/focus?pane=`, `/split?pane=&dir=left|right|up|down`,
/// `/start?pane=&kind=terminal|agent[&message=][&agent=<name>]` (what the picker's Terminal and Agent send), `/kind?pane=&kind=empty|terminal|web[&artifact=][&scope=project|machine]` (`web` takes the artifact's name, as the picker sends it), `/ratio?split=&ratio=` (a divider drag's end; `&held=1` for one still in progress) and
/// `/pane/close?pane=` and `/pane/focused?pane=` (a terminal took the keyboard, as `isFocused` turning true does). A browser pane's chrome: `/browser?pane=&action=back|forward|reload|stop|home`,
/// `&action=edit[&text=]` (a click into the address field, then typing), `&action=open&text=`
/// (typing into the field and pressing Return) and `&action=cancel` (Escape in the field). The
/// Settings window: `/settings?action=open|close`, `&action=tab&tab=general|agents|usage|project|devices`,
/// `&action=project&id=<space>` (a row of the Project tab's list), and per field
/// `&action=edit&layer=global|project&key=&text=`, `&action=save&layer=&key=` (Return),
/// `&action=cancel&layer=&key=` (Esc) and `&action=clear&layer=&key=` (the ✕ button);
/// `/window/front?which=settings` uncovers it. The command palette over the window (a `/palette`
/// with no `pane`): `/palette?action=open|close|tab|run` (`open` takes `&mode=new_space`), `&action=type&text=`, `&action=select&index=`;
/// its `palette` entry of the state holds `open`, `mode`, `query`, `rows` and `selected`. A pane's artifact palette: `/palette?pane=&action=open|type|move|enter|escape|click|confirm|close`
/// (`&text=`, `&by=`, `&scope=&name=`); an open one shows in that pane's `palette` entry of the state. Keys:
/// `/key?keys=shift+cmd+d` (a key event, `&which=sheet` for the sheet over the window), `/action?id=`
/// (a menu item or palette row) and `/confirm?answer=yes|no` (the confirmation sheet's buttons); the
/// state's `keymap`, `keymap_problems`, `keybind`, `confirmation`, `menus`, `zoomed` and
/// `first_responder` read them back. Settings › Artifacts: `/builtins?action=ls|install|update|remove&name=`
/// and `&action=edited&choice=keep|replace|cancel`; the state's `builtins` reads it back. A space's
/// context menu: `/space/menu?space=&item=`, its items listed per space in the state's `space_menus`. Grid motion:
/// `/web/eval` with a JSON body `{"pane", "js"}` runs the script in that web pane's page and answers `{"result": …}`. `/metrics?action=start|stop` records frame gaps, pane body evaluations and terminal resizes sent, and replies with them. Each replies with the state as it was before the daemon answered.
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

    private static func controlJSON(_ entry: ChatControlRegistry.Entry) -> [String: Any] {
        ["kind": entry.kind, "label": entry.label, "hover": entry.hover, "focus": entry.focus, "url": entry.url ?? NSNull(),
         "frame": ["x": entry.frame.minX, "y": entry.frame.minY, "w": entry.frame.width, "h": entry.frame.height]]
    }

    func start() {
        ChatControlRegistry.shared.tracking = true
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
                    let (status, body) = self.respond(head: parsed.head, body: String(decoding: parsed.body, as: UTF8.self), connection: connection)
                    // A request the page answers later (`/web/eval`) replies from its own callback.
                    if status.isEmpty { return }
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

    private func respond(head request: String, body text: String, connection: NWConnection) -> (String, Data) {
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
        if TestMode.hidden, ["/window/front", "/window/back", "/app/activate", "/input/mouse", "/input/key"].contains(target.path) {
            return refuse("403 Forbidden", "this app is hidden for a check and never takes focus or input; use the debug routes (/reply/*, /key, /pane/*), or set WHEEL_VISIBLE=1 to watch")
        }
        switch (method, target.path) {
        case ("POST", "/agent"), ("POST", "/agent/draft"), ("POST", "/agent/focus"), ("POST", "/agent/expand"),
             ("POST", "/agent/notice/dismiss"), ("POST", "/agent/key"), ("POST", "/agent/type"), ("POST", "/agent/config"),
             ("POST", "/agent/link"), ("POST", "/agent/typing"), ("POST", "/agent/queue"), ("POST", "/agent/message"),
             ("POST", "/agent/attach"), ("POST", "/agent/pasteboard"), ("POST", "/agent/paste"), ("POST", "/agent/read-aloud"):
            guard let agent = query["agent"], let chat = model.chats[agent] else {
                return refuse("404 Not Found", "no agent pane shows that agent")
            }
            switch target.path {
            case "/agent":
                Log.info("debug: send to \(agent)")
                chat.draft = text
                model.send(chat)
            case "/agent/typing":
                // A person typing into the composer: one character every 1/`per_second` s (default
                // 10) until `chars` (default 300) are in the draft, each a draft change the way a key
                // is. Answers at once, without the state, so the run is measured without a request
                // per key or a state build of its own.
                let chars = query["chars"].flatMap(Int.init) ?? 300
                let interval = 1 / (query["per_second"].flatMap(Double.init) ?? 10)
                Log.info("debug: typing \(chars) characters into \(agent)")
                func key(_ n: Int) {
                    guard n < chars else { return }
                    chat.draft += n % 6 == 5 ? " " : "a"
                    DispatchQueue.main.asyncAfter(deadline: .now() + interval) { key(n + 1) }
                }
                key(0)
                return ("200 OK", Data("{}".utf8))
            case "/agent/attach":
                // A file dropped on the composer: `path=` is where it is, `remove=<name>` takes its chip out.
                let files = model.attachments(for: chat)
                if let name = query["remove"] {
                    guard let id = files.chips.first(where: { $0.name == name })?.id else { return refuse("404 Not Found", "no chip named \(name)") }
                    files.remove(id)
                } else if let path = query["path"] {
                    files.add([.file(URL(fileURLWithPath: path))])
                } else {
                    return refuse("400 Bad Request", "path or remove is required")
                }
            case "/agent/pasteboard":
                // Put `kind=image` (a 40x30 PNG), `kind=file` (`path=`) or `kind=text` (`text=`) on the
                // pasteboard and the keyboard in the composer, so a following ⌘V from `/agent/type` is a paste.
                let board = NSPasteboard.general
                board.clearContents()
                switch query["kind"] {
                case "image":
                    let image = NSImage(size: NSSize(width: 40, height: 30), flipped: false) { rect in
                        NSColor.systemOrange.setFill()
                        rect.fill()
                        return true
                    }
                    board.writeObjects([image])
                case "file":
                    guard let path = query["path"] else { return refuse("400 Bad Request", "path is required") }
                    board.writeObjects([URL(fileURLWithPath: path) as NSURL])
                case "text":
                    board.setString(query["text"] ?? "", forType: .string)
                default:
                    return refuse("400 Bad Request", "kind is image, file or text")
                }
                chat.composerFocusRequest += 1
            case "/agent/paste":
                // The Paste command, sent to whatever holds the keyboard as the Edit menu sends it.
                NSApp.wheelWindow?.makeKey()
                // ⌘V as the key monitor reads it; a paste it leaves goes to the text box.
                let sent = model.pasteAttachments(nil)
                    || NSApp.wheelWindow?.firstResponder?.tryToPerform(#selector(NSText.paste(_:)), with: nil) == true
                Log.info("debug: paste into \(NSApp.wheelWindow?.firstResponder.map { "\(type(of: $0))" } ?? "nothing"): \(sent ? "taken" : "nobody took it")")
            case "/agent/draft":
                // Text in the composer, not sent: what a person typing sees.
                Log.info("debug: draft for \(agent), \(text.count) chars")
                chat.draft = text
            case "/agent/read-aloud":
                // The speaker toggle's state, set directly: `on=1` or `on=0`. A stand-in for the click.
                Log.info("debug: read-aloud for \(agent): \(query["on"] ?? "")")
                chat.readsAloud = query["on"] == "1"
            case "/agent/focus":
                Log.info("debug: focus the composer of \(agent)")
                chat.composerFocusRequest += 1
            case "/agent/queue":
                // A click on a message's action: `action=edit|remove` with the message's `turn`
                // (a queued one, or for `edit` the one the agent has not answered), or
                // `action=clear` for the Clear queue button.
                let turn = query["turn"].flatMap(UInt64.init)
                switch (query["action"], turn) {
                case let ("edit", turn?):
                    guard let row = chat.transcript.rows.last(where: { $0.turn == turn }), case let .user(text, _) = row.kind else {
                        return refuse("404 Not Found", "no message for that turn")
                    }
                    chat.edit(turn: turn, text: text)
                case let ("remove", turn?): chat.remove(turn: turn)
                case let ("now", turn?): chat.sendNow(turn: turn)
                case ("clear", _): chat.clearQueue()
                default: return refuse("400 Bad Request", "action takes edit, remove or now with a turn, or clear")
                }
            case "/agent/message":
                // A pick from a message's right-click menu: `row` is the message's row id and
                // `action` one of copy, markdown or attach.
                guard let row = query["row"].flatMap(Int.init), let action = query["action"].flatMap(MessageAction.init(rawValue:)) else {
                    return refuse("400 Bad Request", "row and action (copy, markdown or attach) are required")
                }
                guard chat.isMessage(row: row) else { return refuse("404 Not Found", "that row is not a message") }
                // Build the menu a right-click opens and run the item the way a click does.
                let (menu, responder) = chat.selection.messageMenu(row: row, chat: chat)
                guard let index = menu.items.firstIndex(where: { ($0.representedObject as? MessageAction) == action }) else {
                    return refuse("404 Not Found", "the menu has no such item")
                }
                withExtendedLifetime(responder) { menu.performActionForItem(at: index) }
            case "/agent/link":
                // A click on a link in a reply: `url` is the link's address (a `wheel-link://` one
                // from `links.linked`, or a web address) and `row` the reply's row id.
                guard let url = query["url"].flatMap(URL.init(string:)), let action = ChatLinkAction(url: url),
                      let row = query["row"].flatMap(Int.init) else {
                    return refuse("400 Bad Request", "url (a link address) and row are required")
                }
                Log.info("debug: click \(url.absoluteString) in \(agent) row \(row)")
                model.links.click(action, look: model.appearance(forAgent: agent), agent: agent, row: row)
            case "/agent/expand":
                // Open or close a tool line, tool group or thought by its id from `transcript_items`:
                // the click on its line.
                guard let id = query["id"].flatMap(Int.init) else { return refuse("400 Bad Request", "id is required") }
                // An edits card's id with `path` is the click on that file's row, which opens or
                // closes its diff; with `collapse=all` the card's "Collapse all" button.
                if let path = query["path"] { chat.toggleDiff(card: id, path: path) }
                else if query["collapse"] == "all" { chat.collapseDiffs(card: id) }
                else { chat.toggle(id) }
            case "/agent/notice/dismiss":
                Log.info("debug: notice dismiss over \(agent)")
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
                Log.info("debug: key \(key.rawValue) to \(agent)'s transcript (first responder taken: \(focused))")
                NSApp.sendEvent(event)
            case "/agent/type":
                // Type one printable character (`text=h`, optional `mods=cmd`) as a keyboard does, through
                // the application's event path, so the app's key monitors see it. `from` says what holds
                // the keyboard first: `transcript` (default) its scroll view, `window` nothing (a click on
                // a pane header, the title bar or a header button leaves it there), `sidebar` the space
                // list's table, `readonly` a selectable read-only text (a clicked reply), `editable` an
                // editable text field, `keep` whatever already has it (the palette's open field). `to=settings`
                // sends the key to the Settings window instead, `from=search` first putting the keyboard in its
                // first text field (the search field of a topic page). The window is made key, as a click
                // would, without activating the app.
                guard let text = query["text"], !text.isEmpty else { return refuse("400 Bad Request", "text is required") }
                var flags: NSEvent.ModifierFlags = []
                for mod in (query["mods"] ?? "").split(separator: ",") {
                    switch mod {
                    case "cmd": flags.insert(.command)
                    case "ctrl": flags.insert(.control)
                    case "opt": flags.insert(.option)
                    case "shift": flags.insert(.shift)
                    default: return refuse("400 Bad Request", "mods are cmd, ctrl, opt, shift")
                    }
                }
                guard let main = NSApp.wheelWindow else { return refuse("404 Not Found", "no Wheel window") }
                var target = main
                if query["to"] == "settings" {
                    guard let settings = Self.settingsWindow else {
                        return refuse("404 Not Found", "the Settings window is not open")
                    }
                    target = settings
                    if query["from"] == "search" {
                        func field(in view: NSView) -> NSTextField? {
                            if let field = view as? NSTextField, field.isEditable { return field }
                            for sub in view.subviews { if let found = field(in: sub) { return found } }
                            return nil
                        }
                        guard let search = settings.contentView.flatMap(field(in:)) else { return refuse("404 Not Found", "no text field in the Settings window") }
                        settings.makeKey()
                        settings.makeFirstResponder(search)
                    }
                } else {
                    main.makeKey()
                    var temporary: NSView?
                    switch query["from"] ?? "transcript" {
                    case "transcript":
                        let found = transcriptScrollView(chat)
                        guard case let .success((_, scroll)) = found else {
                            if case let .failure(refusal) = found { return refuse(refusal.status, refusal.message) }
                            return refuse("404 Not Found", "no scroll view under that transcript")
                        }
                        main.makeFirstResponder(scroll.documentView ?? scroll)
                    case "window":
                        main.makeFirstResponder(nil)
                    case "keep":
                        break
                    case "sidebar":
                        func table(in view: NSView) -> NSTableView? {
                            if let table = view as? NSTableView { return table }
                            for sub in view.subviews { if let found = table(in: sub) { return found } }
                            return nil
                        }
                        guard let sidebar = main.contentView.flatMap(table(in:)) else { return refuse("404 Not Found", "no table view in the window") }
                        main.makeFirstResponder(sidebar)
                    case "readonly", "editable":
                        let view = NSTextView(frame: NSRect(x: 0, y: 0, width: 80, height: 20))
                        view.isEditable = query["from"] == "editable"
                        view.isSelectable = true
                        main.contentView?.addSubview(view)
                        temporary = view
                        main.makeFirstResponder(view)
                    default:
                        return refuse("400 Bad Request", "from is transcript, window, keep, sidebar, readonly or editable")
                    }
                    defer { temporary?.removeFromSuperview() }
                    guard let event = NSEvent.keyEvent(
                        with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
                        windowNumber: target.windowNumber, context: nil, characters: text, charactersIgnoringModifiers: text.lowercased(),
                        isARepeat: false, keyCode: 0) else {
                        return refuse("500 Internal Server Error", "could not make a key event")
                    }
                    Log.info("debug: type \(text) from \(query["from"] ?? "transcript"), first responder \(responderName(main)), key window \(main.isKeyWindow)")
                    NSApp.sendEvent(event)
                    return ("200 OK", Data("{}".utf8))
                }
                guard let event = NSEvent.keyEvent(
                    with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
                    windowNumber: target.windowNumber, context: nil, characters: text, charactersIgnoringModifiers: text.lowercased(),
                    isARepeat: false, keyCode: 0) else {
                    return refuse("500 Internal Server Error", "could not make a key event")
                }
                Log.info("debug: type \(text) to the Settings window")
                NSApp.sendEvent(event)
            default:
                // The same call the control's menu makes, so a check drives the pane's own path.
                guard let id = query["config"], let value = query["value"] else {
                    return refuse("400 Bad Request", "config and value are required")
                }
                guard let option = chat.config.first(where: { $0.id == id }) else {
                    return refuse("404 Not Found", "\(agent) has no config option \(id)")
                }
                Log.info("debug: set \(agent) \(id) to \(value)")
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
            // `agent` is an agent pane's agent.
            guard let agent = query["agent"], let fromEnd = query["from_end"].flatMap(Double.init) else {
                return refuse("400 Bad Request", "agent and from_end are required")
            }
            guard let chat = model.chats[agent] else {
                return refuse("404 Not Found", "no transcript shows that agent")
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
            Log.info("debug: scroll \(agent) to \(Int(fromEnd)) from its end (content \(Int(document.frame.height)), visible \(Int(clip.bounds.height)))")
        case ("POST", "/cursor"):
            // Which page views can set the cursor at window point (`x`, `y`, AppKit coordinates,
            // origin bottom left): AppKit delivers a view's tracking areas and cursor rects only
            // while neither it nor an ancestor is hidden, so a web or browser pane's page claims
            // the point when its frame holds it and it is not hidden. A real pointer cannot be
            // used (it is the person's), so this reads the condition under which a page sets the
            // cursor. Answers `{"claims": ["<pane>", ...]}`.
            guard let window = NSApp.wheelWindow, let x = query["x"].flatMap(Double.init), let y = query["y"].flatMap(Double.init) else {
                return refuse("400 Bad Request", "x and y (window points) and a Wheel window are required")
            }
            let point = NSPoint(x: x, y: y)
            var views: [String: NSView] = model.webPanes.mapValues(\.view)
            for (pane, browser) in model.browserPanes { views[pane] = browser.view }
            let claims = views.filter { _, view in
                view.window === window && !view.isHiddenOrHasHiddenAncestor && view.convert(view.bounds, to: nil).contains(point)
            }.keys.sorted()
            Log.info("debug: page views claiming the cursor at (\(x), \(y)): \(claims)")
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: ["claims": claims])) ?? Data())
        case ("POST", "/web/eval"):
            // Run `js` in the page of web pane `pane` (an artifact's page, not a browser pane) and
            // answer `{"result": <the value>}`, or `{"error": <what the page threw>}`. The body is
            // `{"pane": "p3", "js": "..."}`.
            guard let object = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any],
                  let pane = object["pane"] as? String, let js = object["js"] as? String else {
                return refuse("400 Bad Request", "the body is {\"pane\": ..., \"js\": ...}")
            }
            guard let web = model.webPanes[pane]?.view else { return refuse("404 Not Found", "pane \(pane) shows no artifact page") }
            Log.info("debug: web eval on \(pane), \(js.count) characters of script")
            web.evaluateJavaScript(js) { [weak self] value, error in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    if let error {
                        let info = (error as NSError).userInfo
                        let message = (info["WKJavaScriptExceptionMessage"] as? String) ?? error.localizedDescription
                        Log.info("debug: web eval on \(pane) threw: \(message)")
                        return self.reply(connection, "422 Unprocessable Entity", Data((try? JSONSerialization.data(withJSONObject: ["error": message])) ?? Data()))
                    }
                    let wrapped: [String: Any] = ["result": value ?? NSNull()]
                    let body = JSONSerialization.isValidJSONObject(wrapped) ? (try? JSONSerialization.data(withJSONObject: wrapped, options: [.sortedKeys])) : nil
                    Log.info("debug: web eval on \(pane) answered \(body?.count ?? 0) bytes")
                    self.reply(connection, body == nil ? "422 Unprocessable Entity" : "200 OK", body ?? Data(#"{"error":"the result is not JSON"}"#.utf8))
                }
            }
            return ("", Data())
        case ("POST", "/reply/copy"):
            // Press the copy button of reply `row` (a row id from `transcript_items`) and answer
            // with the pasteboard: the reply's Markdown.
            guard let session = query["agent"], let row = query["row"].flatMap(Int.init) else {
                return refuse("400 Bad Request", "agent and row are required")
            }
            NSPasteboard.general.clearContents()
            guard ReplyCopyRegistry.shared.press(agent: session, row: row) else { return refuse("404 Not Found", "reply \(row) is not on screen") }
            let copied = NSPasteboard.general.string(forType: .string) ?? ""
            Log.info("debug: pressed copy on reply \(row) of \(session), \(copied.count) characters")
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: ["pasteboard": copied])) ?? Data("{}".utf8))
        case ("POST", "/reply/play"):
            // Press the play button of reply `row`: it speaks that reply, or stops it when it is
            // the one being read. Answers with the key being read afterwards.
            guard let session = query["agent"], let row = query["row"].flatMap(Int.init) else {
                return refuse("400 Bad Request", "agent and row are required")
            }
            guard ReplyCopyRegistry.shared.play(agent: session, row: row) else { return refuse("404 Not Found", "reply \(row) is not on screen") }
            Log.info("debug: pressed play on reply \(row) of \(session)")
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: ["playing": (ReadAloud.shared.playing as Any?) ?? NSNull()])) ?? Data("{}".utf8))
        case ("POST", "/code/copy"):
            // Press the copy button of the `index`-th code block on screen in an agent's
            // transcript (the order of `code_blocks` in the state) and answer with the pasteboard.
            guard let agent = query["agent"], let index = query["index"].flatMap(Int.init) else {
                return refuse("400 Bad Request", "agent and index are required")
            }
            let blocks = CodeBlockRegistry.shared.blocks(agent: agent)
            guard blocks.indices.contains(index) else {
                return refuse("404 Not Found", "\(blocks.count) code blocks on screen for that agent")
            }
            NSPasteboard.general.clearContents()
            blocks[index].press()
            let copied = NSPasteboard.general.string(forType: .string) ?? ""
            Log.info("debug: pressed copy on code block \(index) of \(agent), \(copied.count) characters")
            let body = (try? JSONSerialization.data(withJSONObject: ["pasteboard": copied])) ?? Data("{}".utf8)
            return ("200 OK", body)
        case ("POST", "/image/open"):
            // Do what a click on the `index`-th image of an agent's replies does (the order of
            // `images` in the state): open it at full size, listed as `image_full` in the state.
            guard let agent = query["agent"], let index = query["index"].flatMap(Int.init) else {
                return refuse("400 Bad Request", "agent and index are required")
            }
            guard ChatImageRegistry.shared.click(agent: agent, index: index) else {
                return refuse("404 Not Found", "no loaded image \(index) for that agent")
            }
            Log.info("debug: opened image \(index) of \(agent) at full size")
            return ("200 OK", Data("{}".utf8))
        case ("POST", "/transcript/select"):
            // Select text across rows and copy it. `agent` is an agent pane's agent; `from_row`
            // and `to_row` are row ids from `transcript_items`, `from_offset` and `to_offset`
            // characters into the row's plain text. Rows need not be on screen; `mouse=1` drives the
            // transcript's mouse handlers at the offsets' window points instead (both rows built).
            // The answer is a JSON object, not the state.
            guard let agent = query["agent"],
                  let fromRow = query["from_row"].flatMap(Int.init), let fromOffset = query["from_offset"].flatMap(Int.init),
                  let toRow = query["to_row"].flatMap(Int.init), let toOffset = query["to_offset"].flatMap(Int.init) else {
                return refuse("400 Bad Request", "agent, from_row, from_offset, to_row and to_offset are required")
            }
            guard let chat = model.chats[agent] else { return refuse("404 Not Found", "no agent pane shows that agent") }
            // `clicks=2|3` presses at the from point that many times (word, paragraph) and ignores
            // the to point; `extend=left,right,up,down` (`word:` before Left or Right for a word)
            // then presses those shift-arrows, in order, before the copy.
            return selectAndCopy(chat, from: (fromRow, fromOffset), to: (toRow, toOffset), mouse: query["mouse"] == "1",
                                 clicks: query["clicks"].flatMap(Int.init) ?? 1,
                                 extend: query["extend"]?.split(separator: ",").map(String.init) ?? [])
        case ("POST", "/transcript/drag"):
            // Drag from one window point to another (top-left origin, as `/transcript/point` reports
            // them) through the transcript's mouse handlers, then copy the way ⌘C does. The answer
            // names the offsets the points resolved to, so a check can compare them with the
            // characters it expects under the pointer.
            guard let agent = query["agent"], let fx = query["from_x"].flatMap(Double.init), let fy = query["from_y"].flatMap(Double.init),
                  let tx = query["to_x"].flatMap(Double.init), let ty = query["to_y"].flatMap(Double.init) else {
                return refuse("400 Bad Request", "agent, from_x, from_y, to_x and to_y are required")
            }
            guard let chat = model.chats[agent] else { return refuse("404 Not Found", "no agent pane shows that agent") }
            guard let window = NSApp.wheelWindow, chat.frameInWindow != .zero else { return refuse("404 Not Found", "that transcript is not on screen") }
            let selection = chat.selection
            selection.mouseDown(at: CGPoint(x: fx, y: fy), extend: false, in: chat)
            selection.mouseDragged(to: CGPoint(x: tx, y: ty), in: chat, window: window)
            selection.mouseUp()
            NSPasteboard.general.clearContents()
            selection.copy(in: chat)
            let copied = NSPasteboard.general.string(forType: .string) ?? ""
            Log.info("debug: drag \(chat.name) (\(fx),\(fy)) -> (\(tx),\(ty)): copied \(String(reflecting: copied))")
            let answer: [String: Any] = ["pasteboard": copied,
                                        "anchor": selection.anchor.map { ["row": $0.row, "offset": $0.offset] as Any } ?? NSNull(),
                                        "head": selection.head.map { ["row": $0.row, "offset": $0.offset] as Any } ?? NSNull()]
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: answer, options: [.sortedKeys])) ?? Data())
        case ("POST", "/transcript/mouse"):
            // A pointer in the transcript: a press at window point (`x`, `y`, SwiftUI's global space,
            // top-left origin), then, with `to_x` and `to_y`, a drag to there in `steps` moves, then
            // the release. Each event goes to the transcript's mouse monitor, as the application would
            // hand it, and to the view under the press, which an application that is not active never
            // hands a window's events to. `shift=1` holds Shift (extends the selection), `clicks` is
            // the click count. The answer is the selection after the release, the copy ⌘C would make,
            // and the file preview a click opened.
            guard let session = query["agent"], let x = query["x"].flatMap(Double.init), let y = query["y"].flatMap(Double.init) else {
                return refuse("400 Bad Request", "session, x and y are required")
            }
            guard let chat = model.chats[session] else { return refuse("404 Not Found", "no agent pane shows that session") }
            guard let window = NSApp.wheelWindow, chat.frameInWindow != .zero else {
                return refuse("404 Not Found", "that transcript is not on screen")
            }
            let height = window.swiftUIHeight
            guard height > 0 else {
                return refuse("404 Not Found", "that transcript is not on screen")
            }
            let flags: NSEvent.ModifierFlags = query["shift"] == "1" ? [.shift] : []
            let clicks = query["clicks"].flatMap(Int.init) ?? 1
            let end = query["to_x"].flatMap(Double.init).flatMap { tx in query["to_y"].flatMap(Double.init).map { CGPoint(x: tx, y: $0) } }
            var number = 0
            var pressed: NSView?
            func post(_ type: NSEvent.EventType, _ point: CGPoint) {
                number += 1
                guard let event = NSEvent.mouseEvent(
                    with: type, location: NSPoint(x: point.x, y: height - point.y), modifierFlags: flags,
                    timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil,
                    eventNumber: number, clickCount: clicks, pressure: type == .leftMouseUp ? 0 : 1) else { return }
                chat.selection.debugHandle(event, in: chat)
                // As for a pointer, the drag and the release go to the view the press went to.
                if type == .leftMouseDown, let content = window.contentView {
                    pressed = content.hitTest(content.superview?.convert(NSPoint(x: point.x, y: height - point.y), from: nil) ?? .zero)
                    Log.info("debug: mouse press at (\(point.x),\(point.y)) goes to \(pressed.map { String(describing: Swift.type(of: $0)) } ?? "nothing")")
                }
                switch type {
                case .leftMouseDown: pressed?.mouseDown(with: event)
                case .leftMouseDragged: pressed?.mouseDragged(with: event)
                default: pressed?.mouseUp(with: event)
                }
                Self.spin(0.06)
            }
            let start = CGPoint(x: x, y: y)
            post(.leftMouseDown, start)
            if let end {
                let steps = max(query["steps"].flatMap(Int.init) ?? 6, 1)
                for step in 1...steps {
                    let t = Double(step) / Double(steps)
                    post(.leftMouseDragged, CGPoint(x: start.x + (end.x - start.x) * t, y: start.y + (end.y - start.y) * t))
                }
            }
            post(.leftMouseUp, end ?? start)
            Self.spin(0.3)
            let selection = chat.selection
            NSPasteboard.general.clearContents()
            selection.copy(in: chat)
            let copied = NSPasteboard.general.string(forType: .string) ?? ""
            Log.info("debug: mouse \(chat.name) (\(x),\(y))\(end.map { " -> (\($0.x),\($0.y))" } ?? "")\(flags.isEmpty ? "" : " shift"): copied \(String(reflecting: copied))")
            let answer: [String: Any] = ["pasteboard": copied, "key_window": window.isKeyWindow, "app_active": NSApp.isActive,
                                        "anchor": selection.anchor.map { ["row": $0.row, "offset": $0.offset] as Any } ?? NSNull(),
                                        "head": selection.head.map { ["row": $0.row, "offset": $0.offset] as Any } ?? NSNull()]
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: answer, options: [.sortedKeys])) ?? Data())
        case ("POST", "/transcript/key"):
            // A key press (`text=c`, `mods=cmd,shift,opt`) into the transcript's key handler, the one
            // the app's key monitor runs, with the key-window test waived (an app launched in the
            // background never is the key window). Answers whether the handler took the key, and
            // the pasteboard after it.
            guard let session = query["agent"], let text = query["text"], !text.isEmpty else {
                return refuse("400 Bad Request", "session and text are required")
            }
            guard let chat = model.chats[session] else { return refuse("404 Not Found", "no agent pane shows that session") }
            guard let window = NSApp.wheelWindow, chat.frameInWindow != .zero else { return refuse("404 Not Found", "that transcript is not on screen") }
            var flags: NSEvent.ModifierFlags = []
            for mod in (query["mods"] ?? "").split(separator: ",") {
                switch mod {
                case "cmd": flags.insert(.command)
                case "shift": flags.insert(.shift)
                case "opt": flags.insert(.option)
                default: return refuse("400 Bad Request", "mods are cmd, shift, opt")
                }
            }
            guard let event = NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: window.windowNumber, context: nil, characters: text, charactersIgnoringModifiers: text,
                isARepeat: false, keyCode: 0) else { return refuse("500 Internal Server Error", "could not make a key event") }
            NSPasteboard.general.clearContents()
            let taken = chat.selection.debugKey(event, in: chat, window: window)
            let copied = NSPasteboard.general.string(forType: .string) ?? ""
            Log.info("debug: key \(text) to \(chat.name)'s transcript handler: taken \(taken), pasteboard \(String(reflecting: copied))")
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: ["taken": taken, "pasteboard": copied], options: [.sortedKeys])) ?? Data())
        case ("GET", "/transcript/link"):
            // The address of the link drawn under window point (`x`, `y`, SwiftUI's global space), or
            // null: the lookup a tap on a reply makes, without the pointer.
            guard let session = query["agent"], let x = query["x"].flatMap(Double.init), let y = query["y"].flatMap(Double.init) else {
                return refuse("400 Bad Request", "session, x and y are required")
            }
            guard let chat = model.chats[session] else { return refuse("404 Not Found", "no agent pane shows that session") }
            let point = CGPoint(x: x, y: y)
            var found: String?
            for (row, target) in chat.selection.targets where target.frame.contains(point) {
                found = chat.selection.drawn[row]?.link(at: CGPoint(x: x - target.frame.minX, y: y - target.frame.minY))?.absoluteString
            }
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: ["url": found.map { $0 as Any } ?? NSNull()] as [String: Any])) ?? Data())
        case ("GET", "/transcript/drawn"):
            // The lines SwiftUI drew for a built reply and the prose offsets each one spans.
            guard let session = query["agent"], let row = query["row"].flatMap(Int.init) else {
                return refuse("400 Bad Request", "agent and row are required")
            }
            guard let chat = model.chats[session], let drawn = chat.selection.drawn[row] else { return refuse("404 Not Found", "row \(row) has no drawn text") }
            let item = chat.items.first { $0.id == row }
            let prose = item.map { chat.selection.drawnProse(of: $0, in: chat) }
            let body: [String: Any] = ["lines": drawn.described, "prose": prose?.text ?? "",
                                      "copies": (prose?.copies ?? []).map { ["from": $0.range.lowerBound, "to": $0.range.upperBound, "text": $0.text] as [String: Any] }]
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])) ?? Data())
        case ("GET", "/transcript/point"):
            // The window point (top-left origin) of a character offset in a built reply, from the
            // layout SwiftUI drew, for `/transcript/drag`.
            guard let agent = query["agent"], let row = query["row"].flatMap(Int.init), let offset = query["offset"].flatMap(Int.init) else {
                return refuse("400 Bad Request", "agent, row and offset are required")
            }
            guard let chat = model.chats[agent] else { return refuse("404 Not Found", "no agent pane shows that agent") }
            guard let target = chat.selection.targets[row], let item = chat.items.first(where: { $0.id == row }) else {
                return refuse("404 Not Found", "row \(row) is not built")
            }
            let inside = chat.selection.layout(of: item, target: target, in: chat).point(of: offset)
            let body: [String: Any] = ["x": target.frame.minX + inside.x, "y": target.frame.minY + inside.y]
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])) ?? Data())
        case ("POST", "/toast"):
            // level=notice|warning|error, text, optional id, pane, reply=<placeholder>; the reply is echoed to the log.
            guard let text = query["text"] else { return refuse("400 Bad Request", "text is required") }
            let id = query["id"] ?? "debug"
            let reply = query["reply"].map { placeholder in
                ToastReply(placeholder: placeholder) { Log.info("debug toast \(id): reply of \($0.count) characters") }
            }
            model.post(Toast(id: id, level: ToastLevel(rawValue: query["level"] ?? "") ?? .notice, text: text, reply: reply,
                             pane: query["pane"], dismiss: nil))
        case ("POST", "/toast/press"):
            // The button titled `button` (Copy and Dismiss included) on toast `id`: the same closures the buttons run.
            guard let id = query["id"], let button = query["button"], let toast = model.toasts.first(where: { $0.id == id }) else {
                return refuse("404 Not Found", "no such toast")
            }
            switch button {
            case "Copy": ToastView.copy(toast.text)
            case "Dismiss": model.dismiss(toast)
            default:
                guard let action = toast.actions.first(where: { $0.title == button }) else { return refuse("404 Not Found", "no such button") }
                action.run()
            }
        case ("POST", "/toast/clear"):
            model.removeToast(query["id"] ?? "debug")
        case ("POST", "/client/check"):
            model.checkClientUpdate()
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
        case ("POST", "/pane/focused"):
            guard let pane = query["pane"] else { return refuse("400 Bad Request", "pane is required") }
            model.paneFocused(pane: pane)
        case ("POST", "/pane/click"):
            // A press and release sent through the application's event path at the middle of a
            // pane's body (`at=body`, the default) or header (`at=header`), or at `x`,`y` (window
            // points, top-left origin), in `window` (an id prefix) or the key window. Answers the
            // pane the click monitor finds there and the point clicked.
            guard let window = wheelWindow(query["window"]), let height = window.contentView?.bounds.height else {
                return refuse("404 Not Found", "no such window")
            }
            let point: CGPoint
            if let x = query["x"].flatMap(Double.init), let y = query["y"].flatMap(Double.init) {
                point = CGPoint(x: x, y: y)
            } else if let pane = query["pane"], let frame = model.paneFrames[pane], let body = model.paneBodyFrames[pane] {
                point = query["at"] == "header" ? CGPoint(x: frame.midX, y: (frame.minY + body.minY) / 2) : CGPoint(x: body.midX, y: body.midY)
            } else {
                return refuse("404 Not Found", "pane is not on screen, or no x and y given")
            }
            let hit = model.pane(at: point, in: window)
            // `count=2` sends a second press and release that AppKit reads as a double-click.
            let count = min(max(query["count"].flatMap(Int.init) ?? 1, 1), 2)
            Log.info("debug: click at \(Int(point.x)),\(Int(point.y)) in \(window.identifier?.rawValue ?? "?"), pane \(hit ?? "none"), count \(count)")
            for click in 1...count {
                for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
                    guard let event = NSEvent.mouseEvent(
                        with: type, location: NSPoint(x: point.x, y: height - point.y), modifierFlags: [],
                        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil,
                        eventNumber: 0, clickCount: click, pressure: type == .leftMouseDown ? 1 : 0) else {
                        return refuse("500 Internal Server Error", "could not make a mouse event")
                    }
                    NSApp.sendEvent(event)
                }
            }
            let body = (try? JSONSerialization.data(withJSONObject: ["pane": hit.map { $0 as Any } ?? NSNull(), "x": point.x, "y": point.y])) ?? Data("{}".utf8)
            return ("200 OK", body)
        case ("POST", "/split"):
            guard let pane = query["pane"], let dir = query["dir"].flatMap(Dir.init(rawValue:)) else {
                return refuse("400 Bad Request", "pane and dir (left, right, up, down) are required")
            }
            model.split(pane, dir)
        case ("POST", "/start"):
            guard let pane = query["pane"], let kind = query["kind"].flatMap(StartKind.init(rawValue:)) else {
                return refuse("400 Bad Request", "pane and kind (terminal, agent) are required")
            }
            model.startPane(pane, kind, message: query["message"], agent: query["agent"])
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
        case ("POST", "/palette") where query["pane"] == nil:
            // The command palette over the window (no `pane`): open (⌘K), close (Esc), type
            // (`text=` into the field), select (`index=`, an arrow's or a click's landing) and run
            // (Return on the selected row).
            let action = query["action"] ?? ""
            Log.info("debug: command palette \(action)")
            if action != "open", model.commandPalette == nil { return refuse("409 Conflict", "the command palette is not open") }
            switch action {
            case "open":
                switch query["mode"] ?? "root" {
                case "root": model.openCommandPalette()
                case "new_space": model.openCommandPalette(mode: .newSpace)
                default: return refuse("400 Bad Request", "mode must be root or new_space")
                }
            case "close": model.closeCommandPalette()
            case "tab": model.commandPaletteTab()
            case "type":
                guard let text = query["text"] else { return refuse("400 Bad Request", "type takes text") }
                model.commandPaletteTyped(text)
            case "select":
                guard let index = query["index"].flatMap(Int.init), model.commandPalette?.rows.indices.contains(index) == true else {
                    return refuse("400 Bad Request", "select takes index=<a row's number>")
                }
                model.commandPaletteSelect(index)
            case "run": model.commandPaletteRun()
            default:
                return refuse("400 Bad Request", "action must be open, close, type, tab, select or run")
            }
        case ("POST", "/palette"):
            // The artifact palette's controls, as the keys and clicks drive them: open (the
            // "Artifact…" button), type (text into the field), move (by=±n, the arrows), enter
            // (Return: open the selected row, or list again after a failure), escape, click (a row,
            // by scope and name), confirm (the yes asked before a shell or agent is replaced) and
            // close (a click on the dimmed body).
            guard let pane = query["pane"], model.spaces.contains(where: { $0.allPanes.contains { $0.id == pane } }) else {
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
        case ("POST", "/procs"):
            // The process card's requests: `action=list` asks for `space`'s processes (what opening
            // the card does), `action=stop&pid=<n>` stops one the way the card's Stop button does.
            guard let space = query["space"] else { return refuse("400 Bad Request", "space=<id> is required") }
            switch query["action"] ?? "list" {
            case "list": model.requestProcs(space)
            case "stop":
                guard let pid = query["pid"].flatMap(UInt32.init) else { return refuse("400 Bad Request", "pid=<n> is required") }
                model.stopProc(space, pid: pid)
            default:
                return refuse("400 Bad Request", "action must be list or stop")
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
            // edit (typing), save (Return), cancel (Esc) and clear (the ✕ button); scroll brings a
            // key's row into view, as a person scrolling the page does, for a window shot.
            let action = query["action"] ?? ""
            Log.info("debug: settings \(action)")
            switch action {
            case "open": model.settings.openRequest += 1
            case "close": Self.settingsWindow?.close()
            case "tab":
                guard let tab = query["tab"].flatMap(SettingsTab.init(rawValue:)) else {
                    return refuse("400 Bad Request", "tab takes tab=general, agents, usage, keys, chat, terminal, artifacts, guidance, project or devices")
                }
                model.settings.tab = tab
            case "project":
                guard let id = query["id"], model.spaces.contains(where: { $0.id == id }) else {
                    return refuse("404 Not Found", "no space with that id")
                }
                model.selectSettingsProject(id)
            case "install": model.settings.commandLine.install()
            case "resize":
                // The window as a drag of its corner leaves it: AppKit clamps the size to the
                // window's minimum, so what /state reports is what a person would get.
                guard let window = Self.settingsWindow, window.styleMask.contains(.resizable),
                      let w = query["width"].flatMap(Double.init), let h = query["height"].flatMap(Double.init) else {
                    return refuse("400 Bad Request", "resize takes width and height and needs the open, resizable Settings window")
                }
                var frame = window.frame
                let size = CGSize(width: w, height: h)
                frame.origin.y += frame.height - size.height
                frame.size.width = max(size.width, window.minSize.width)
                frame.size.height = max(size.height, window.minSize.height)
                window.setFrame(frame, display: true)
            case "open_file":
                // The page's Open button; launch=true really starts the editor, otherwise the
                // press is only recorded in /state settings.opened_file.
                guard let layer = query["layer"].flatMap(SettingLayer.init(rawValue:)) else {
                    return refuse("400 Bad Request", "layer=global or layer=project is required")
                }
                guard model.settings.file(layer) != nil else { return refuse("404 Not Found", "the page names no file for that layer") }
                model.settings.openFile(layer, launch: query["launch"] == "true")
            case "guidance_preview":
                // The agent whose composed text Settings › Guidance's right column shows.
                guard let agent = query["agent"] else { return refuse("400 Bad Request", "guidance_preview takes agent=<name>") }
                model.previewGuidance(agent)
            case "guidance_focus":
                // A press on a document row on the left: the segment the right column scrolls to.
                model.settings.guidance.focus = query["id"]
            case "guidance_open":
                // Settings › Guidance's Open in Editor for `agent` (none: every agent); launch=true
                // really starts the editor, otherwise the press is only recorded in /state.
                model.openGuidanceFile(query["agent"], launch: query["launch"] == "true")
            case "chat_query", "terminal_query":
                model.settings.pages[action == "chat_query" ? .chat : .terminal, default: .init()].query = query["text"] ?? ""
            case "chat_layer", "terminal_layer":
                guard let layer = query["layer"].flatMap(SettingLayer.init(rawValue:)) else {
                    return refuse("400 Bad Request", "layer=global or layer=project is required")
                }
                model.settings.pages[action == "chat_layer" ? .chat : .terminal, default: .init()].layer = layer
            case "chat_toml", "terminal_toml":
                model.settings.pages[action == "chat_toml" ? .chat : .terminal, default: .init()].showToml = query["on"] != "false"
            case "scroll":
                guard let key = query["key"], model.settings.setting(key) != nil else {
                    return refuse("404 Not Found", "the window shows no setting with that key")
                }
                model.settings.scrollTarget = key
            case "edit", "save", "cancel", "clear":
                guard let layer = query["layer"].flatMap(SettingLayer.init(rawValue:)) else {
                    return refuse("400 Bad Request", "layer=global or layer=project is required")
                }
                guard let key = query["key"], model.settings.setting(key, in: layer) != nil else {
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
                return refuse("400 Bad Request", "action must be open, close, tab, project, install, open_file, chat_query, chat_layer, chat_toml, scroll, edit, save, cancel or clear")
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
            // `window=<id prefix>` names one of several Wheel windows; the key one otherwise.
            let named = switch query["which"] {
            case "settings": Self.settingsWindow
            case "usage": NSApp.windows.first(where: { $0.title == "Usage" })
            case "desktop": model.desktopOverlay?.nsWindow
            case "little": model.layout.littles.first { $0.id.hasPrefix(query["window"] ?? "") }.flatMap { (model.windowHost as? WindowManager)?.littleWindow($0.id) }
            case "pairing": model.pairingPanel?.panel
            case "popover": NSApp.windows.first { String(describing: type(of: $0)).contains("Popover") }
            default: wheelWindow(query["window"])
            }
            guard let window = sheet ? NSApp.windows.first(where: \.isSheet) : named else {
                return refuse("404 Not Found", sheet ? "no sheet is open" : "no Wheel window")
            }
            guard let view = (query["frame"] == "1" ? window.contentView?.superview : window.contentView),
                  let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
                return refuse("500 Internal Server Error", "the window could not be rendered")
            }
            view.cacheDisplay(in: view.bounds, to: rep)
            var out = rep
            do {
                // A window draws its background in the window, not in the content view, so
                // cacheDisplay leaves it transparent and a viewer paints its own ground behind the
                // PNG (light text vanishes there in dark appearance): lay the content over the
                // window colour in the view's own appearance.
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
            default: wheelWindow(query["window"])
            }
            guard let window = found else { return refuse("404 Not Found", "no such window") }
            if target.path == "/window/front" { window.orderFrontRegardless() } else { window.orderBack(nil) }
            Log.info("debug: window \(target.path == "/window/front" ? "in front" : "sent back")")
        case ("POST", "/popped/frame"):
            // Where a popped window sits, as its drag or its resize handle sets it at the end:
            // `pane` names any pane in the window; `x`, `y`, `w` and `h` are points in its space's grid.
            guard let pane = query["pane"],
                  let tree = model.spaces.lazy.flatMap(\.popped).first(where: { $0.panes.contains { $0.id == pane } })
            else { return refuse("404 Not Found", "no popped pane with that id") }
            let n = ["x", "y", "w", "h"].compactMap { query[$0].flatMap(Double.init) }
            guard n.count == 4 else { return refuse("400 Bad Request", "x, y, w and h are required") }
            model.setPoppedFrame(CGRect(x: n[0], y: n[1], width: n[2], height: n[3]), of: tree)
        case ("POST", "/popped/edge-drop"):
            // A popped window dropped on its space's edge, without a pointer: the model call the
            // window's drag makes when it ends over the edge.
            guard let pane = query["pane"],
                  let tree = model.spaces.lazy.flatMap(\.popped).first(where: { $0.panes.contains { $0.id == pane } })
            else { return refuse("404 Not Found", "no popped pane with that id") }
            model.backgroundWindow(tree)
        case ("POST", "/window/key"):
            // Make the main window the app's key window without activating the app, so key events
            // and clicks sent through the application reach it as they would reach a focused one.
            // Answers whether the window is key and whether the app is active.
            guard let window = wheelWindow(query["window"]) else { return refuse("404 Not Found", "no such window") }
            window.makeKeyAndOrderFront(nil)
            Log.info("debug: window made key: key \(window.isKeyWindow), app active \(NSApp.isActive)")
            let info: [String: Any] = ["key_window": window.isKeyWindow, "app_active": NSApp.isActive, "visible": window.occlusionState.contains(.visible)]
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: info)) ?? Data())
        case ("POST", "/app/activate"):
            // Make the app active with the main window key, as a click on it would: a check that
            // drives the real pointer needs the application to receive window events.
            guard let window = wheelWindow(query["window"]) else { return refuse("404 Not Found", "no such window") }
            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)
            Self.spin(0.5)
            Log.info("debug: app activated: key \(window.isKeyWindow), app active \(NSApp.isActive)")
            let info: [String: Any] = ["key_window": window.isKeyWindow, "app_active": NSApp.isActive]
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: info)) ?? Data())
        case ("GET", "/window/screen"):
            // Where SwiftUI window point (`x`, `y`, top-left origin including the title bar) is on the
            // display, in the top-left-origin space CGEvent posts in, for a check that moves the real pointer.
            guard let x = query["x"].flatMap(Double.init), let y = query["y"].flatMap(Double.init) else {
                return refuse("400 Bad Request", "x and y are required")
            }
            guard let window = wheelWindow(query["window"]), window.swiftUIHeight > 0, let main = NSScreen.screens.first else {
                return refuse("404 Not Found", "no such window")
            }
            let onScreen = window.convertPoint(toScreen: NSPoint(x: x, y: window.swiftUIHeight - y))
            let body: [String: Any] = ["x": onScreen.x, "y": main.frame.height - onScreen.y, "app_active": NSApp.isActive]
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])) ?? Data())
        case ("POST", "/input/mouse"), ("POST", "/input/key"), ("GET", "/input/cursor"), ("GET", "/input/views"):
            return realInput(target.path, query)
        case ("POST", "/pane/drag"):
            // A header drag without a pointer: `action` is start (`pane`), hover (`target` and
            // `zone`: center, left, right, up or down; a target in another space takes a side), hover
            // with `space` (a tab, row or popover row: the end of that space; the held wait runs as
            // for a pointer), `newspace` (a window id: a new space there), `desktop=1`, or `at=x,y`
            // (a screen point from `GET /geometry`, run through the real hit test), drop or cancel.
            // The same model calls the pointer's gesture makes.
            Log.info("debug: pane drag \(query["action"] ?? "-")")
            switch query["action"] {
            case "start":
                let held = Set(model.spaces.flatMap { $0.allPanes.map(\.id) })
                guard let pane = query["pane"], held.contains(pane) else { return refuse("404 Not Found", "no pane with that id") }
                model.beginPaneDrag(pane, fresh: true)
            case "hover":
                guard model.paneDrag != nil else { return refuse("409 Conflict", "no drag in progress") }
                if let space = query["space"] {
                    guard model.spaces.contains(where: { $0.id == space }) else { return refuse("404 Not Found", "no space with that id") }
                    model.hoverPaneAway(.end(space: space))
                    break
                }
                if let window = query["newspace"] {
                    guard model.layout.window(window) != nil else { return refuse("404 Not Found", "no window with that id") }
                    model.hoverPaneAway(.newSpace(window: window))
                    break
                }
                if query["desktop"] != nil {
                    model.hoverPaneAway(.desktop)
                    break
                }
                if let at = query["at"] {
                    let parts = at.split(separator: ",").compactMap { Double($0) }
                    guard parts.count == 2 else { return refuse("400 Bad Request", "at is x,y") }
                    model.movePaneDrag(at: CGPoint(x: parts[0], y: parts[1]))
                    break
                }
                guard let target = query["target"], let zone = query["zone"] else { return refuse("400 Bad Request", "target and zone, space, newspace, desktop or at are required") }
                let parsed: DropZone? = zone == "center" ? .center : Dir(rawValue: zone).map { .edge($0) }
                guard let parsed else { return refuse("400 Bad Request", "zone is center, left, right, up or down") }
                model.hoverPaneDrag(PaneDrop(target: target, zone: parsed))
            case "drop": model.endPaneDrag()
            case "cancel": model.cancelPaneDrag()
            default: return refuse("400 Bad Request", "action is start, hover, drop or cancel")
            }
        case ("POST", "/windows"):
            // The window layout's own operations, the calls the menus, the popover and drags make:
            // `action` is new (`name`, `spaces` comma-separated), close (`window`), place (`space`,
            // `window` or `window=background`, `at`), tearoff (`space`), rename (`window`, `name`),
            // sidebar (`window`, `open`), mode (`window`, `mode`), pin (`window` or `space`, `on`), focus (`window`), quit-flag (`quitting`) and reopen.
            Log.info("debug: windows \(query)")
            func windowID(_ key: String = "window") -> String? {
                guard let text = query[key] else { return nil }
                if case let .success(id) = model.windowID(named: text) { return id }
                return nil
            }
            switch query["action"] {
            case "new":
                let id = model.newWindow(name: query["name"], session: query["session"], space: query["space"])
                return ("200 OK", Data(#"{"window":"\#(id)"}"#.utf8))
            case "session":
                // A window shows another session (`window`, `session`); the title bar's session menu runs the same.
                guard let id = windowID() ?? model.layout.focusedWindow?.id else { return refuse("404 Not Found", "no such window") }
                guard let session = query["session"], model.layout.session(session) != nil else { return refuse("404 Not Found", "no such session") }
                model.show(session: session, in: id)
            case "newsession":
                guard let name = query["name"] else { return refuse("400 Bad Request", "name is required") }
                model.newSession(named: name)
            case "renamesession":
                guard let session = query["session"], let name = query["name"] else { return refuse("400 Bad Request", "session and name are required") }
                model.renameSession(session, to: name)
            case "closesession":
                guard let session = query["session"] else { return refuse("400 Bad Request", "session is required") }
                model.closeSession(session)
            case "movespace":
                guard let space = query["space"], let session = query["session"] else { return refuse("400 Bad Request", "space and session are required") }
                model.moveSpace(space, toSession: session, at: query["at"].flatMap(Int.init))
            case "close":
                guard let id = windowID() else { return refuse("404 Not Found", "no such window") }
                model.askCloseWindow(id)
            case "closeanswer":
                // The close sheet's buttons: `answer` is background, stop or cancel.
                guard model.closingWindow != nil else { return refuse("409 Conflict", "no close sheet is showing") }
                let answers: [String: AppModel.WindowCloseAnswer] = ["background": .background, "stop": .stop, "cancel": .cancel]
                guard let answer = query["answer"].flatMap({ answers[$0] }) else { return refuse("400 Bad Request", "answer is background, stop or cancel") }
                model.answerWindowClose(answer)
            case "closebutton":
                // What the red close button does: the host asks the model, which shows the sheet when the window holds spaces.
                guard let id = windowID() else { return refuse("404 Not Found", "no such window") }
                (model.windowHost as? WindowManager)?.window(id)?.performClose(nil)
            case "place":
                guard let space = query["space"] else { return refuse("400 Bad Request", "space is required") }
                let target = query["window"] == "background" ? nil : windowID()
                if query["window"] != "background", target == nil { return refuse("404 Not Found", "no such window") }
                guard model.place(space, in: target, at: query["at"].flatMap(Int.init)) else { return refuse("404 Not Found", "no such space") }
            case "tearoff":
                guard let space = query["space"], model.tearOff(space) != nil else { return refuse("404 Not Found", "no such space") }
            case "little":
                // The space menu's Open in Little Window (`space`).
                guard let text = query["space"], case let .success(space) = model.spaceID(named: text) else { return refuse("404 Not Found", "no such space") }
                guard model.openLittle(for: space) != nil else { return refuse("409 Conflict", "the layout places that space nowhere") }
            case "littleback":
                // Bring Back, the tab's back icon and the little window's back icon, which all close the little window of `space`.
                guard let text = query["space"], case let .success(space) = model.spaceID(named: text), model.layout.little(showing: space) != nil else { return refuse("404 Not Found", "no little window shows that space") }
                model.bringBack(space: space)
            case "littlegoto":
                guard let text = query["space"], case let .success(space) = model.spaceID(named: text), let little = model.layout.little(showing: space) else { return refuse("404 Not Found", "no little window shows that space") }
                model.goToLittle(little.id)
            case "littleclosebutton":
                // What the little window's red light does.
                guard let text = query["space"], case let .success(space) = model.spaceID(named: text), let little = model.layout.little(showing: space) else { return refuse("404 Not Found", "no little window shows that space") }
                (model.windowHost as? WindowManager)?.littleWindow(little.id)?.performClose(nil)
            case "rename":
                guard let id = windowID() else { return refuse("404 Not Found", "no such window") }
                model.renameWindow(id, to: query["name"])
            case "pin":
                // The pin button in a main window's strip (`window`) or a little window's header (`space`); `on` is true or false.
                let on = query["on"] != "false"
                if let space = query["space"] {
                    guard case let .success(id) = model.spaceID(named: space), let little = model.layout.little(showing: id) else { return refuse("404 Not Found", "no little window shows that space") }
                    model.setPinned(on, little: little.id)
                } else {
                    guard let id = windowID() else { return refuse("404 Not Found", "no such window") }
                    model.setPinned(on, window: id)
                }
            case "sidebar":
                guard let id = windowID() else { return refuse("404 Not Found", "no such window") }
                model.setSidebar(open: query["open"] != "false", in: id)
            case "mode":
                guard let id = windowID(), let mode = query["mode"], WindowMode.builtIn.contains(mode) else {
                    return refuse("400 Bad Request", "window and a mode (\(WindowMode.builtIn.joined(separator: ", "))) are required")
                }
                model.setMode(mode, in: id)
            case "focus":
                guard let id = windowID() else { return refuse("404 Not Found", "no such window") }
                model.windowHost?.bringFront(id)
                model.windowBecameFocused(id)
            case "frame":
                // Place a window (`x`, `y`, `w`, `h` in AppKit's screen coordinates) so a drag check can
                // aim at each one without another covering it.
                guard let id = windowID(), let window = (model.windowHost as? WindowManager)?.window(id),
                      let x = query["x"].flatMap(Double.init), let y = query["y"].flatMap(Double.init),
                      let w = query["w"].flatMap(Double.init), let h = query["h"].flatMap(Double.init) else {
                    return refuse("400 Bad Request", "window, x, y, w and h are required")
                }
                window.setFrame(NSRect(x: x, y: y, width: w, height: h), display: true)
            case "sweep":
                // A window resize as a person's drag makes one: every 16 ms for `seconds` (default 2.5)
                // the width swings `by` points (default 300) either side of where it starts, twice, then
                // returns to it. Answers at once; the steps run on the main run loop, so `/metrics`
                // records them without a request per step.
                guard let id = windowID(), let window = (model.windowHost as? WindowManager)?.window(id) else {
                    return refuse("404 Not Found", "no such window")
                }
                let seconds = query["seconds"].flatMap(Double.init) ?? 2.5
                let by = query["by"].flatMap(Double.init) ?? 300
                let start = window.frame, began = Date()
                model.windowResize(began: true)
                func step() {
                    let t = Date().timeIntervalSince(began)
                    var frame = start
                    if t < seconds { frame.size.width += by * sin(t / seconds * 4 * .pi) }
                    window.setFrame(frame, display: true)
                    if t < seconds { DispatchQueue.main.asyncAfter(deadline: .now() + 1.0 / 60) { step() } } else { model.windowResize(began: false) }
                }
                step()
            case "reopen":
                model.openWindowForBackground()
            default:
                return refuse("400 Bad Request", "action takes new, close, closebutton, place, tearoff, rename, sidebar, mode, pin, focus, frame, sweep or reopen")
            }
        case ("POST", "/pairing"):
            // The pairing row and window: `action` is copy, show, close, switch (`on`), details
            // (`on`), and clear-clipboard, the minute's end without the minute.
            Log.info("debug: pairing \(query)")
            switch query["action"] {
            case "copy":
                guard model.pairingDisabledReason == nil else { return refuse("409 Conflict", model.pairingDisabledReason ?? "") }
                model.copyPairingCode()
            case "copy_image": model.copyPairingImage()
            case "show":
                guard model.pairingDisabledReason == nil else { return refuse("409 Conflict", model.pairingDisabledReason ?? "") }
                model.showPairingWindow()
            case "close": model.closePairingWindow()
            case "switch": model.pairing.closeOnConnect = query["on"] != "false"
            case "details": model.pairing.detailsOpen = query["on"] != "false"
            default: return refuse("400 Bad Request", "action is copy, copy_image, show, close, switch or details")
            }
        case ("POST", "/server"):
            // The header's controls: `action` is expand, collapse, stop (asks first), confirm,
            // cancel, start, restart or dismiss_banner: the calls the
            // buttons make.
            Log.info("debug: server \(query)")
            switch query["action"] {
            case "expand": model.serverExpanded = true
            case "collapse": model.serverExpanded = false
            case "stop": model.askToStopServer()
            case "confirm":
                guard model.serverConfirming else { return refuse("409 Conflict", "Stop Server was not asked first") }
                model.stopServer()
            case "cancel": model.cancelStopServer()
            case "start": model.startServer()
            case "restart": model.restartServer()
            case "dismiss_banner": model.dismissServerBanner()
            default: return refuse("400 Bad Request", "action is expand, collapse, stop, confirm, cancel, start, restart or dismiss_banner")
            }
        case ("POST", "/spacedrag"):
            // A space drag without a pointer: `action` is start (`space`), hover, drop or cancel.
            // `hover` takes `target` (sidebar:<window>:<index>, tabs:<window>:<index>, window:<window>, card:<window or
            // background>[:<index>], desktop) or `at=x,y`, a screen point the real hit test runs on.
            Log.info("debug: space drag \(query)")
            switch query["action"] {
            case "start":
                guard let space = query["space"], model.spaces.contains(where: { $0.id == space }) else { return refuse("404 Not Found", "no such space") }
                model.beginSpaceDrag(space)
            case "hover":
                guard model.spaceDrag != nil else { return refuse("409 Conflict", "no space drag in progress") }
                if let at = query["at"]?.split(separator: ",").compactMap({ Double($0) }), at.count == 2 {
                    model.moveSpaceDrag(at: CGPoint(x: at[0], y: at[1]))
                } else if let spec = query["target"] {
                    let parts = spec.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
                    func window(_ text: String) -> String? { if case let .success(id) = model.windowID(named: text) { id } else { nil } }
                    func session(_ text: String) -> String? { model.sessions.first { $0.id == text || $0.name == text }?.id }
                    let target: SpaceDropTarget?
                    switch (parts.first, parts.count) {
                    case ("sidebar", 3): target = window(parts[1]).flatMap { id in Int(parts[2]).map { .sidebar(window: id, index: $0) } }
                    case ("tabs", 3): target = window(parts[1]).flatMap { id in Int(parts[2]).map { .tabs(window: id, index: $0) } }
                    case ("window", 2): target = window(parts[1]).map { .window($0) }
                    case ("card", 2), ("card", 3):
                        // Without an index the space goes to the end of the group.
                        let key = session(parts[1])
                        let held = key.map { model.spaces(inSession: $0).count } ?? 0
                        let index = parts.count == 3 ? Int(parts[2]) : held
                        target = key.flatMap { key in index.map { .card(session: key, index: $0) } }
                    case ("desktop", 1): target = .desktop
                    default: target = nil
                    }
                    guard let target else { return refuse("400 Bad Request", "target is sidebar:<window>:<index>, tabs:<window>:<index>, window:<window>, card:<session id or name>[:<index>] or desktop") }
                    model.hoverSpaceDrag(target)
                } else {
                    return refuse("400 Bad Request", "hover takes target or at")
                }
            case "drop": model.endSpaceDrag()
            case "cancel": model.cancelSpaceDrag()
            default: return refuse("400 Bad Request", "action is start, hover, drop or cancel")
            }
        case ("POST", "/spacetap"):
            // What a click on a space's tab or sidebar row runs: `space` is shown in the window
            // that holds it, timed as a click is. The tab's hit area is the view's own and is checked by eye.
            Log.info("debug: space tap \(query)")
            guard case let .success(id) = model.spaceID(named: query["space"] ?? "") else { return refuse("404 Not Found", "no such space") }
            guard let record = model.layout.window(holding: id) else { return refuse("409 Conflict", "that space is not shown in a window") }
            model.tapSpace(id, in: record.id)
        case ("GET", "/metrics"), ("POST", "/metrics"):
            // What a layout change costs (`GridMetrics`): `POST /metrics?action=start` begins a
            // recording, `?action=stop` ends it, and every reply (and `GET`) is the numbers so far.
            switch query["action"] {
            case "start": GridMetrics.shared.start()
            case "stop": GridMetrics.shared.stop()
            case nil: break
            default: return refuse("400 Bad Request", "action is start or stop")
            }
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: GridMetrics.shared.report(), options: [.sortedKeys])) ?? Data())
        case ("POST", "/chat/hover"):
            // Hold a pointer at a window point (`x`, `y`, top left origin, the window content's own
            // coordinates) so the controls under it show their hover state as a real pointer
            // causes it; no `x` and `y` lets it go.
            if let x = query["x"].flatMap(Double.init), let y = query["y"].flatMap(Double.init) {
                Log.info("debug: chat pointer held at \(Int(x)),\(Int(y))")
                ChatControlRegistry.shared.hold(at: CGPoint(x: x, y: y))
            } else {
                Log.info("debug: chat pointer released")
                ChatControlRegistry.shared.hold(at: nil)
            }
            return ("200 OK", Data("{}".utf8))
        case ("GET", "/chat/control"):
            // The chat control under a window point (`x`, `y`; the held pointer when omitted) and
            // whether its hover state is on, or `{}` when none is.
            let registry = ChatControlRegistry.shared
            let at = query["x"].flatMap(Double.init).flatMap { x in query["y"].flatMap(Double.init).map { CGPoint(x: x, y: $0) } } ?? registry.probe
            let found = at.flatMap { registry.control(at: $0) }
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: found.map(Self.controlJSON) ?? [:], options: [.sortedKeys])) ?? Data())
        case ("GET", "/chat/controls"):
            let all = ChatControlRegistry.shared.all.filter { $0.frame != .zero }.sorted { ($0.frame.minY, $0.frame.minX) < ($1.frame.minY, $1.frame.minX) }
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: all.map(Self.controlJSON), options: [.sortedKeys])) ?? Data())
        case ("GET", "/geometry"):
            // Where each window, sidebar row and popover card is on screen (AppKit's coordinates,
            // origin bottom left): what a real drag is aimed at.
            return ("200 OK", (try? JSONSerialization.data(withJSONObject: model.geometryJSON(), options: [.sortedKeys])) ?? Data())
        case ("POST", "/rename"):
            // Renaming as the fields do it: `kind` is space or window, `id` names it, `action` is
            // begin (the menu item), commit (Return, with `name`) or cancel (Escape).
            Log.info("debug: rename \(query)")
            guard let kind = query["kind"], let text = query["id"] else { return refuse("400 Bad Request", "kind and id are required") }
            let id: String
            if kind == "window" {
                guard case let .success(found) = model.windowID(named: text) else { return refuse("404 Not Found", "no such window") }
                id = found
            } else {
                guard case let .success(found) = model.spaceID(named: text) else { return refuse("404 Not Found", "no such space") }
                id = found
            }
            switch (kind, query["action"]) {
            case ("space", "begin"): model.beginRename(space: id)
            case ("window", "begin"): model.beginRename(window: id)
            case ("space", "commit"): model.renameSpace(id, to: query["name"] ?? "")
            case ("window", "commit"): model.commitRename(window: id, to: query["name"] ?? "")
            case (_, "cancel"): model.endRenaming()
            default: return refuse("400 Bad Request", "kind is space or window; action is begin, commit or cancel")
            }
        case ("POST", "/menubar"):
            // The menu bar item: `action` is open or close (the status item's own click),
            // snapshot (`path`: the popover window drawn as a PNG; `target=icon` the status item image), send_window (`space`, `window`
            // or `new`), send_background (`space`) or show (`space`): the calls the popover's
            // buttons make.
            Log.info("debug: menubar \(query)")
            switch query["action"] {
            case "open", "close":
                let open = query["action"] == "open"
                if Self.popoverWindow()?.isVisible == open { break }
                guard let button = Self.statusButton() else { return refuse("404 Not Found", "no menu bar item (\(Self.windowClasses()))") }
                button.performClick(nil)
            case "preview":
                // The popover's content in an ordinary window of its own, which a capture can draw
                // faithfully (the status item's own window is drawn with materials the view cache cannot reproduce).
                let host = NSHostingView(rootView: MenuBarPopover(model: model).background(Color(nsColor: .windowBackgroundColor)))
                let size = host.fittingSize
                host.frame = NSRect(origin: .zero, size: size)
                let window = Self.preview ?? NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
                window.contentView = host
                window.setContentSize(size)
                window.isReleasedWhenClosed = false
                window.orderFrontRegardless()
                Self.preview = window
            case "snapshot":
                guard let path = query["path"] else { return refuse("400 Bad Request", "path is required") }
                // `target=icon` writes the status item's image as it is drawn now, at 4x.
                if query["target"] == "icon" {
                    let icon = MenuBarIcon.image(background: model.windowlessSpaces.count, update: model.serverRestartWaiting)
                    let size = NSSize(width: icon.size.width * 4, height: icon.size.height * 4)
                    let big = NSImage(size: size, flipped: false) { rect in
                        NSColor.white.setFill(); rect.fill()
                        icon.draw(in: rect)
                        return true
                    }
                    guard let tiff = big.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
                          let png = rep.representation(using: .png, properties: [:]) else { return refuse("500 Internal Server Error", "no png") }
                    do { try png.write(to: URL(filePath: path)) } catch { return refuse("500 Internal Server Error", "could not write \(path): \(error)") }
                    break
                }
                // `target=preview` draws the hover preview instead of the popover.
                let view: NSView
                if query["target"] == "preview" {
                    guard let panel = model.previewPanel, panel.isShown, let content = panel.contentView else {
                        return refuse("409 Conflict", "the hover preview is not open")
                    }
                    view = content
                } else {
                    guard let found = Self.popoverWindow().flatMap({ $0.isVisible ? $0 : nil }) ?? Self.preview, found.isVisible, let content = found.contentView else {
                        return refuse("409 Conflict", "the popover is not open (\(Self.windowClasses()))")
                    }
                    view = content
                }
                // The window's own view drawn into a bitmap: no screen recording is involved.
                guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return refuse("500 Internal Server Error", "no bitmap") }
                view.cacheDisplay(in: view.bounds, to: rep)
                guard let png = rep.representation(using: .png, properties: [:]) else { return refuse("500 Internal Server Error", "no png") }
                do { try png.write(to: URL(filePath: path)) } catch { return refuse("500 Internal Server Error", "could not write \(path): \(error)") }
            case "send_window":
                guard let space = query["space"] else { return refuse("400 Bad Request", "space is required") }
                if query["window"] == "new" {
                    guard model.tearOff(space) != nil else { return refuse("404 Not Found", "no such space") }
                } else {
                    guard let text = query["window"], case let .success(id) = model.windowID(named: text) else { return refuse("404 Not Found", "no such window") }
                    guard model.place(space, in: id) else { return refuse("404 Not Found", "no such space") }
                }
            case "show":
                guard let space = query["space"] else { return refuse("400 Bad Request", "space is required") }
                model.focus(space: space)
            case "place":
                // A row's icon button, by the id `/state` lists under `menu_bar.rows`.
                guard let space = query["space"], let button = query["button"] else { return refuse("400 Bad Request", "space and button are required") }
                guard model.runPlace(button, space: space) else { return refuse("404 Not Found", "no button \(button) on a row for \(space)") }
            case "hover":
                // The pointer over a row (its buttons show), or off every row when `space` is empty.
                // It drives the preview the way the pointer does: open after the delay, closed on leaving.
                if let hovered = query["space"].flatMap({ $0.isEmpty ? nil : $0 }) {
                    model.rowHover(hovered, inside: true)
                } else if let held = model.popoverHover {
                    model.rowHover(held, inside: false)
                }
            case "icon":
                guard let path = query["path"] else { return refuse("400 Bad Request", "path is required") }
                let image = MenuBarIcon.image(background: model.windowlessSpaces.count)
                guard let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
                      let png = rep.representation(using: .png, properties: [:]) else { return refuse("500 Internal Server Error", "no png") }
                do { try png.write(to: URL(filePath: path)) } catch { return refuse("500 Internal Server Error", "could not write \(path): \(error)") }
            default:
                return refuse("400 Bad Request", "action takes open, close, snapshot, preview, send_window, send_background, show, place, hover or icon")
            }
        case ("POST", "/sidebar"):
            // The sidebar toggle and its rows: `open` is true, false or toggle. `space` clicks that
            // space's row.
            Log.info("debug: sidebar \(query)")
            let open: Bool?
            switch query["open"] {
            case nil: open = nil
            case "true": open = true
            case "false": open = false
            case "toggle": open = !model.sidebarOpen
            default: return refuse("400 Bad Request", "open is true, false or toggle")
            }
            if let space = query["space"], !model.spaces.contains(where: { $0.id == space }) {
                return refuse("404 Not Found", "no space with that id")
            }
            if let open { model.setSidebar(open: open) }
            if let space = query["space"] { model.focus(space: space) }
        case ("POST", "/a2ui/action"):
            // An A2UI button's action, raised without a pointer: `name` is the action event's name,
            // and every other query item is a string in its context.
            guard let pane = query["pane"], let name = query["name"] else { return refuse("400 Bad Request", "pane and name are required") }
            guard let a2ui = model.a2uiPanes[pane], a2ui.surface != nil else { return refuse("404 Not Found", "pane \(pane) draws no A2UI surface") }
            a2ui.press(name, context: query.filter { $0.key != "pane" && $0.key != "name" })
        case ("POST", "/context/toggle"):
            guard let pane = query["pane"] else { return refuse("400 Bad Request", "pane is required") }
            model.toggleContextLabel(pane)
        case ("POST", "/pane/menu"):
            // A pick from the pane header's context menu, by the item id `/state` lists under `menu`.
            guard let pane = query["pane"], let item = query["item"] else { return refuse("400 Bad Request", "pane and item are required") }
            guard let found = model.spaces.lazy.flatMap({ $0.allPanes }).first(where: { $0.id == pane }) else {
                return refuse("404 Not Found", "no pane \(pane)")
            }
            guard let entry = model.headerMenu(for: pane, kind: found.kind).first(where: { $0.id == item }), entry.enabled else {
                return refuse("409 Conflict", "no enabled menu item \(item)")
            }
            if item == "close" { model.closePane(pane) } else { model.runHeaderMenu(item, pane: pane) }
        case ("POST", "/pane/header"):
            // Where a pane kind's header puts a control, as Settings › Pane header does: `kind`, `control` (a `HeaderControl` name) and `placement` bar|menu|off, or `reset` for the kind.
            guard let kind = query["kind"] else { return refuse("400 Bad Request", "kind is required") }
            if query["reset"] != nil { model.resetHeaderLayout(kind: kind); break }
            guard let control = query["control"].flatMap(HeaderControl.init(rawValue:)), let placement = query["placement"].flatMap(HeaderPlacement.init(rawValue:)) else {
                return refuse("400 Bad Request", "control and placement are required")
            }
            model.setHeaderPlacement(control, kind: kind, to: placement)
        case ("POST", "/pane/button"):
            // A press of a pane header's own button, by the id `/state` lists under `header_buttons`.
            guard let pane = query["pane"], let button = query["button"] else { return refuse("400 Bad Request", "pane and button are required") }
            guard model.spaces.lazy.flatMap({ $0.allPanes }).contains(where: { $0.id == pane }) else {
                return refuse("404 Not Found", "no pane \(pane)")
            }
            switch button {
            case "pop": model.toggleSite(pane, .popped)
            case "background": model.toggleSite(pane, .backgrounded)
            case "screenshot": model.copyPaneImage(pane)
            case "conversations": model.conversationsPane = pane
            case "conversations-close": model.conversationsPane = nil
            case "tray-open":
                guard let space = model.space(holding: pane) else { return refuse("404 Not Found", "no pane \(pane)") }
                model.setTrayOpen(space.id, true)
            case "tray-row": model.setSite(pane, .grid)
            case "tray-pop": model.setSite(pane, .popped)
            case "zoom" where model.zoomButton(for: pane).enabled: model.toggleZoom(pane)
            default: return refuse("409 Conflict", "no enabled header button \(button)")
            }
        case ("POST", "/conversations/pick"):
            // What a click on a row of the conversations list sends: `id=new` or a conversation's id.
            guard let pane = query["pane"], let id = query["id"] else { return refuse("400 Bad Request", "pane and id are required") }
            guard case let .agent(agent, _, _)? = model.spaces.lazy.flatMap({ $0.allPanes }).first(where: { $0.id == pane })?.kind,
                  let chat = model.chats[agent] else { return refuse("404 Not Found", "no agent pane \(pane)") }
            let actions = model.conversationActions(chat, pane: pane)
            if id == "new" { actions.startNew() } else { actions.resume(id) }
        case ("POST", "/conversations/load"):
            guard let pane = query["pane"] else { return refuse("400 Bad Request", "pane is required") }
            guard case let .agent(agent, _, _)? = model.spaces.lazy.flatMap({ $0.allPanes }).first(where: { $0.id == pane })?.kind,
                  let chat = model.chats[agent] else { return refuse("404 Not Found", "no agent pane \(pane)") }
            model.conversationActions(chat, pane: pane).load()
        case ("POST", "/space/menu"):
            // A pick from a space's context menu (tab or sidebar row), by the item id
            // `/state` lists under `space_menus`, in the window that holds the space.
            guard let space = query["space"], let item = query["item"] else { return refuse("400 Bad Request", "space and item are required") }
            // A space in a session no window shows (a popover row) has no window.
            let window = model.layout.window(holding: space)?.id
            guard model.layout.sessions.session(of: space) != nil else { return refuse("404 Not Found", "no session holds space \(space)") }
            guard let entry = model.spaceMenu(for: space, in: window).first(where: { $0.id == item }), entry.enabled else {
                return refuse("409 Conflict", "no enabled menu item \(item)")
            }
            model.runSpaceMenu(item, space: space, in: window)
        case ("POST", "/pane/close"):
            guard let pane = query["pane"] else { return refuse("400 Bad Request", "pane is required") }
            model.closePane(pane)
        case ("POST", "/action"):
            // A menu item or palette row by its action id, as `keymap` in the state names them.
            guard let id = query["id"] else { return refuse("400 Bad Request", "id is required") }
            guard CommandActions.entry(id, model) != nil else { return refuse("404 Not Found", "no action \(id)") }
            CommandActions.run(id, model)
        case ("POST", "/key"):
            // A key pressed the way a keyboard sends it: `keys` in wheeld's `keybind` spelling
            // (`shift+cmd+d`, `t`), offered in the order a key window's keys go — the keymap's
            // monitor, the window's views (a terminal first), then the first responder's keyDown.
            // Each is called here because AppKit hands an app that is not active no keys, and the
            // checks run the app in the background. The main menu is left out: NSMenu matches a
            // key event made in code against its items without its modifier flags (⌘D runs a
            // ⇧⌘D item), so what it did with one would say nothing about a real key. A window's
            // keyDown hands a ⌘ or ⌃ key to the menu too, so such a key stops before it.
            // `which=sheet` sends it to the sheet over the window, as Return or Esc in a confirmation.
            let receiver = query["which"] == "sheet" ? NSApp.windows.first(where: \.isSheet) : NSApp.wheelWindow
            guard let keys = query["keys"], let chord = KeyChord(keys), let window = receiver else {
                return refuse("400 Bad Request", "keys in wheeld's spelling (shift+cmd+d) and a Wheel window (or sheet) are required")
            }
            guard let event = KeyPress.event(chord, window: window) else {
                return refuse("400 Bad Request", "no key code for \(chord.key)")
            }
            let responder = responderName(window)
            let taker: String
            if model.runKey(event) {
                taker = "the keymap"
            } else if window.performKeyEquivalent(with: event) {
                taker = "a view's key equivalent"
            } else if !chord.modifiers.isDisjoint(with: [.cmd, .ctrl]) {
                taker = "nothing (the menus are left out)"
            } else {
                window.sendEvent(event)
                if let up = KeyPress.event(chord, window: window, type: .keyUp) { window.sendEvent(up) }
                taker = "keyDown"
            }
            Log.info("debug: key \(chord.text) (\(chord.symbol)) with \(responder) holding the keyboard: taken by \(taker)")
        case ("POST", "/builtins"):
            // Settings › Artifacts: `action=ls`, `install|update|remove&name=` (a row's buttons;
            // remove is the alert's yes), and `edited&choice=keep|replace|cancel` (the edited-files
            // sheet's buttons).
            let name = query["name"] ?? ""
            switch query["action"] {
            case "ls": model.askBuiltins()
            case "install": model.installBuiltin(name)
            case "update": model.updateBuiltin(name)
            case "remove": model.removeBuiltin(name)
            case "edited":
                guard let edited = model.builtinsModel.edited else { return refuse("409 Conflict", "no edited-files sheet is open") }
                switch query["choice"] {
                case "keep": model.updateBuiltin(edited.name, onEdited: .keep)
                case "replace": model.updateBuiltin(edited.name, onEdited: .replace)
                case "cancel": model.builtinsModel.edited = nil
                default: return refuse("400 Bad Request", "choice takes keep, replace or cancel")
                }
            default: return refuse("400 Bad Request", "action takes ls, install, update, remove or edited")
            }
        case ("POST", "/confirm"):
            // The confirmation sheet's buttons: answer=yes (the destructive one, or Return) or no (Cancel, Esc).
            guard model.confirmation != nil else { return refuse("409 Conflict", "no confirmation is showing") }
            model.answerConfirmation(query["answer"] == "yes")
        case ("POST", "/app/quit"):
            // Quit the way ⌘Q does; the app may be gone before this reply reaches the caller.
            Log.info("debug: quitting")
            DispatchQueue.main.async { NSApp.terminate(nil) }
        case ("GET", "/devices"):
            // What the Devices tab's pairing code encodes, for the address it shows.
            let devices = model.settings.devices
            guard let json = devices.pairingJSON else {
                return refuse("409 Conflict", devices.unavailable)
            }
            return ("200 OK", Data(json.utf8))
        case ("POST", "/devices"):
            // `action=pair` asks the daemon for a fresh one-time token (what the Devices tab's
            // button does); `action=revoke&id=` forgets a device.
            switch query["action"] {
            case "pair": model.requestPairingCode()
            case "revoke":
                guard let id = query["id"] else { return refuse("400 Bad Request", "id is required") }
                model.revoke(device: id)
            default: return refuse("400 Bad Request", "action takes pair or revoke")
            }
        case ("GET", _):
            break
        default:
            return refuse("404 Not Found", "unknown route")
        }
        // `quiet=1` answers without the state, so a check that times a run of requests measures the
        // app rather than the state's own build, which lays out every transcript.
        if query["quiet"] == "1" { return ("200 OK", Data("{}".utf8)) }
        let body = (try? JSONSerialization.data(withJSONObject: state(), options: [.prettyPrinted, .sortedKeys])) ?? Data()
        return ("200 OK", body)
    }

    /// Select text across rows the way a drag does, and copy it the way ⌘C does. The ends are
    /// character offsets into the rows' plain text; with `mouse` set and both rows on screen, the
    /// press, drag and release go through the transcript's own mouse handlers at the window points
    /// of those offsets, otherwise the selection is set on the model, which reaches rows scrolled
    /// out of view.
    private func selectAndCopy(_ chat: AgentChat, from: (row: Int, offset: Int), to: (row: Int, offset: Int), mouse: Bool, clicks: Int, extend: [String]) -> (String, Data) {
        func fail(_ status: String, _ why: String) -> (String, Data) {
            Log.info("debug request refused: \(why)")
            return (status, Data(#"{"error":"\#(why)"}"#.utf8))
        }
        guard let window = NSApp.wheelWindow, chat.frameInWindow != .zero else {
            return fail("404 Not Found", "that transcript is not on screen")
        }
        func text(_ id: Int) -> RenderedProse? {
            chat.items.first { $0.id == id }.map { chat.selection.drawnProse(of: $0, in: chat) }.flatMap { $0.length == 0 ? nil : $0 }
        }
        guard let startText = text(from.row) else { return fail("404 Not Found", "row \(from.row) has no text in this transcript") }
        guard let endText = text(to.row) else { return fail("404 Not Found", "row \(to.row) has no text in this transcript") }
        let selection = chat.selection
        let start = TranscriptSelection.Point(row: from.row, offset: min(max(from.offset, 0), startText.length))
        let end = TranscriptSelection.Point(row: to.row, offset: min(max(to.offset, 0), endText.length))
        if mouse {
            guard let first = selection.targets[from.row], let last = selection.targets[to.row] else {
                return fail("404 Not Found", "rows \(from.row) and \(to.row) are not both on screen")
            }
            func point(_ row: Int, _ target: TranscriptSelection.Target, _ text: RenderedProse, _ offset: Int) -> CGPoint {
                let inside = (selection.drawn[row].flatMap { $0.isEmpty ? nil : $0 } as (any ProseLayout)?
                    ?? TextGeometry(text, style: target.style, width: target.frame.width)).point(of: offset)
                return CGPoint(x: target.frame.minX + inside.x, y: target.frame.minY + inside.y)
            }
            if clicks >= 2 {
                selection.mouseDown(at: point(from.row, first, startText, start.offset), extend: false, clicks: clicks, in: chat)
                selection.mouseUp()
            } else {
                selection.mouseDown(at: point(from.row, first, startText, start.offset), extend: false, in: chat)
                selection.mouseDragged(to: point(to.row, last, endText, end.offset), in: chat, window: window)
                selection.mouseUp()
            }
        } else {
            selection.select(from: start, to: end)
        }
        let arrows: [String: UInt16] = ["left": 123, "right": 124, "down": 125, "up": 126]
        for step in extend {
            let word = step.hasPrefix("word:")
            guard let code = arrows[step.replacingOccurrences(of: "word:", with: "")] else {
                return fail("400 Bad Request", "extend takes left, right, up or down, optionally word:left or word:right")
            }
            _ = selection.extend(keyCode: code, byWord: word, in: chat)
        }
        NSPasteboard.general.clearContents()
        selection.copy(in: chat)
        let copied = NSPasteboard.general.string(forType: .string) ?? ""
        let selected = selection.rows(in: chat).map(\.id)
        Log.info("debug: select \(chat.name) \(from) -> \(to)\(mouse ? " by mouse" : ""): rows \(selected), copied \(String(reflecting: copied))")
        let answer: [String: Any] = ["selected_rows": selected, "row_highlighted": !selected.isEmpty, "pasteboard": copied,
                                    "anchor": selection.anchor.map { ["row": $0.row, "offset": $0.offset] as Any } ?? NSNull(),
                                    "head": selection.head.map { ["row": $0.row, "offset": $0.offset] as Any } ?? NSNull()]
        return ("200 OK", (try? JSONSerialization.data(withJSONObject: answer, options: [.prettyPrinted, .sortedKeys])) ?? Data())
    }

    /// Real pointer and keyboard input (wheel-h3z5.31): CGEvents posted at the window server, so the
    /// application receives them as it would a person's hand, and the cursor read back is the one
    /// AppKit chose. Refuses unless this app is the active app, so no event reaches another app.
    /// `POST /input/mouse?action=move|down|drag|up|click&x=&y=[&clicks=][&mods=][&button=left|right][&window=]`
    /// (window points, top-left origin, as `/window/screen` takes them), `POST /input/key?action=press|down|up`
    /// with `text=` (one letter, digit or space) or `code=` (a virtual key code, for any other key) and `mods=cmd,shift,opt,ctrl`, `GET /input/cursor`.
    private func realInput(_ path: String, _ query: [String: String]) -> (String, Data) {
        func refuse(_ status: String, _ why: String) -> (String, Data) {
            Log.info("debug request refused: \(why)")
            return (status, Data(#"{"error":"\#(why)"}"#.utf8))
        }
        func json(_ body: [String: Any]) -> (String, Data) {
            ("200 OK", (try? JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])) ?? Data())
        }
        guard NSApp.isActive || ["/input/cursor", "/input/views"].contains(path) else {
            return refuse("409 Conflict", "this app is not the active app; POST /app/activate first, so no event goes to another app")
        }
        var flags: CGEventFlags = []
        for mod in (query["mods"] ?? "").split(separator: ",") {
            switch mod {
            case "cmd": flags.insert(.maskCommand)
            case "shift": flags.insert(.maskShift)
            case "opt": flags.insert(.maskAlternate)
            case "ctrl": flags.insert(.maskControl)
            default: return refuse("400 Bad Request", "mods are cmd, shift, opt, ctrl")
            }
        }
        // With Accessibility the event enters at the HID tap, where a person's hand does; without it
        // the event goes to this process through the window server's per-process queue.
        let hid = CGPreflightPostEventAccess()
        func post(_ event: CGEvent) {
            event.flags = flags
            if hid { event.post(tap: .cghidEventTap) } else { event.postToPid(getpid()) }
        }
        switch path {
        case "/input/views":
            // Every visible view whose class name holds `class`, as a window-point frame (top-left origin,
            // the space `/input/mouse` takes), so a check aims at a view without guessing its place.
            guard let match = query["class"], !match.isEmpty else { return refuse("400 Bad Request", "class is required") }
            guard let window = inputWindow(query["window"]), let root = window.contentView?.superview ?? window.contentView else {
                return refuse("404 Not Found", "no such window")
            }
            var found: [[String: Any]] = []
            func walk(_ view: NSView) {
                guard !view.isHiddenOrHasHiddenAncestor else { return }
                let name = String(describing: Swift.type(of: view))
                if name.contains(match) {
                    let box = view.convert(view.bounds, to: nil)
                    found.append(["class": name, "x": box.minX, "y": window.swiftUIHeight - box.maxY, "w": box.width, "h": box.height,
                                  "first_responder": window.firstResponder === view])
                }
                view.subviews.forEach(walk)
            }
            walk(root)
            return json(["views": found])
        case "/input/cursor":
            let cursor = NSCursor.current
            let known: [(String, NSCursor)] = [
                ("arrow", .arrow), ("iBeam", .iBeam), ("pointingHand", .pointingHand), ("crosshair", .crosshair),
                ("openHand", .openHand), ("closedHand", .closedHand), ("resizeLeftRight", .resizeLeftRight),
                ("resizeUpDown", .resizeUpDown), ("operationNotAllowed", .operationNotAllowed),
                ("contextualMenu", .contextualMenu), ("dragCopy", .dragCopy), ("dragLink", .dragLink)]
            let name = known.first { $0.1 == cursor || $0.1.image.tiffRepresentation == cursor.image.tiffRepresentation }?.0 ?? "other"
            let mouse = NSEvent.mouseLocation
            return json(["cursor": name, "hot_spot": [cursor.hotSpot.x, cursor.hotSpot.y], "image_size": [cursor.image.size.width, cursor.image.size.height],
                         "mouse": [mouse.x, NSScreen.screens.first.map { $0.frame.height - mouse.y } ?? mouse.y], "app_active": NSApp.isActive, "tap": hid ? "hid" : "pid"])
        case "/input/mouse":
            guard let x = query["x"].flatMap(Double.init), let y = query["y"].flatMap(Double.init) else {
                return refuse("400 Bad Request", "x and y are required")
            }
            guard let window = inputWindow(query["window"]), window.swiftUIHeight > 0, let main = NSScreen.screens.first else {
                return refuse("404 Not Found", "no such window")
            }
            let onScreen = window.convertPoint(toScreen: NSPoint(x: x, y: window.swiftUIHeight - y))
            let point = CGPoint(x: onScreen.x, y: main.frame.height - onScreen.y)
            let right = query["button"] == "right"
            let button: CGMouseButton = right ? .right : .left
            let clicks = max(query["clicks"].flatMap(Int64.init) ?? 1, 1)
            func mouse(_ type: CGEventType, count: Int64 = 1) -> Bool {
                guard let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: button) else { return false }
                event.setIntegerValueField(.mouseEventClickState, value: count)
                // Name the window under the pointer, as the window server does for a hand, so the event
                // is routed to that window when it is posted to this process.
                event.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: Int64(window.windowNumber))
                event.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: Int64(window.windowNumber))
                post(event)
                Self.spin(0.06)
                return true
            }
            let down: CGEventType = right ? .rightMouseDown : .leftMouseDown
            let up: CGEventType = right ? .rightMouseUp : .leftMouseUp
            let drag: CGEventType = right ? .rightMouseDragged : .leftMouseDragged
            let sent: Bool
            switch query["action"] ?? "" {
            case "move":
                // Warp first so the window server's pointer is there, then post the move.
                CGWarpMouseCursorPosition(point)
                sent = mouse(.mouseMoved)
            case "down": sent = mouse(down, count: clicks)
            case "drag": sent = mouse(drag, count: clicks)
            case "up": sent = mouse(up, count: clicks)
            case "click":
                var all = true
                for count in 1...clicks { all = all && mouse(down, count: count) && mouse(up, count: count) }
                sent = all
            default: return refuse("400 Bad Request", "action is move, down, drag, up or click")
            }
            guard sent else { return refuse("500 Internal Server Error", "could not make a mouse event") }
            Self.spin(0.2)
            Log.info("debug: real mouse \(query["action"] ?? "") at window (\(Int(x)),\(Int(y))) screen (\(Int(point.x)),\(Int(point.y))) clicks \(clicks) tap \(hid ? "hid" : "pid")")
            return json(["screen": [point.x, point.y], "tap": hid ? "hid" : "pid", "key_window": window.isKeyWindow, "app_active": NSApp.isActive])
        default:
            let text = query["text"]
            guard text?.isEmpty == false || query["code"] != nil else { return refuse("400 Bad Request", "text or code is required") }
            // `text` is one key by name (a letter, a digit or a space); any other key goes by `code`, so a
            // key with no name here is refused and never sent as the A key.
            let code: UInt16
            if let given = query["code"] {
                guard let parsed = UInt16(given) else { return refuse("400 Bad Request", "code is a virtual key code number") }
                code = parsed
            } else if let text, text.count == 1, let known = Self.keyCodes[text.lowercased()] {
                code = known
            } else {
                return refuse("400 Bad Request", "text names one letter, digit or space; give code for any other key")
            }
            let action = query["action"] ?? "press"
            guard ["press", "down", "up"].contains(action) else { return refuse("400 Bad Request", "action is press, down or up") }
            let key = NSApp.keyWindow
            for isDown in (action == "press" ? [true, false] : [action == "down"]) {
                guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: isDown) else {
                    return refuse("500 Internal Server Error", "could not make a key event")
                }
                if let text, !text.isEmpty, flags.isDisjoint(with: [.maskCommand, .maskControl]) {
                    let units = Array(text.utf16)
                    event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
                }
                post(event)
                Self.spin(0.05)
            }
            Self.spin(0.2)
            Log.info("debug: real key \(action) \(text ?? "code \(code)") mods \(query["mods"] ?? "") to \(key?.title ?? "no key window") tap \(hid ? "hid" : "pid")")
            let responder = key?.firstResponder.map { String(describing: Swift.type(of: $0)) } ?? "none"
            return json(["key_window": key?.title ?? "", "first_responder": responder, "tap": hid ? "hid" : "pid", "app_active": NSApp.isActive])
        }
    }

    /// `settings` is the Settings window; anything else is a Wheel window by id prefix.
    private func inputWindow(_ name: String?) -> NSWindow? {
        name == "settings" ? Self.settingsWindow : wheelWindow(name)
    }

    /// Virtual key codes of a US keyboard for the keys `/input/key` types by name, so a chord such as
    /// ⌘C reaches the app as the C key and not as the key code 0.
    private static let keyCodes: [String: UInt16] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14,
        "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "9": 25, "7": 26, "8": 28, "0": 29,
        "o": 31, "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46, " ": 49]

    /// Let the main run loop work for `seconds`: the views handle an event on a later turn, so one
    /// event is given time to land before the next goes in.
    private static func spin(_ seconds: TimeInterval) {
        let end = Date().addingTimeInterval(seconds)
        while Date() < end { RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01)) }
    }

    struct Refusal: Error { let status: String; let message: String }

    /// The window and scroll view behind a transcript.
    private func transcriptScrollView(_ chat: AgentChat) -> Result<(NSWindow, NSScrollView), Refusal> {
        guard let window = NSApp.wheelWindow, chat.frameInWindow != .zero else {
            return .failure(Refusal(status: "404 Not Found", message: "that transcript is not on screen"))
        }
        guard let scroll = chat.scrollView(in: window) else {
            return .failure(Refusal(status: "404 Not Found", message: "no scroll view under that transcript"))
        }
        return .success((window, scroll))
    }

    /// A row as one line; a tool row a request gates ends with `· waiting perm <id>`.
    private func rowText(_ row: TranscriptRow, agent: String) -> String {
        var text = rowText(row.kind)
        // What the tool or thought line shows of its time: running, and for how long.
        switch row.kind {
        case .tool, .thought:
            if row.open { text += " · running" }
            if let seconds = row.elapsed(now: .now) { text += " · \(formatElapsed(seconds))" }
        default: break
        }
        guard let call = row.toolCall, let perm = model.perms.first(where: { $0.agent == agent && $0.toolCall == call }) else { return text }
        return "\(text) · waiting perm \(perm.id)"
    }

    /// The transcript as laid out, with the ids `/agent/expand` takes: `<kind> <id>` for a row drawn
    /// alone, `tools <n> @<id> [<ids>]` for a group of n calls; `open` after one the person opened.
    private func itemsText(_ chat: AgentChat) -> [String] {
        let open = { (id: Int) in chat.expanded.contains(id) ? " open" : "" }
        let agent = chat.name
        return chat.displayItems.map { item -> String in
            switch item {
            case let .live(live):
                // The running turn's line: what it says, its phase and how long the turn has run.
                let waiting = model.perms.contains { $0.agent == agent }
                let seconds = live.started.map { String(Int(max(0, Date().timeIntervalSince($0)))) } ?? "none"
                return "live \(live.id) · phase \(live.phase(waiting: waiting)) · label \"\(live.label(waiting: waiting))\" · elapsed \(seconds)"
            case let .row(row): return "\(rowText(row.row.kind).prefix { $0 != ":" && $0 != " " }) \(row.id)\(open(row.id))"
            case let .tools(group):
                return "tools \(group.rows.count) @\(group.openKey)\(open(group.openKey)) [\(group.rows.map { "\($0.id)\(open($0.id))" }.joined(separator: ","))]"
            case let .work(group):
                return "work \(group.rows.count)\(Self.totalsText(group.rows.map(\.row))) @\(group.openKey)\(open(group.openKey)) [\(group.rows.map { "\($0.id)\(open($0.id))" }.joined(separator: ","))] · label \"\(group.turnLabel(summary: model.appearance(forAgent: agent).layout.workSummary))\"\(Self.runsText(group.rows.map(\.row), open: open))"
            case let .edits(group):
                let files = Edits.files(in: group.rows.map(\.row))
                let shown = model.appearance(forAgent: agent).layout.editsShown
                let listedAll = chat.expanded.contains(group.id)
                let fold = Edits.fold(files, shown: shown, all: listedAll)
                var more = ""
                if let rest = fold.rest { more = " · more \"\(Edits.moreLabel(count: rest.count)) +\(rest.added) -\(rest.removed)\"" }
                else if listedAll && files.count > shown { more = " · show fewer" }
                let opened = chat.openDiffPaths(card: group.id, in: files)
                return "edits \(files.count) files · listed \(fold.listed.map { "\($0.path) +\($0.added) -\($0.removed)" }.joined(separator: ", "))\(more) · open [\(opened.joined(separator: ","))] · collapse_all \(opened.isEmpty ? "hidden" : "shown") @\(group.id)\(open(group.id))"
            }
        }
    }

    /// ` · folds [Searched files 2 @-4398046511105 open [7,8], ...]` for the consecutive same-kind calls a work
    /// section folds, else nothing.
    private static func runsText(_ rows: [TranscriptRow], open: (Int) -> String) -> String {
        let folds = WorkRuns.entries(rows).compactMap { entry -> String? in
            guard case let .run(kind, indices) = entry else { return nil }
            let key = WorkRuns.openKey(first: rows[indices[0]].id)
            return "\(WorkRuns.label(kind: kind)) \(indices.count) @\(key)\(open(key)) [\(indices.map { "\(rows[$0].id)" }.joined(separator: ","))]"
        }
        return folds.isEmpty ? "" : " · folds [\(folds.joined(separator: ", "))]"
    }

    /// ` +a -r` when the turn's calls changed lines, else nothing.
    private static func totalsText(_ rows: [TranscriptRow]) -> String {
        let total = Edits.totals(in: rows)
        return total.added + total.removed > 0 ? " +\(total.added) -\(total.removed)" : ""
    }

    private func rowText(_ kind: TranscriptRow.Kind) -> String {
        switch kind {
        case let .user(text, queued): queued ? "user (queued): \(text)" : "user: \(text)"
        case let .agent(text): "agent: \(text)"
        case let .thought(text): "thought: \(text)"
        case let .tool(title, kind, status, content, detail):
            (["tool: \(title) (\(status))", toolInfoText(ToolRowInfo(title: title, kind: kind, status: status, content: content, detail: detail))] + content.map(contentText)).joined(separator: " · ")
        case let .plan(entries): "plan: \(entries.map(\.content).joined(separator: "; "))"
        case let .mode(mode): "mode: \(mode)"
        case let .notice(text): "notice: \(text)"
        case let .error(text): "error: \(text)"
        }
    }

    /// What a tool row draws: `icon <symbol> · label "<sentence>"`, then `+a -r` for a call with diffs and
    /// `reason "<first line>"` for a failed one.
    private func toolInfoText(_ info: ToolRowInfo) -> String {
        var parts = ["icon \(info.symbol)", "label \"\(info.label)\""]
        if let added = info.added, let removed = info.removed { parts.append("+\(added) -\(removed)") }
        if let reason = info.reason { parts.append("reason \"\(reason)\"") }
        return parts.joined(separator: " · ")
    }

    /// One tool content item with its text quoted and escaped, so a check can compare exact values.
    private func contentText(_ item: ToolContent) -> String {
        switch item {
        case let .text(text): "text \(String(reflecting: text))"
        case let .diff(path, old, new): "diff \(path): \(old.map { String(reflecting: $0) } ?? "new file") -> \(String(reflecting: new))"
        case let .other(type): "unknown \(type)"
        }
    }

    /// The status a pane, row or tab draws: what the daemon sent and whether `status.dots` lets it
    /// show a dot. NSNull when the daemon sent none.
    private func statusJSON(_ status: PaneStatus?) -> Any {
        guard let status else { return NSNull() }
        var o: [String: Any] = ["kind": status.rank.rawValue, "drawn": model.statusDots.contains(status.rank), "label": status.label]
        if case let .needsYou(waiting, reason) = status {
            o["waiting"] = waiting
            o["reason"] = reason ?? NSNull()
        }
        return o
    }

    /// The dots `SpaceDots` draws for a space: each mark's kind worst first and how many more follow; a space
    /// whose panes sent no statuses draws its own status, if shown.
    private func menuBarDots(_ space: SpaceInfo) -> [String: Any] {
        if space.paneStatuses.isEmpty {
            let own = space.status.flatMap { model.statusDots.contains($0.rank) ? $0.rank.rawValue : nil }
            return ["marks": own.map { [$0] } ?? [], "more": 0]
        }
        let agents = space.agentStatuses(shown: model.statusDots)
        return ["marks": agents.marks.map { $0.rank.rawValue }, "more": agents.more]
    }

    /// The caption a sidebar row carries beside the name; NSNull when none shows.
    private func captionJSON(_ status: PaneStatus?) -> Any {
        guard let status, model.statusDots.contains(status.rank) else { return NSNull() }
        return status.caption ?? NSNull()
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
                ["kind": "terminal", "term": term, "agent": agent.map { ["name": $0.name, "conversation": $0.conversation, "context": contextJSON($0.context, pane: id)] as Any } ?? NSNull()]
            case let .web(artifact): ["kind": "web", "scope": artifact.scope.rawValue, "name": artifact.name]
            case let .browser(url): ["kind": "browser", "url": url]
            case let .agent(agent, harness, _): ["kind": "agent", "agent": agent, "harness": harness]
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
    /// buttons, whether the chrome is narrow, the field's text while edited and any failure; for an agent pane, its agent, that
    /// agent's state, its last 20 transcript rows, its composer's draft, the error and notice
    /// under it, the request card it draws and the controls under its composer.
    private func panes() -> [String: Any] {
        var out: [String: Any] = [:]
        for space in model.spaces {
            for pane in space.allPanes {
                var o: [String: Any] = ["space": space.id, "kind": pane.kind.name, "status": statusJSON(space.paneStatuses[pane.id]),
                                        "zoomed": model.zoomedPane(in: space) == pane.id,
                                        "header_buttons": [model.popButton(for: pane.id), model.backgroundButton(), model.zoomButton(for: pane.id), AppModel.HeaderButton(id: "screenshot", title: "Copy pane image", symbol: "camera.viewfinder", enabled: true, on: false)].map { ["id": $0.id, "title": $0.title, "symbol": $0.symbol, "enabled": $0.enabled, "on": $0.on] as [String: Any] },
                                        "menu": model.headerMenu(for: pane.id, kind: pane.kind).map { ["id": $0.id, "title": $0.title, "enabled": $0.enabled] as [String: Any] }]
                if case .empty = pane.kind { o["picker"] = AgentInfo.pickable(model.agents).map(\.name) }
                if case let .terminal(term, _) = pane.kind, let terminal = model.terminals[term] {
                    o["term"] = term
                    o["attached"] = terminal.attached
                    o["visible"] = terminal.view.isSurfaceVisible
                    o["cols"] = terminal.size?.cols ?? NSNull()
                    o["rows"] = terminal.size?.rows ?? NSNull()
                    o["grid_cols"] = terminal.reported.map { $0.columns } ?? NSNull()
                    o["grid_rows"] = terminal.reported.map { $0.rows } ?? NSNull()
                    o["viewport_text"] = terminal.viewportText ?? NSNull()
                    let look = model.terminalLooks[space.id] ?? .standard
                    o["appearance"] = [
                        "font_family": look.fontFamily, "font_size": look.fontSize, "line_height": look.lineHeight,
                        "theme": look.theme.rawValue, "cursor": look.cursor.rawValue, "cursor_blink": look.cursorBlink,
                    ] as [String: Any]
                    o["ghostty_config"] = model.terminalConfig(forTerm: term) ?? NSNull()
                    o["font_points"] = terminal.view.fontSize.map { Double($0) } ?? NSNull()
                    o["look_space"] = terminal.space ?? NSNull()
                }
                if case let .terminal(_, agent) = pane.kind {
                    o["context"] = contextJSON(agent?.context, pane: pane.id)
                }
                if case let .agent(agent, harness, _) = pane.kind {
                    o["agent"] = agent
                    let chat = model.chats[agent]
                    o["harness"] = harness
                    o["state"] = chat.map { model.stateLabel($0) } ?? NSNull()
                    o["transcript"] = chat.map { chat in chat.transcript.rows.suffix(20).map { rowText($0, agent: agent) } } ?? NSNull()
                    let look = model.appearance(forAgent: agent)
                    o["appearance"] = [
                        "family": look.family.rawValue, "size": look.size, "line_height": look.lineHeight,
                        "paragraph_spacing": look.paragraphSpacing, "max_width": look.maxWidth ?? NSNull(),
                        "density": look.density.rawValue, "quote_style": look.quoteStyle.rawValue,
                        "code_theme": look.codeTheme.rawValue, "code_font": look.codeFont.rawValue,
                        "code_size": look.codeSize, "code_wrap": look.codeWrap.rawValue,
                        "code_line_numbers": look.lineNumbers, "footer_items": look.footerItems,
                    ] as [String: Any]
                    o["home"] = ["text": model.shownHome(agent: agent, harness: harness),
                                 "variable": AgentInfo.homeVariable(of: harness)] as [String: Any]
                    o["code_blocks"] = CodeBlockRegistry.shared.blocks(agent: agent).enumerated().map { index, block in
                        ["index": index, "language": block.language ?? NSNull(), "text": block.text, "text_unreadable": !block.readable, "drawn": block.drawn,
                         "copy_button": "Copy code"] as [String: Any]
                    }
                    o["images"] = ChatImageRegistry.shared.images(agent: agent).map { image in
                        ["row": image.row, "source": image.source, "path": image.path ?? NSNull(), "loaded": image.loaded,
                         "remote": image.remote, "width": image.width, "height": image.height, "bytes": image.bytes,
                         "error": image.error ?? NSNull()] as [String: Any]
                    }
                    o["image_full"] = model.links.fullImages[agent].map {
                        ["row": $0.row, "path": $0.path, "bytes": $0.data.count] as [String: Any]
                    } ?? NSNull()
                    o["rich"] = chat.map { chat in
                        ChatRichReport.report(of: chat.rows.compactMap { row -> String? in
                            guard case let .agent(text) = row.row.kind else { return nil }
                            return model.links.linked(text, look: look, agent: agent)
                        })
                    } ?? NSNull()
                    o["links"] = [
                        "linked": chat.map { chat in
                            chat.rows.compactMap { row -> [String: Any]? in
                                guard case let .agent(text) = row.row.kind else { return nil }
                                return ["row": row.id, "markdown": model.links.linked(text, look: look, agent: agent)]
                            }
                        } ?? [],
                        // The last click that opened a pane beside this chat: the action, the page or
                        // artifact it asked for, and the pane now showing that page (null until it exists).
                        "click": model.lastLinkClick[agent].map { click -> [String: Any] in
                            let opened = model.spaces.flatMap(\.allPanes).first { pane in
                                guard !click.before.contains(pane.id) else { return false }
                                switch pane.kind {
                                case let .browser(url): return click.page == url
                                case let .web(artifact): return click.artifact == artifact.name
                                default: return false
                                }
                            }
                            return ["action": click.action, "page": click.page ?? NSNull(), "artifact": click.artifact ?? NSNull(),
                                    "pane": opened?.id ?? NSNull()]
                        } ?? NSNull(),
                        "style": ChatLinkStyle.report(of: chat.map { chat in
                            chat.rows.compactMap { row -> String? in
                                guard case let .agent(text) = row.row.kind else { return nil }
                                return model.links.linked(text, look: look, agent: agent)
                            }
                        } ?? [], look: look),
                    ] as [String: Any]
                    o["transcript_items"] = chat.map { itemsText($0) } ?? NSNull()
                    // The items of the last right-click menu a message opened ("-" is a separator).
                    o["message_menu"] = chat?.lastMessageMenu ?? []
                    o["transcript_messages"] = chat.map { chat in
                        chat.rows.compactMap { row -> [String: Any]? in
                            guard case let .agent(text) = row.row.kind else { return nil }
                            return ["row": row.id, "chars": text.count, "head": String(text.prefix(60))]
                        }
                    } ?? NSNull()
                    // Every tool row as the line draws it: title, status, icon, sentence, counts, reason.
                    o["transcript_tools"] = chat.map { chat in
                        chat.rows.compactMap { row -> String? in
                            guard case .tool = row.row.kind else { return nil }
                            return rowText(row.row.kind)
                        }
                    } ?? NSNull()
                    o["composer"] = chat?.draft ?? NSNull()
                    o["reads_aloud"] = chat?.readsAloud ?? NSNull()
                    o["read_aloud"] = ["spoken": ReadAloud.shared.spokenCount, "last": ReadAloud.shared.lastSpoken, "playing": (ReadAloud.shared.playing as Any?) ?? NSNull()] as [String: Any]
                    o["composer_drag_types"] = (NSApp.wheelWindow?.firstResponder as? NSTextView)?.registeredDraggedTypes.count ?? NSNull()
                    o["attachments"] = chat.map { chat in
                        model.attachments(for: chat).chips.map { ["name": $0.name, "detail": $0.detail, "thumbnail": $0.thumbnail != nil] as [String: Any] }
                    } ?? NSNull()
                    o["queued"] = chat?.queued.map { ["turn": $0.turn, "text": $0.text] } ?? NSNull()
                    o["unanswered_turn"] = chat?.unansweredTurn.map { $0 as Any } ?? NSNull()
                    o["take_back"] = chat?.queue.takeBackAvailable ?? NSNull()
                    o["from_end"] = chat.map { Int($0.fromEnd) } ?? NSNull()
                    o["pinned"] = chat?.pinned ?? NSNull()
                    o["error"] = chat?.error ?? NSNull()
                    o["notice"] = chat?.notice ?? NSNull()
                    o["title"] = chat?.title ?? NSNull()
                    o["conversations"] = chat.map { chat -> [String: Any] in
                        var listing: [String: Any] = ["popover_open": model.conversationsPane == pane.id, "switching_to": chat.switchingTo ?? NSNull()]
                        switch chat.conversationListing {
                        case .idle: listing["state"] = "idle"
                        case .loading: listing["state"] = "loading"
                        case let .failed(message): listing["state"] = "failed"; listing["message"] = message
                        case let .loaded(all):
                            listing["state"] = "loaded"
                            listing["items"] = all.map { ["id": $0.id, "title": $0.title ?? NSNull(), "age": $0.age() ?? NSNull(), "current": $0.current] as [String: Any] }
                        }
                        listing["blocked"] = chat.isBusy || !chat.queued.isEmpty
                        return listing
                    } ?? NSNull()
                    o["context"] = contextJSON(chat?.context, pane: pane.id)
                    o["footer"] = chat?.footer.map { ["shells": Int($0.shells), "agents": Int($0.agents), "background_text": $0.backgroundText as Any, "branch": $0.branch as Any] } ?? NSNull()
                    o["request"] = model.perms.first { $0.agent == agent }?.id ?? NSNull()
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
                    o["request"] = model.perms.first { $0.agent == pane.id }?.id ?? NSNull()
                    o["url"] = web.address ?? NSNull()
                    o["last_reload_at"] = time(web.lastReloadAt)
                    o["loaded_at"] = time(web.loadedAt)
                    o["status"] = web.status ?? NSNull()
                    o["title"] = web.title
                    o["text"] = web.text ?? NSNull()
                    o["load_error"] = web.loadError ?? NSNull()
                    o["in_window"] = web.view.window != nil
                    o["view_hidden"] = web.view.isHidden
                    o["load_count"] = web.loadCount
                    o["view_size"] = [web.view.frame.width, web.view.frame.height]
                    if let a2ui = model.a2uiPanes[pane.id] {
                        let data: [String: Any] = Dictionary(uniqueKeysWithValues: ["/status", "/count", "/draft"].map { ($0, a2ui.text(at: $0) as Any? ?? NSNull()) })
                        o["a2ui"] = ["state": a2ui.stateName, "native": a2ui.isNative, "notice": a2ui.notice as Any? ?? NSNull(),
                                     "unsupported": a2ui.unsupported, "data": data] as [String: Any]
                    }
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
                    o["view_hidden"] = browser.view.isHidden
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
                ["scope": $0.info.scope.rawValue, "name": $0.info.name, "title": $0.info.title, "description": $0.info.description ?? NSNull(), "problems": $0.info.problems] as [String: Any]
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

    /// The command palette: whether it is open, what is typed, the rows it lists and the selected one.
    private func commandPaletteJSON() -> [String: Any] {
        guard let palette = model.commandPalette else {
            return ["open": false, "mode": NSNull(), "query": "", "rows": [] as [Any], "selected": NSNull()]
        }
        return [
            "open": true,
            "mode": palette.mode.rawValue,
            "verb": palette.verb?.rawValue ?? NSNull(),
            "query": palette.query,
            "rows": palette.rows.map { ["id": $0.item.id, "kind": $0.item.kind, "title": $0.item.title, "detail": $0.item.detail] },
            "selected": palette.rows.isEmpty ? NSNull() : palette.selected as Any,
        ]
    }

    nonisolated(unsafe) private static var preview: NSWindow?

    /// The menu bar item's button.
    private static func statusButton() -> NSStatusBarButton? {
        func find(_ view: NSView?) -> NSStatusBarButton? {
            guard let view else { return nil }
            if let button = view as? NSStatusBarButton { return button }
            return view.subviews.lazy.compactMap(find).first
        }
        return NSApp.windows.lazy.compactMap { find($0.contentView) }.first
    }

    /// The window the menu bar item opens.
    private static func popoverWindow() -> NSWindow? {
        NSApp.windows.first { String(describing: type(of: $0)).contains("MenuBarExtra") }
    }

    private static func windowClasses() -> String {
        NSApp.windows.map { String(describing: type(of: $0)) }.joined(separator: ",")
    }

    /// The Wheel window whose id starts with `prefix`, or the key one.
    private func wheelWindow(_ prefix: String?) -> NSWindow? {
        guard let prefix else { return NSApp.wheelWindow }
        return NSApp.windows.first { $0.identifier?.rawValue.hasPrefix(WindowManager.idPrefix + prefix) == true }
    }

    /// Each space's context menu as its tab or sidebar row draws it, or its popover row when no
    /// window shows its session.
    private func spaceMenus() -> [String: [[String: Any]]] {
        var holders: [String: String?] = [:]
        for window in model.layout.windows {
            for space in model.layout.spaces(of: window) where holders[space] == nil { holders[space] = window.id }
        }
        for row in model.windowlessSpaces where holders[row.id] == nil { holders[row.id] = .some(nil) }
        var menus: [String: [[String: Any]]] = [:]
        for (space, window) in holders {
            menus[space] = model.spaceMenu(for: space, in: window).map {
                ["id": $0.id, "title": $0.title, "enabled": $0.enabled, "destructive": $0.destructive] as [String: Any]
            }
        }
        return menus
    }

    /// The pairing row and window as the state query reports them.
    private func pairingState() -> [String: Any] {
        var out: [String: Any] = [:]
        let panel = model.pairingPanel?.panel
        out["row"] = model.pairingRowDetail
        out["disabled"] = model.pairingDisabledReason ?? NSNull()
        out["window_open"] = model.pairing.open
        out["window_visible"] = panel?.isVisible ?? false
        out["window_level_floating"] = panel.map { $0.level == .floating } ?? false
        out["window_buttons"] = panel.map { p -> [String: Bool] in
            ["close": p.standardWindowButton(.closeButton)?.isHidden == false,
             "minimize": p.standardWindowButton(.miniaturizeButton)?.isHidden == false,
             "zoom": p.standardWindowButton(.zoomButton)?.isHidden == false]
        } ?? NSNull()
        out["close_on_connect"] = model.pairing.closeOnConnect
        out["details_open"] = model.pairing.detailsOpen
        out["connected"] = model.pairing.connected ?? NSNull()
        out["status"] = model.pairing.connected.map { "\($0) connected" } ?? "Waiting for a device to scan…"
        out["events"] = model.pairing.events
        out["code"] = model.settings.devices.pairingJSON ?? NSNull()
        let board = NSPasteboard.general
        out["clipboard"] = ["string": board.string(forType: .string) ?? NSNull(), "concealed": board.types?.contains(NSPasteboard.PasteboardType("org.nspasteboard.ConcealedType")) ?? false] as [String: Any]
        return out
    }

    /// The spaces the focused window lists.
    private var focusedSpaces: [SpaceInfo] { model.layout.focusedWindow.map { model.spaces(in: $0.id) } ?? [] }

    private func state() -> [String: Any] {
        let connection: String = switch model.connection {
        case .connected: "connected"
        case .connecting: "connecting"
        case let .unreachable(reason): "unreachable: \(reason)"
        }
        return [
            "daemon_url": model.endpoint.url.absoluteString,
            "daemon_service": model.service.map {
                ["label": $0.label, "instance": $0.instance, "listen": $0.listen, "status": $0.status,
                 "bundled_source": $0.bundledSource ?? NSNull(),
                 "running_source": (model.serverHost?.source).map { $0 as Any } ?? NSNull(),
                 "update": model.serverRestartWaiting] as [String: Any]
            } ?? NSNull(),
            "connection": connection,
            "server_protocol": [
                "version": model.serverProtocol,
                "client": ServerProtocol.current,
            ] as [String: Any],
            "spaces": model.spaces.map { space in
                ["id": space.id, "name": space.name, "cwd": space.cwd, "focus": space.focus, "root": node(space.root),
                 "status": statusJSON(space.status),
                 "popped": space.popped.map { tree in
                     ["panes": tree.panes.map(\.id),
                      "frame": tree.panes.lazy.compactMap { self.model.poppedFrames[$0.id] }.first
                          .map { [$0.minX, $0.minY, $0.width, $0.height] as [Double] } ?? NSNull()] as [String: Any]
                 },
                 "backgrounded": space.backgrounded.flatMap(\.panes).map(\.id),
                 "tray": ["chip": !space.backgrounded.isEmpty,
                          "badge": space.backgrounded.flatMap(\.panes).contains { if case .needsYou? = self.model.paneStatus($0.id) { true } else { false } },
                          "open": self.model.trayOpen(space.id),
                          "place": self.model.trayPlace.edge.rawValue,
                          "rows": space.backgrounded.flatMap(\.panes).map { ["pane": $0.id, "icon": $0.kind.icon, "title": self.model.trayTitle($0.kind)] }] as [String: Any],
                 "stack": ["layers": space.stackLayers, "glyph": space.firstPaneKind.icon] as [String: Any]] as [String: Any]
            },
            "header_layout": model.headerLayout.choices.mapValues { $0.mapValues(\.rawValue) },
            "status_dots": StatusRank.allCases.filter(model.statusDots.contains).map(\.rawValue),
            "active": model.active ?? NSNull(),
            "last_switch": model.lastSwitch.map {
                ["space": $0.space, "press_to_windows_ms": $0.windowsMs, "press_to_frame_ms": $0.frameMs ?? NSNull(),
                 "press_to_grid_ms": $0.gridMs ?? NSNull()] as [String: Any]
            } ?? NSNull(),
            "windows": model.layout.windows.map { window in
                [
                    "id": window.id,
                    "name": window.name ?? NSNull(),
                    "title": model.title(of: window),
                    "session": window.session ?? NSNull(),
                    "spaces": model.layout.spaces(of: window),
                    "active": window.active ?? NSNull(),
                    "sidebar": window.sidebar,
                    "mode": window.mode,
                    "pinned": window.pinned,
                    "level": (model.windowHost as? WindowManager)?.window(window.id)?.level.rawValue ?? NSNull(),
                    // False: only the title row's empty ground moves a main window.
                    "movable": (model.windowHost as? WindowManager)?.window(window.id)?.isMovable ?? NSNull(),
                    "focused": window.id == model.layout.focusedWindow?.id,
                    "open": (model.windowHost as? WindowManager)?.window(window.id) != nil,
                ] as [String: Any]
            },
            "sessions": model.layout.sessions.map { ["id": $0.id, "name": $0.name, "spaces": $0.spaces] as [String: Any] },
            "background": model.layout.background,
            "background_sessions": model.layout.backgroundSessions.map(\.id),
            // Each windowed space's context menu, as its tab and sidebar row draw it.
            "little_windows": model.layout.littles.map { little in
                let nswindow = (model.windowHost as? WindowManager)?.littleWindow(little.id)
                return ["id": little.id, "space": little.space, "owner": little.owner ?? NSNull(), "open": nswindow != nil,
                        "pinned": little.pinned, "level": nswindow?.level.rawValue ?? NSNull(),
                        "frame": nswindow.map { [$0.frame.minX, $0.frame.minY, $0.frame.width, $0.frame.height] } ?? NSNull()] as [String: Any]
            },
            "space_menus": spaceMenus(),
            "space_drag": model.spaceDrag.map { ["space": $0.space, "hint": model.spaceDragHint ?? NSNull(), "insertion": model.spaceDragInsertion.map { ["window": $0.window, "index": $0.index] as [String: Any] } ?? NSNull(), "preview_card": model.desktopOverlay?.isShown ?? false, "chip": model.dragChip?.text ?? NSNull()] as [String: Any] } ?? NSNull(),
            "renaming": ["space": model.renamingSpace ?? NSNull(), "window": model.renamingWindow ?? NSNull(), "errors": model.spaceRenameError] as [String: Any],
            "pairing": pairingState(),
            "server": [
                "status": model.serverStatus.word,
                "stopped_by_menu": model.serverStoppedByMenu,
                "expanded": model.serverExpanded,
                "confirming": model.serverConfirming,
                "restart_waiting": model.serverRestartWaiting,
                "banner": model.serverBannerShown,
                "disk_access": [
                    "screen": model.diskAccessScreenShown,
                    "server": model.serverDiskAccess.map { $0 as Any } ?? NSNull(),
                    "server_path": model.serverHost?.exe ?? NSNull(),
                ] as [String: Any],
                "details": model.serverDetails,
                "control": model.serverControl?.unavailable ?? "available",
                "problem": model.serverProblem ?? NSNull(),
            ] as [String: Any],
            "menu_bar": [
                // The number on the status item: spaces running with no window.
                "badge": model.windowlessSpaces.count,
                "approvals": model.unseenApprovals,
                "hover": model.popoverHover ?? NSNull(),
                // The hover preview beside the popover: the space, the side of the popover it is on, and what it draws.
                "preview": model.previewSpace.map { space in
                    [
                        "space": space,
                        "visible": model.previewPanel?.isShown ?? false,
                        "side": model.previewPanel?.side ?? "none",
                        "frame": model.previewPanel.map { [$0.frame.minX, $0.frame.minY, $0.frame.width, $0.frame.height] } ?? [],
                        "panes": model.previewPanes(of: space).map {
                            ["pane": $0.pane, "kind": $0.kind, "status": $0.status ?? NSNull(), "last": $0.last ?? NSNull()] as [String: Any]
                        },
                        "processes": (model.procs[space] ?? []).map(\.name),
                    ] as [String: Any]
                } ?? NSNull(),
                "dock_badge": NSApp.dockTile.badgeLabel ?? NSNull(),
                "status": model.serverStatus.word,
                "pairing": model.pairingAddressText,
                "open": Self.popoverWindow()?.isVisible ?? false,
                "waiting": model.waitingApprovals,
                "windows": model.layout.windows.map { window in
                    ["id": window.id, "number": model.windowNumber(window.id) ?? 0, "title": model.title(of: window), "spaces": model.spaces(in: window.id).map(\.name)] as [String: Any]
                },
                // One card per session: its windows' numbers (none when no window shows it) and its spaces.
                "sessions": model.sessions.map { session in
                    ["id": session.id, "name": session.name,
                     "windows": model.windows(showing: session.id).map { model.windowNumber($0.id) ?? 0 },
                     "spaces": model.spaces(inSession: session.id).map(\.name)] as [String: Any]
                },
                // Every popover row: its session, whether it is dimmed or marked, and its place buttons.
                "rows": model.sessions.flatMap { session in model.spaces(inSession: session.id).map { ($0, session.id) } }.map { space, session in
                    let ownerlessLittle = model.layout.little(showing: space.id).map { $0.owner == nil } ?? false
                    return [
                        "space": space.id, "name": space.name, "group": session,
                        "dimmed": model.isDimmed(space: space.id) && !ownerlessLittle,
                        "little_mark": ownerlessLittle,
                        "hovered": model.popoverHover == space.id,
                        // The agent dots the row draws: each mark's kind, worst first, and how many more follow.
                        "dots": menuBarDots(space),
                        "buttons": model.placeButtons(for: space.id).map {
                            ["id": $0.id, "help": $0.help, "lit": $0.lit] as [String: Any]
                        },
                    ] as [String: Any]
                },
            ] as [String: Any],
            "restore_windows": model.restoresWindows,
            "sidebar": [
                "open": model.sidebarOpen,
                // What each side draws: the rows while open, the tab names while closed.
                "rows": model.sidebarOpen ? focusedSpaces.map { ["space": $0.id, "name": $0.name, "active": $0.id == model.active, "status": statusJSON($0.status), "caption": captionJSON($0.status)] as [String: Any] } : [],
                "tabs": model.sidebarOpen ? [] : focusedSpaces.map { ["space": $0.id, "name": $0.name, "active": $0.id == model.active, "status": statusJSON($0.status)] as [String: Any] },
            ] as [String: Any],
            "focus_key": model.focusKey ?? NSNull(),
            // What holds the keyboard in the Wheel window: a terminal's `<term>` when one does.
            "first_responder": NSApp.wheelWindow.map(responderName) ?? NSNull(),
            "keymap": model.keymap.chords.mapValues(\.text),
            "keymap_problems": model.keymap.problems,
            "keybind": model.keybindSetting,
            // The window whose close sheet is showing, and its summary line.
            "window_close": model.closingWindow.map { id in
                ["window": id, "summary": model.layout.window(id).map { model.closeSummary(of: $0) } ?? ""] as [String: Any]
            } ?? NSNull(),
            "confirmation": model.confirmation.map {
                ["title": $0.title, "message": $0.message, "button": $0.button, "window": $0.window] as [String: Any]
            } ?? NSNull(),
            "menus": Self.menuItems(),
            "zoomed": model.activeSpace.flatMap { model.zoomedPane(in: $0) } ?? NSNull(),
            "panes": panes(),
            "agents": model.agents.map { ["name": $0.name, "command": $0.command, "home": $0.home ?? NSNull(), "found": $0.found, "path": $0.path ?? NSNull(), "why": $0.why ?? NSNull(), "install": $0.installLine ?? NSNull()] as [String: Any] },
            "layout_error": model.layoutError ?? NSNull(),
            "toasts": model.toasts.map {
                ["id": $0.id, "level": $0.level.rawValue, "text": $0.text, "actions": $0.actions.map(\.title), "reply": $0.reply != nil,
                 "pane": $0.pane ?? NSNull(), "closable": $0.closable] as [String: Any]
            },
            "client_update": ["launched": model.launchStamp?.description ?? NSNull(), "installed": model.clientInstalled?.description ?? NSNull()] as [String: Any],
            "drag_ratios": model.dragRatios,
            "pane_drag": model.paneDrag.map { drag in
                ["pane": drag.pane, "target": drag.drop?.target ?? NSNull(), "zone": drag.drop.map { $0.zone.label } ?? NSNull(), "committed": drag.committed,
                 "away": drag.away.map { Self.awayJSON($0) } ?? NSNull(), "hint": model.paneDragHint ?? NSNull(),
                 "held": model.paneHoldSpace ?? NSNull(), "chip": model.dragChip?.text ?? NSNull()] as [String: Any]
            } ?? NSNull(),
            "pane_move_menu": model.paneMoveGroupsJSON(),
            "perms": model.perms.map { perm in
                ["id": perm.id, "agent": perm.agent, "title": perm.title, "tool_call": perm.toolCall ?? NSNull(),
                 "reason": perm.reason ?? NSNull(), "decision_reason": perm.decisionReason ?? NSNull(),
                 "blocked_path": perm.blockedPath ?? NSNull(), "always_rule": perm.alwaysRule ?? NSNull(),
                 // Each button's tooltip, as the card shows it.
                 "options": perm.options.map { ["id": $0.id, "kind": $0.kind, "name": $0.name, "help": PermissionChoices.help($0, in: perm, keys: true)] }] as [String: Any]
            },
            "waiting_count": model.perms.count,
            "request_card": model.requestCardShown.map {
                ["tool_call": $0.call, "content": $0.content.map(contentText)] as [String: Any]
            } ?? NSNull(),
            "settings": settingsJSON(),
            "builtins": [
                "loaded": model.builtinsModel.loaded,
                "problem": model.builtinsModel.problem ?? NSNull(),
                "rows": model.builtinsModel.builtins.map {
                    ["name": $0.name, "shipped_version": $0.shippedVersion, "installed_version": $0.installedVersion.map { $0 as Any } ?? NSNull(),
                     "edited_files": $0.editedFiles, "action": $0.action.rawValue] as [String: Any]
                },
                "edited": model.builtinsModel.edited.map { ["name": $0.name, "files": $0.files.map { ["path": $0.path, "diff": $0.diff] }] as [String: Any] } ?? NSNull(),
            ] as [String: Any],
            "palette": commandPaletteJSON(),
            "shortcuts": [
                "open": model.shortcutsOpen,
                "groups": CommandActions.shortcutSheet(model).map { group in
                    ["name": group.name, "rows": group.rows.map { ["id": $0.id, "title": $0.title, "keys": $0.keys] }] as [String: Any]
                },
            ] as [String: Any],
            "usage": usageJSON(),
            "procs": model.procs.mapValues { list in
                list.map { ["pid": Int($0.pid), "pane": $0.pane, "name": $0.name, "command": $0.command, "cpu": $0.cpu] as [String: Any] }
            },
            "log_path": Log.path.path,
        ]
    }

    /// The window's first responder: `terminal <term>` for a pane's surface (the one whose state
    /// says it holds the keyboard), else its class.
    func responderName(_ window: NSWindow) -> String {
        guard let responder = window.firstResponder else { return "none" }
        let name = String(describing: type(of: responder))
        if name.contains("TerminalView"), let term = model.terminals.first(where: { $0.value.view.isFocused })?.key {
            return "terminal \(term)"
        }
        return name
    }

    /// A pane's landing outside its space, as the debug state reports it.
    static func awayJSON(_ away: PaneAway) -> [String: Any] {
        switch away {
        case let .beside(pane, dir, space): ["kind": "beside", "pane": pane, "dir": dir.rawValue, "space": space]
        case let .end(space): ["kind": "end", "space": space]
        case let .newSpace(window): ["kind": "new_space", "window": window]
        case .desktop: ["kind": "desktop"]
        }
    }

    /// Every main-menu item that has a key, as `Menu/Item ⇧⌘D`, and whether it is enabled: what
    /// each menu shows when it opens, which is when SwiftUI brings its items up to date.
    static func menuItems() -> [String] {
        var out: [String] = []
        func walk(_ menu: NSMenu, _ path: String) {
            menu.delegate?.menuNeedsUpdate?(menu)
            for item in menu.items {
                if let sub = item.submenu { walk(sub, path.isEmpty ? item.title : "\(path)/\(item.title)"); continue }
                guard !item.keyEquivalent.isEmpty else { continue }
                var mods = ""
                let mask = item.keyEquivalentModifierMask
                if mask.contains(.control) { mods += "⌃" }
                if mask.contains(.option) { mods += "⌥" }
                if mask.contains(.shift) { mods += "⇧" }
                if mask.contains(.command) { mods += "⌘" }
                let key = item.keyEquivalent.unicodeScalars.first.map { KeyPress.printable($0) } ?? item.keyEquivalent
                out.append("\(path)/\(item.title) \(mods)\(key)\(item.isEnabled ? "" : " (disabled)")")
            }
        }
        if let main = NSApp.mainMenu { walk(main, "") }
        return out
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
    private func topicJSON(_ topic: SettingTopic) -> [String: Any] {
        let s = model.settings
        let page = s.page(topic)
        return [
            "query": page.query,
            "layer": page.layer.rawValue,
            "toml_shown": page.showToml,
            "rows": s.topicRows(topic).map(\.key),
            "toml": s.topicToml(topic),
        ]
    }

    private func settingsJSON() -> [String: Any] {
        let s = model.settings
        return [
            "open": s.open,
            "guidance": [
                "loaded": s.guidance.loaded,
                "error": s.guidance.error ?? NSNull(),
                "brief_bytes": s.guidance.brief?.utf8.count ?? NSNull(),
                "preview_agent": s.guidance.previewAgent,
                "given": s.guidance.given,
                "segments": s.guidance.segments.map { ["id": $0.id, "text": $0.text] as [String: Any] },
                "files": s.guidance.files.map { ["agent": $0.agent ?? NSNull(), "path": $0.path, "text": $0.text] as [String: Any] },
                "opened_file": s.openedFile.map { ["path": $0.path, "how": $0.how] as [String: Any] } ?? NSNull(),
            ] as [String: Any],
            "window_title": Self.settingsWindow?.title ?? NSNull(),
            "window_frame": Self.settingsWindow.map { ["x": $0.frame.origin.x, "y": $0.frame.origin.y, "width": $0.frame.width, "height": $0.frame.height] as [String: Any] } ?? NSNull(),
            "window_resizable": Self.settingsWindow?.styleMask.contains(.resizable) ?? NSNull(),
            "window_min": Self.settingsWindow.map { ["width": $0.minSize.width, "height": $0.minSize.height] as [String: Any] } ?? NSNull(),
            "opened_file": s.openedFile.map { ["path": $0.path, "how": $0.how] as [String: Any] } ?? NSNull(),
            "files": ["global": s.file(.global) ?? NSNull(), "project": s.file(.project) ?? NSNull()] as [String: Any],
            "tab": s.tab.rawValue,
            "devices": s.devices.stateJSON(),
            "chat": topicJSON(.chat),
            "terminal": topicJSON(.terminal),
            "project": s.project ?? NSNull(),
            "space": s.space ?? NSNull(),
            "loaded": s.loaded,
            "list_error": s.listError ?? NSNull(),
            "logs": s.logShipping.map { logs in
                [
                    "url": logs.url,
                    "state": logs.state,
                    "last_ok_ms": logs.lastOkMs ?? NSNull(),
                    "waiting_bytes": logs.waitingBytes,
                    "error": logs.error ?? NSNull(),
                    "title": LogShippingStatus.title(logs),
                    "detail": LogShippingStatus.detail(logs) ?? NSNull(),
                ] as [String: Any]
            } ?? NSNull(),
            "folder_access": s.hostAccess.map { host in
                [
                    "exe": host.exe,
                    "folders": host.folders.map { ["path": $0.path, "allowed": $0.allowed, "waiting": $0.waiting, "error": $0.error ?? NSNull()] as [String: Any] },
                    "title": FolderAccessSettings.title(host),
                ] as [String: Any]
            } ?? NSNull(),
            "command_line": [
                "state": s.commandLine.state,
                "detail": s.commandLine.detail ?? NSNull(),
                "running": s.commandLine.running,
                "shell_problem": s.commandLine.shellProblem ?? NSNull(),
                "login_path": s.commandLine.loginPath.map { ["path": $0.path, "how": $0.how, "problem": $0.problem ?? NSNull()] as [String: Any] } ?? NSNull(),
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
                    "machine_only": setting.machineOnly,
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
                "label": raw ? "raw" : "percent", "bar_fraction": context.fraction, "help": context.helpText] as [String: Any]
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

/// A key event as a US keyboard sends it for a chord: its key code, and as its characters what the
/// key types with Shift alone applied, which is what a menu's key equivalent is matched against.
enum KeyPress {
    static func event(_ chord: KeyChord, window: NSWindow, type: NSEvent.EventType = .keyDown) -> NSEvent? {
        guard let key = KeyCodes.keys[chord.key] else { return nil }
        var flags: NSEvent.ModifierFlags = []
        if chord.modifiers.contains(.ctrl) { flags.insert(.control) }
        if chord.modifiers.contains(.opt) { flags.insert(.option) }
        if chord.modifiers.contains(.shift) { flags.insert(.shift) }
        if chord.modifiers.contains(.cmd) { flags.insert(.command) }
        if chord.key.hasPrefix("arrow_") { flags.formUnion([.function, .numericPad]) }
        if KeyCodes.special[chord.key] != nil, !["enter", "tab", "space", "backspace", "escape"].contains(chord.key) { flags.insert(.function) }
        let ignoring = chord.modifiers.contains(.shift) ? key.shifted : key.plain
        return NSEvent.keyEvent(
            with: type, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, characters: ignoring, charactersIgnoringModifiers: ignoring,
            isARepeat: false, keyCode: key.code)
    }

    /// A key equivalent as a menu prints it: arrows and Return as symbols, letters uppercased.
    static func printable(_ scalar: Unicode.Scalar) -> String {
        switch Int(scalar.value) {
        case NSLeftArrowFunctionKey: "←"
        case NSRightArrowFunctionKey: "→"
        case NSUpArrowFunctionKey: "↑"
        case NSDownArrowFunctionKey: "↓"
        case 0x0D, 0x03: "↩"
        default: String(Character(scalar)).uppercased()
        }
    }
}
#endif
