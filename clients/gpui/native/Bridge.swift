import AppKit
import Combine
import CoreImage
import WebKit
import IOSurface
import QuartzCore
import GhosttyTerminal
import SwiftUI
import WheelKit
import WheelUI

// The native half of the GPUI client: real WKWebView and Ghostty terminal views as children
// of the GPUI window's NSView, one per pane id. Every C entrypoint is called on the main thread.
// Panes are opened, placed and closed by JSON commands; the Rust side owns what each pane is.

/// A pane's container. Its holes (an overlay's, and the cards drawn over it) neither draw nor hit-test.
final class Slot: NSView {
    override var isFlipped: Bool { true }
    var holes: [CGRect] = []
    /// How many rects the mask keeps: 1 with no hole over the view.
    var shown = 1
    override func hitTest(_ point: NSPoint) -> NSView? {
        let local = convert(point, from: superview)
        if holes.contains(where: { $0.contains(local) }) { return nil }
        return super.hitTest(point)
    }
    /// Cut `rects` (the root's coordinates) out of the slot. The mask is what is left of the
    /// bounds once every hole is taken away, so two holes that overlap stay holes.
    func cut(_ rects: [CGRect]) {
        holes = rects.map { convert($0, from: superview) }.filter { $0.intersects(bounds) }
        var left = [bounds]
        for hole in holes {
            left = left.flatMap { r -> [CGRect] in
                let h = r.intersection(hole)
                if h.isNull || h.isEmpty { return [r] }
                return [CGRect(x: r.minX, y: r.minY, width: r.width, height: h.minY - r.minY),
                        CGRect(x: r.minX, y: h.maxY, width: r.width, height: r.maxY - h.maxY),
                        CGRect(x: r.minX, y: h.minY, width: h.minX - r.minX, height: h.height),
                        CGRect(x: h.maxX, y: h.minY, width: r.maxX - h.maxX, height: h.height)].filter { $0.width > 0 && $0.height > 0 }
            }
        }
        shown = left.count
        let path = CGMutablePath()
        left.forEach { path.addRect($0) }
        let mask = CAShapeLayer()
        mask.path = path
        layer?.mask = mask
    }
}

@MainActor final class PaneHost {
    let id: String
    let kind: String
    let slot = Slot()
    /// The cards drawn over this view, as GPUI placed them (x, y, w, h, top-left origin).
    var holesTop: [[Double]] = []
    var web: WKWebView?
    var terminal: DropTerminalView?
    var session: InMemoryTerminalSession?
    var termID: String?
    /// The space a terminal was opened in, whose controller styles it.
    var space: String?
    var url: String?
    var loads: [String] = []
    /// An artifact page: the address under `WebPanes.scheme` it opens at, how many times a
    /// `pane_reload` opened it again, the HTTP status of the last document, the store its
    /// browser storage lives in, and its visible text after the last load.
    var pageURL: URL?
    var reloads = 0
    var status: Int?
    var store: UUID?
    var text: String?
    /// A web pane on an artifact whose MCP server speaks A2UI draws that page natively (ADR-0031,
    /// ADR-0038): the same `A2UIPane` and `A2UIPaneView` the Swift Mac app uses.
    var page: A2UIPane?
    /// The artifact the page belongs to, as `<scope>/<name>`, and the space directory it was opened
    /// for: its store and scheme handler are bound to both, so another of either is another view.
    var artifact: String?
    var cwd: String?
    var pageView: NSHostingView<A2UIPaneView>?
    var pageShown: Bool { page?.isNative == true }
    /// A browser pane: WheelUI's model, the one the Swift Mac app shows under its chrome. Here the
    /// GPUI view draws the chrome from the state the bridge emits on every change.
    var browser: BrowserPane?
    var browserChanges: AnyCancellable?
    var grids: [[Int]] = []
    /// The grids sent to wheeld as `term_resize`; the report waiting while the grid moves; and a
    /// count of reports, so a lagging one's fallback knows a newer one came.
    var sent: [[Int]] = []
    var heldReport: InMemoryTerminalViewport?
    var reports = 0
    /// The reports that did not fit the view when they came.
    var lagged = 0
    var writes: [String] = []
    init(id: String, kind: String) { self.id = id; self.kind = kind; slot.wantsLayer = true }
    var content: NSView { (pageShown ? pageView : nil) ?? web ?? terminal! }
}

/// A terminal that takes dropped files: each file's path, shell-escaped, pasted at the prompt as
/// Ghostty's own Mac app does (its surface view's drop handler), several separated by a space.
final class DropTerminalView: AppTerminalView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([.fileURL])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { Self.paths(sender).isEmpty ? [] : .copy }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool { drop(Self.paths(sender)) }
    static func paths(_ sender: NSDraggingInfo) -> [String] {
        let urls = sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]
        return urls?.map(\.path) ?? []
    }
    /// Paste `paths` as the shell reads them; false when there are none or no surface yet.
    @discardableResult func drop(_ paths: [String]) -> Bool {
        guard !paths.isEmpty else { return false }
        return paste(text: paths.map(Self.escaped).joined(separator: " "))
    }
    /// A backslash before each character a shell would read as syntax (Ghostty's `shellEscape` set).
    static let shellSpecial = Set("\\ ()[]{}<>\"'`!#$&;|*?\t")
    static func escaped(_ path: String) -> String {
        var out = ""
        for c in path {
            if shellSpecial.contains(c) { out.append("\\") }
            out.append(c)
        }
        return out
    }
}

/// A frame the bridge sends the Rust side on its own initiative: the C callback `attach` was
/// given, with the bridge's window id and one JSON object.
public typealias Emit = @convention(c) (Int32, UnsafePointer<CChar>) -> Void

@MainActor final class Native: NSObject, WKNavigationDelegate {
    let root: NSView
    let window: Int32
    let emitter: Emit
    /// One Ghostty controller per space, so a space's `terminal.*` settings restyle its terminals
    /// and no other space's (the Swift app's `AppModel.terminalController(for:)`); a terminal keeps
    /// the controller of the space it was opened in.
    var controllers: [String: TerminalController] = [:]
    /// How each space's terminals draw, from its `terminal.*` settings.
    var looks: [String: TerminalAppearance] = [:]
    /// What Ghostty answered to each space's last restyle: it took the configuration, the theme.
    var restyled: [String: [Bool]] = [:]
    func controller(for space: String?) -> TerminalController {
        let key = space ?? ""
        if let held = controllers[key] { return held }
        let look = looks[key] ?? .standard
        let made = TerminalStyle.controller(look)
        controllers[key] = made
        return made
    }
    /// A space's settings arrived (`terminal_look`): restyle its controller in place, so the shells
    /// keep running and every surface redraws (the Swift app's `restyleTerminals`).
    func restyle(_ value: [String: Any]) {
        guard let space = value["space"] as? String, let list = value["settings"],
              let data = try? JSONSerialization.data(withJSONObject: list),
              let settings = try? JSONDecoder().decode([SettingInfo].self, from: data) else {
            errors.append("terminal_look: unreadable settings for \(value["space"] ?? "no space")")
            return
        }
        let look = TerminalAppearance(settings: settings)
        guard looks[space] != look else { return }
        looks[space] = look
        guard let controller = controllers[space] else { return }
        let took = TerminalStyle.restyle(controller, to: look)
        restyled[space] = [took.config, took.theme]
    }
    var panes: [String: PaneHost] = [:]
    var socket: URLSessionWebSocketTask?
    /// The connection the A2UI pages talk over, started with the first page and dropped with `disconnect`.
    var pageClient: DaemonClient?
    /// Where the daemon is and the token it wants, for every artifact page's scheme handler.
    let pageDaemon = PageDaemon()
    var sendTask: Task<Void, Never>?
    var errors: [String] = []
    /// What the browser models logged, the last 200 lines, for the check's report.
    var logs: [String] = []
    func log(_ line: String) {
        logs.append(line)
        if logs.count > 200 { logs.removeFirst(logs.count - 200) }
    }
    var overlay: CGRect?
    /// The clear of the last concealed clipboard copy, until its minute runs out.
    var concealedClear: Task<Void, Never>?
    var mouseNumber = 0
    var heldBeforeKey: String?
    var mouseTarget: NSView?
    var snapshotsPending = 0
    var termOutputs = 0
    var sends: [String] = []
    var monitor: Any?
    var pressMonitor: Any?
    var motionObserver: Any?
    var liveResizeObservers: [Any] = []
    /// The chords the GPUI view runs an action for now, in wheeld's spelling (`keymap` op).
    var routed: Set<String> = []

    init(root: NSView, window: Int32, hidden: Bool, emit: @escaping Emit) {
        self.root = root
        self.window = window
        self.emitter = emit
        super.init()
        // Test mode: no Dock icon, activation, ordering or key-window call, deaf to the pointer.
        if hidden {
            NSApp.setActivationPolicy(.prohibited)
            root.window?.ignoresMouseEvents = true
        }
        // Hardware keys reach the focused native view first; the pane-cycling shortcuts are the
        // GPUI view's, so a native pane hands the window's keyboard back to it for those.
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.routeKey(event)
            return event
        }
        // A press lands on a native view before GPUI sees it, so the GPUI view hears of it here
        // and closes what a press away closes (its popovers), as a Swift popover closes on one.
        pressMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]) { [weak self] event in
            guard let self, event.window === root.window, let superview = root.superview else { return event }
            let at = superview.convert(event.locationInWindow, from: nil)
            pressed(root.hitTest(at), at: root.convert(at, from: superview))
            return event
        }
        // The GPUI view keeps the system's Reduce Motion: sent once its frames are registered, after
        // `attach` returns, and on every change.
        motionObserver = NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.emitReduceMotion() }
        }
        DispatchQueue.main.async { [weak self] in self?.emitReduceMotion() }
        // A window edge under the pointer holds the terminals' resizes until it is let go, as
        // AppModel counts `windowResizes` from these notifications. With no queue the block runs
        // as AppKit posts, before the resize's first frame is laid out.
        for (name, on) in [(NSWindow.willStartLiveResizeNotification, true), (NSWindow.didEndLiveResizeNotification, false)] {
            liveResizeObservers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: nil) { [weak self] note in
                MainActor.assumeIsolated {
                    guard let self, let window = self.root.window, note.object as? NSWindow === window else { return }
                    self.liveResizing = on
                    if !self.holding { self.sendHeld() }
                }
            })
        }
    }
    func emitReduceMotion() { emit(["event": "reduce_motion", "on": reduceMotion()]) }
    /// The pane whose native view `view` is or lies inside.
    func owner(of view: NSView?) -> PaneHost? {
        guard let view else { return nil }
        return panes.values.first { view === $0.content || view.isDescendant(of: $0.content) }
    }
    /// A press reached `hit` at `point` (the root's coordinates): when that is a pane's native
    /// view, say so with the point from the window's top left.
    func pressed(_ hit: NSView?, at point: NSPoint) {
        guard let host = owner(of: hit) else { return }
        let y = root.isFlipped ? point.y : root.bounds.height - point.y
        emit(["event": "pressed", "pane": host.id, "point": [Double(point.x), Double(y)]])
    }
    /// Send one JSON object to the Rust side: a browser pane's state or report, or the answer to a
    /// daemon request the Rust side relayed.
    func emit(_ value: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: value), let string = String(data: data, encoding: .utf8) else { return }
        string.withCString { emitter(window, $0) }
    }

    /// A chord the GPUI view's keymap runs now belongs to it, wherever the keyboard is, as the
    /// Swift app's key monitor runs its keymap before the pane sees the key: make the GPUI view the
    /// first responder before the key lands.
    func routeKey(_ event: NSEvent) {
        guard event.window === root.window else { return }
        // The pane that held the keyboard is named for this key only.
        heldBeforeKey = nil
        guard root.window?.firstResponder !== root, let chord = KeyChord(event: event), routed.contains(chord.text) else { return }
        // The pane that held the keyboard, so the GPUI view knows where cycling starts.
        let responder = root.window?.firstResponder as? NSView
        heldBeforeKey = panes.first { responder === $0.value.content || responder?.isDescendant(of: $0.value.content) == true }?.key
        _ = root.window?.makeFirstResponder(root)
    }

    /// The window is closing: drop every view, the socket and the key monitor.
    func detach() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        if let pressMonitor { NSEvent.removeMonitor(pressMonitor) }
        if let motionObserver { NSWorkspace.shared.notificationCenter.removeObserver(motionObserver) }
        liveResizeObservers.forEach { NotificationCenter.default.removeObserver($0) }
        motionObserver = nil
        monitor = nil
        pressMonitor = nil
        for id in Array(panes.keys) { close(id) }
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
        pageClient?.stop()
        pageClient = nil
    }

    // MARK: daemon socket (terminal bytes only; layout and ops go over Rust's own socket)
    func connect(_ url: String) {
        guard socket == nil, let target = URL(string: url) else { return }
        socket = URLSession.shared.webSocketTask(with: target)
        socket?.resume()
        receive()
    }
    func send(_ value: [String: Any]) {
        guard let socket, let data = try? JSONSerialization.data(withJSONObject: value), let string = String(data: data, encoding: .utf8) else { return }
        // URLSession sends from independently spawned tasks may overtake each other:
        // keep key, Return and resize ordering on the wire.
        let previous = sendTask
        sendTask = Task { @MainActor in
            await previous?.value
            sends.append((value["op"] as? String ?? "?") + " " + (value["id"] as? String ?? ""))
            do { try await socket.send(.string(string)) } catch { errors.append("send \(value["op"] as? String ?? "?"): " + error.localizedDescription) }
        }
    }
    func receive() {
        guard let socket else { return }
        let dead = socket
        Task { @MainActor in
            do {
                let message = try await socket.receive()
                let data: Data
                switch message { case .string(let s): data = Data(s.utf8); case .data(let d): data = d; @unknown default: return }
                if let value = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                   value["ev"] as? String == "term_output", let term = value["id"] as? String,
                   let encoded = value["data"] as? String, let bytes = Data(base64Encoded: encoded) {
                    termOutputs += 1
                    for host in panes.values where host.termID == term { host.session?.receive(bytes) }
                }
                receive()
            } catch {
                errors.append(error.localizedDescription)
                if self.socket === dead { self.socket = nil }
            }
        }
    }

    // MARK: panes
    func open(_ value: [String: Any]) {
        let id = value["pane"] as! String
        let kind = value["kind"] as! String
        let artifact = (value["artifact"] as? String).map { "\(value["scope"] as? String ?? "")/\($0)" }
        if let existing = panes[id], existing.kind == kind, existing.artifact == artifact, existing.cwd == value["cwd"] as? String {
            if kind == "web", let url = value["url"] as? String, url != existing.url { existing.url = url; existing.web?.load(URLRequest(url: URL(string: url)!)) }
            if kind == "browser", let url = value["url"] as? String { existing.browser?.daemonSays(url) }
            return
        }
        close(id)
        let host = PaneHost(id: id, kind: kind)
        host.artifact = artifact
        host.cwd = value["cwd"] as? String
        root.addSubview(host.slot)
        if kind == "browser" {
            // The model reports the pages it lands on, asks what typed text means and relays the
            // page's console through the Rust side, which owns the connection to wheeld.
            let browser = BrowserPane(pane: id, url: value["url"] as? String ?? "about:blank", daemonIsRemote: value["remote"] as? Bool == true,
                log: { [weak self] in self?.log($0) },
                report: { [weak self] in self?.emit(["event": "navigated", "pane": id, "url": $0]) },
                ask: { [weak self] in self?.emit(["event": "typed", "pane": id, "text": $0]) },
                console: { [weak self] reset, entries in
                    let list = entries.map { e -> [String: Any] in
                        var o: [String: Any] = ["time": e.time, "level": e.level, "text": e.text]
                        if let url = e.url { o["url"] = url }
                        if let line = e.line { o["line"] = line }
                        return o
                    }
                    self?.emit(["event": "console", "pane": id, "reset": reset, "entries": list])
                })
            host.browser = browser
            host.web = browser.view
            host.url = browser.url
            host.slot.addSubview(browser.view)
            // Every change of the chrome's state goes to the GPUI view once the change has landed.
            host.browserChanges = browser.objectWillChange.receive(on: RunLoop.main).sink { [weak self, weak host] _ in
                guard let self, let host else { return }
                self.emit(self.browserState(host))
            }
            panes[id] = host
            emit(browserState(host))
            return
        }
        if kind == "web" {
            let config = WKWebViewConfiguration()
            let artifact = (value["artifact"] as? String).flatMap { name in (value["scope"] as? String).flatMap(ArtifactScope.init(rawValue:)).map { ArtifactRef(scope: $0, name: name) } }
            if let artifact {
                // An artifact's page, as the Swift Mac app opens it (WebPane.swift): its own browser
                // storage, one store per artifact, and its address under the client's scheme, which
                // `PageSchemeHandler` fetches from the daemon with the token in a header.
                let store = WebPanes.store(cwd: value["cwd"] as? String ?? "", artifact: artifact)
                host.store = store
                config.websiteDataStore = WKWebsiteDataStore(forIdentifier: store)
                if let ws = value["ws"] as? String { daemonAddress(ws) }
                let handler = PageSchemeHandler(pane: id, daemon: pageDaemon)
                handler.onDocument = { [weak host] code in host?.status = code }
                config.setURLSchemeHandler(handler, forURLScheme: WebPanes.scheme)
            }
            let web = WKWebView(frame: .zero, configuration: config)
            web.navigationDelegate = self
            host.web = web
            host.slot.addSubview(web)
            if artifact != nil, let page = WebPanes.pageURL(pane: id) {
                host.pageURL = page
                host.url = page.absoluteString
                web.load(URLRequest(url: page))
            } else if let url = value["url"] as? String { host.url = url; web.load(URLRequest(url: URL(string: url)!)) }
            if let artifact, let client = pages(value["ws"] as! String) {
                let (name, scope) = (artifact.name, artifact.scope)
                let page = A2UIPane(pane: id, artifact: ArtifactRef(scope: scope, name: name), space: value["space"] as? String,
                                    send: { [weak client] in client?.send($0) }, nextReq: { [weak client] in client?.req() ?? 0 })
                let view = NSHostingView(rootView: A2UIPaneView(pane: page))
                view.isHidden = true
                host.page = page
                host.pageView = view
                host.slot.addSubview(view)
                if client.connection == .connected { page.start() }
            }
        } else {
            let term = value["term"] as! String
            connect(value["ws"] as! String)
            let terminal = DropTerminalView(frame: .zero)
            host.termID = term
            host.space = value["space"] as? String
            host.session = InMemoryTerminalSession(write: { [weak self, weak host] data in
                Task { @MainActor in
                    guard let self, let host else { return }
                    host.writes.append(String(decoding: data, as: UTF8.self))
                    self.send(["op": "term_input", "id": term, "data": data.base64EncodedString()])
                }
            }, resize: { [weak self, weak host] viewport in
                Task { @MainActor in
                    guard let self, let host else { return }
                    host.grids.append([Int(viewport.columns), Int(viewport.rows)])
                    self.resized(host, viewport)
                }
            })
            terminal.controller = controller(for: host.space)
            terminal.configuration = TerminalSurfaceOptions(backend: .inMemory(host.session!))
            host.terminal = terminal
            host.slot.addSubview(terminal)
            send(["op": "term_attach", "id": term])
        }
        panes[id] = host
    }
    /// What the GPUI view draws a browser pane's chrome from.
    func browserState(_ host: PaneHost) -> [String: Any] {
        guard let b = host.browser else { return [:] }
        var o: [String: Any] = ["event": "browser", "pane": host.id, "url": b.url, "address": b.address, "title": b.title, "loading": b.loading,
            "progress": b.progress, "can_go_back": b.canGoBack, "can_go_forward": b.canGoForward]
        if let f = b.failure { o["failure"] = ["address": f.address, "message": f.message] }
        return o
    }
    /// `back`, `forward`, `reload`, `stop`, `clear_failure`, `open` (typed text), `opened` and
    /// `refused` (the daemon's answers to typed text), `daemon_says` (a `pane_url`).
    func browser(_ value: [String: Any]) {
        guard let host = panes[value["pane"] as! String], let browser = host.browser else { return }
        switch value["action"] as? String {
        case "back": browser.back()
        case "forward": browser.forward()
        case "reload": browser.reload()
        case "stop": browser.stop()
        case "clear_failure": browser.clearFailure()
        case "open": browser.open(typed: value["text"] as? String ?? "")
        case "opened": browser.opened(value["url"] as? String ?? "")
        case "refused": browser.refused(typed: value["text"] as? String ?? "", value["message"] as? String ?? "")
        case "daemon_says": browser.daemonSays(value["url"] as? String ?? "")
        default: break
        }
    }
    /// `render_pane` relayed by the Rust side: the pane as a PNG, answered through `emit` with
    /// `rendered` or `refused`. A web or browser pane is WebKit's own render of the page (under
    /// `full`, the whole document); a native A2UI page and a terminal are their views' pixels.
    func render(_ value: [String: Any]) {
        let id = value["id"] as! UInt64
        let pane = value["pane"] as! String
        guard let host = panes[pane] else { emit(["answer": "refused", "id": id, "message": RenderRefusal.notShown]); return }
        if host.pageShown, let view = host.pageView, let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
            view.cacheDisplay(in: view.bounds, to: rep)
            guard let png = rep.representation(using: .png, properties: [:]) else { emit(["answer": "refused", "id": id, "message": "the page could not be encoded as a PNG"]); return }
            emit(["answer": "rendered", "id": id, "png": png.base64EncodedString()])
        } else if let web = host.web {
            Task { @MainActor [weak self] in
                switch await BrowserInspect.shotPage(web, full: value["full"] as? Bool == true) {
                case let .success(image):
                    guard let png = BrowserInspect.png(image) else { self?.emit(["answer": "refused", "id": id, "message": "the drawing could not be encoded as a PNG"]); return }
                    self?.emit(["answer": "rendered", "id": id, "png": png.base64EncodedString()])
                case let .failure(refusal): self?.emit(["answer": "refused", "id": id, "message": refusal.message])
                }
            }
        } else if let terminal = host.terminal, let image = terminalPixels(terminal), let tiff = image.tiffRepresentation,
                  let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) {
            emit(["answer": "rendered", "id": id, "png": png.base64EncodedString()])
        } else {
            emit(["answer": "refused", "id": id, "message": "the GPUI client draws this pane itself; its picture is not served yet"])
        }
    }
    /// `inspect_pane` relayed by the Rust side, on a browser pane's page: answered through `emit`
    /// with `inspected`, `rendered`, `recorded` or `refused`.
    func inspect(_ value: [String: Any]) {
        let id = value["id"] as! UInt64
        let pane = value["pane"] as! String
        let arg = value["arg"] as? String
        guard let web = panes[pane]?.browser?.view else { emit(["answer": "refused", "id": id, "message": RenderRefusal.notShown]); return }
        Task { @MainActor [weak self] in
            switch await BrowserInspect.run(value["action"] as? String ?? "", arg: arg, in: web) {
            case let .text(text): self?.emit(["answer": "inspected", "id": id, "text": text])
            case let .png(png): self?.emit(["answer": "rendered", "id": id, "png": png.base64EncodedString()])
            case let .film(film): self?.emit(["answer": "recorded", "id": id, "mov": film.mov.base64EncodedString(), "frames": film.frames, "millis": film.millis])
            case let .refused(message): self?.emit(["answer": "refused", "id": id, "message": message])
            }
        }
    }
    /// A file in the pane's folder changed. Opens the address again rather than `WKWebView.reload()`,
    /// which does nothing when the first load never committed (the daemon was down).
    func reload(_ id: String) {
        guard let host = panes[id], let page = host.pageURL else { return }
        host.reloads += 1
        host.web?.load(URLRequest(url: page))
    }
    /// The daemon's address and token for the scheme handler, from the socket address a pane was given.
    func daemonAddress(_ ws: String) {
        guard var parts = URLComponents(string: ws), let token = parts.queryItems?.first(where: { $0.name == "token" })?.value else { return }
        parts.queryItems = nil
        pageDaemon.url = parts.url
        pageDaemon.token = token
    }
    func close(_ id: String) {
        guard let host = panes.removeValue(forKey: id) else { return }
        if let term = host.termID, !panes.values.contains(where: { $0.termID == term }) { send(["op": "term_detach", "id": term]) }
        host.slot.removeFromSuperview()
    }
    /// The page connection, opened on first use from the socket address the pane was given.
    func pages(_ ws: String) -> DaemonClient? {
        if let pageClient { return pageClient }
        guard var parts = URLComponents(string: ws), let token = parts.queryItems?.first(where: { $0.name == "token" })?.value else { return nil }
        parts.queryItems = nil
        guard let url = parts.url else { return nil }
        let client = DaemonClient(endpoint: DaemonEndpoint(url: url, token: { token }))
        client.onConnect = { [weak self] _ in self?.panes.values.forEach { $0.page?.start() } }
        client.onMessage = { [weak self] message in
            guard let self else { return }
            for host in panes.values { host.page?.handle(message) }
            showPages()
        }
        client.log = { [weak self] in self?.errors.append("pages: " + $0) }
        client.start()
        pageClient = client
        return client
    }
    /// Show each page's native view once its server has answered with an A2UI layout, its web page until then.
    func showPages() {
        for host in panes.values {
            guard let view = host.pageView else { continue }
            view.isHidden = !host.pageShown
            host.web?.isHidden = host.pageShown
        }
    }
    /// While the GPUI view moves the panes (`hold_resizes`: the grid animates or a divider is
    /// dragged) or a window edge is dragged, a terminal's resizes wait: only the grid it ends at
    /// goes to wheeld, as AppModel.settle sends a terminal's size when the move ends.
    var resizesHeld = false
    var liveResizing = false
    var holding: Bool { resizesHeld || liveResizing }
    /// A report can lag its view: the surface reflows a display frame after the view moved, so a
    /// report whose pixels are not the view's is a grid of an earlier frame and waits for the one
    /// that follows, sent anyway after `lagLimit` with no newer report (AppModel.settle's rules).
    static let lagLimit: Duration = .milliseconds(100)
    static let paddingPoints: CGFloat = 24
    func resized(_ host: PaneHost, _ vp: InMemoryTerminalViewport) {
        if holding { host.heldReport = vp; return }
        host.reports += 1
        guard fitsView(host, vp) else {
            host.lagged += 1
            sendUnlessNewer(host, vp)
            return
        }
        sendSize(host, vp)
    }
    /// Send `vp` after `lagLimit` unless a newer report came meanwhile.
    func sendUnlessNewer(_ host: PaneHost, _ vp: InMemoryTerminalViewport) {
        let report = host.reports
        Task { @MainActor [weak self, weak host] in
            try? await Task.sleep(for: Self.lagLimit)
            guard let self, let host, host.reports == report else { return }
            // A motion that began meanwhile holds it, so its end sends it when nothing newer comes.
            if self.holding { host.heldReport = host.heldReport ?? vp } else { self.sendSize(host, vp) }
        }
    }
    func sendSize(_ host: PaneHost, _ vp: InMemoryTerminalViewport) {
        let grid = [Int(vp.columns), Int(vp.rows)]
        guard host.sent.last != grid, let term = host.termID else { return }
        host.sent.append(grid)
        send(["op": "term_resize", "id": term, "cols": grid[0], "rows": grid[1]])
    }
    /// Whether report `vp` is of the terminal view's present frame: its pixels are the view's
    /// less the padding and less than a cell.
    func fitsView(_ host: PaneHost, _ vp: InMemoryTerminalViewport) -> Bool {
        guard let view = host.terminal, vp.widthPixels > 0, vp.heightPixels > 0, vp.columns > 0, vp.rows > 0 else { return true }
        let scale = view.window?.backingScaleFactor ?? 2
        let slack = Self.paddingPoints * scale
        let cw = max(CGFloat(vp.cellWidthPixels), CGFloat(vp.widthPixels) / CGFloat(vp.columns))
        let ch = max(CGFloat(vp.cellHeightPixels), CGFloat(vp.heightPixels) / CGFloat(vp.rows))
        let dw = view.bounds.width * scale - CGFloat(vp.widthPixels)
        let dh = view.bounds.height * scale - CGFloat(vp.heightPixels)
        return (0...slack + cw).contains(dw) && (0...slack + ch).contains(dh)
    }
    func holdResizes(_ value: [String: Any]) {
        resizesHeld = value["held"] as? Bool == true
        if !holding { sendHeld() }
    }
    /// The move ended: each terminal's held report goes.
    func sendHeld() {
        // The report held last is of a frame before the motion's end, a cell short of its view
        // or more: the surface reports the grid of its end frame a display frame later, and only
        // when that grid differs. So it waits for that report, and goes when none comes.
        for host in panes.values {
            if let vp = host.heldReport {
                host.heldReport = nil
                host.reports += 1
                sendUnlessNewer(host, vp)
            }
        }
    }
    func place(_ value: [String: Any]) {
        guard let host = panes[value["pane"] as! String] else { return }
        host.slot.isHidden = value["hidden"] as? Bool == true
        host.slot.alphaValue = CGFloat(value["alpha"] as? Double ?? 1)
        let a = value["rect"] as! [Double]
        var frame = CGRect(x: a[0], y: a[1], width: a[2], height: a[3])
        if !root.isFlipped { frame.origin.y = root.bounds.height - frame.maxY }
        host.holesTop = (value["holes"] as? [[Double]]) ?? []
        host.slot.frame = frame
        host.content.frame = host.slot.bounds
        host.pageView?.frame = host.slot.bounds
        host.web?.frame = host.slot.bounds
        host.terminal?.setSurfaceVisible(!host.slot.isHidden)
        host.slot.cut(cuts(host))
    }
    /// What is cut out of `host`'s view: the overlay's rect and the cards over it, in the root's coordinates.
    func cuts(_ host: PaneHost) -> [CGRect] {
        let cards = host.holesTop.map { a -> CGRect in
            var rect = CGRect(x: a[0], y: a[1], width: a[2], height: a[3])
            if !root.isFlipped { rect.origin.y = root.bounds.height - rect.maxY }
            return rect
        }
        return (overlay.map { [$0] } ?? []) + cards
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard let host = panes.values.first(where: { $0.web === webView }) else { return }
        // A browser pane's view answers to its model's delegate, not this one.
        host.loads.append(webView.title ?? "")
        webView.evaluateJavaScript("document.body ? document.body.innerText : ''") { [weak host] result, _ in
            MainActor.assumeIsolated { host?.text = result as? String }
        }
    }
    /// An artifact page stays inside its pane's folder on the daemon; a link anywhere else opens in
    /// the default browser (WebPane.swift's rules). A browser pane's view navigates freely.
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let host = panes.values.first(where: { $0.web === webView }), let page = host.pageURL else { return .allow }
        guard let target = action.request.url else { return .cancel }
        if WebPanes.allowsInlineFrame(target, isMainFrame: action.targetFrame?.isMainFrame ?? true) { return .allow }
        let path = URLComponents(url: target, resolvingAgainstBaseURL: false)?.path ?? target.path
        if target.scheme == page.scheme, target.host == page.host, target.port == page.port, path.hasPrefix("/artifacts/\(host.id)/") { return .allow }
        if action.navigationType == .linkActivated { NSWorkspace.shared.open(target) }
        return .cancel
    }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: any Error) { errors.append(error.localizedDescription) }

    // MARK: pixels
    // Ghostty replaces its initial CAMetalLayer with an IOSurfaceLayer. Read the real renderer
    // surface currently attached to the view, never redraw text.
    func terminalPixels(_ terminal: AppTerminalView) -> NSImage? {
        guard let contents = terminal.layer?.contents, CFGetTypeID(contents as CFTypeRef) == IOSurfaceGetTypeID() else { return nil }
        let surface = unsafeBitCast(contents as AnyObject, to: IOSurfaceRef.self)
        guard IOSurfaceLock(surface, [], nil) == 0 else { return nil }
        defer { IOSurfaceUnlock(surface, [], nil) }
        let width = IOSurfaceGetWidth(surface), height = IOSurfaceGetHeight(surface), row = IOSurfaceGetBytesPerRow(surface)
        let data = Data(bytes: IOSurfaceGetBaseAddress(surface), count: row * height)
        guard let provider = CGDataProvider(data: data as CFData),
              let cg = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: row,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue),
                provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent) else { return nil }
        return NSImage(cgImage: cg, size: CGSize(width: width, height: height))
    }
    func png(_ image: NSImage?, _ path: String) -> Bool {
        guard let tiff = image?.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff), let png = bitmap.representation(using: .png, properties: [:]) else { return false }
        do { try png.write(to: URL(fileURLWithPath: path)); return true } catch { errors.append(error.localizedDescription); return false }
    }

    // MARK: commands
    func command(_ value: [String: Any]) -> [String: Any] {
        switch value["op"] as? String {
        case "open": open(value)
        case "close": close(value["pane"] as! String)
        case "reload": reload(value["pane"] as! String)
        case "browser": browser(value)
        case "render": render(value)
        case "inspect": inspect(value)
        case "page_set":
            // A value written into a native page's data model at `path`, as one of its controls
            // would write it (a picker's choice, a field's text), for a check that has no pointer.
            guard let pane = value["pane"] as? String, let surface = panes[pane]?.page?.surface, let path = value["path"] as? String,
                  let data = try? JSONSerialization.data(withJSONObject: value["value"] ?? NSNull(), options: .fragmentsAllowed),
                  let written = try? JSONDecoder().decode(WheelUI.AnyCodable.self, from: data) else { return ["ok": false] }
            surface.set(path, written)
        case "disconnect":
            // Sends queued behind a socket that is gone never finish; the next socket starts a new chain.
            socket?.cancel(with: .goingAway, reason: nil)
            socket = nil
            sendTask = nil
            pageClient?.stop()
            pageClient = nil
        case "place": place(value)
        case "hold_resizes": holdResizes(value)
        case "live_resize":
            // The check's stand-in for a person dragging the window's edge: AppKit's own start or
            // end notification, posted for this window.
            if let window = root.window {
                NotificationCenter.default.post(name: value["on"] as? Bool == true ? NSWindow.willStartLiveResizeNotification : NSWindow.didEndLiveResizeNotification, object: window)
            }
            return ["held": holding]
        case "reduce_motion":
            // With `set`, the check stands in for the system setting (null hands it back), and
            // every window hears the new value.
            if value.keys.contains("set") {
                reduceMotionOverride = value["set"] as? Bool
                natives.values.forEach { $0.emitReduceMotion() }
            }
            return ["reduce_motion": reduceMotion()]
        case "focus":
            // A pane with no native view is drawn by GPUI: its view takes the keyboard.
            let target = panes[value["pane"] as! String]?.content ?? root
            return ["accepted": root.window?.makeFirstResponder(target) ?? false]
        case "overlay":
            if value["open"] as? Bool == true {
                let a = value["rect"] as! [Double]
                var rect = CGRect(x: a[0], y: a[1], width: a[2], height: a[3])
                if !root.isFlipped { rect.origin.y = root.bounds.height - rect.maxY }
                overlay = rect
                _ = root.window?.makeFirstResponder(root)
            } else {
                overlay = nil
            }
            panes.values.forEach { $0.slot.cut(cuts($0)) }
        case "pty":
            if let host = panes[value["pane"] as! String], let term = host.termID {
                send(["op": "term_input", "id": term, "data": Data((value["text"] as! String).utf8).base64EncodedString()])
            }
        case "keymap":
            routed = Set((value["chords"] as? [String]) ?? [])
        case "choose_folder":
            // The system folder picker, as the Swift app's Open Folder: the path chosen comes back as a frame.
            let panel = NSOpenPanel()
            panel.canChooseDirectories = true
            panel.canChooseFiles = false
            panel.allowsMultipleSelection = false
            panel.begin { [weak self] response in
                guard response == .OK, let url = panel.url else { return }
                self?.emit(["event": "folder_chosen", "path": url.path])
            }
        case "terminal_look": restyle(value)
        case "term_drop":
            // What a drop of these files does, for the check: the drag itself needs a real pointer.
            guard let terminal = panes[value["pane"] as! String]?.terminal else { return ["dropped": false] }
            return ["dropped": terminal.drop(value["paths"] as? [String] ?? [])]
        case "post_key":
            // Real NSEvents to NSWindow.sendEvent: first-responder routing and the views' own key
            // handling run as for hardware events; nothing touches the OS keyboard or focus.
            let text = value["text"] as! String
            let code = UInt16(value["code"] as? Int ?? 0)
            var flags: NSEvent.ModifierFlags = []
            if value["command"] as? Bool == true { flags.insert(.command) }
            if value["shift"] as? Bool == true { flags.insert(.shift) }
            if value["control"] as? Bool == true { flags.insert(.control) }
            if value["option"] as? Bool == true { flags.insert(.option) }
            for eventType in [NSEvent.EventType.keyDown, .keyUp] {
                if let event = NSEvent.keyEvent(with: eventType, location: .zero, modifierFlags: flags,
                    timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: root.window?.windowNumber ?? 0,
                    context: nil, characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: code) {
                    if eventType == .keyDown { routeKey(event) }
                    root.window?.sendEvent(event)
                }
            }
        case "post_mouse":
            let kind = value["kind"] as! String
            let a = value["point"] as! [Double]
            var point = NSPoint(x: a[0], y: a[1])
            if !root.isFlipped { point.y = root.bounds.height - point.y }
            // A synthesized right press reads as button 0 to GPUI (`NSEvent.buttonNumber` is 0 for
            // every event made this way), so a context menu is asked for with `control: true` on a
            // left press, as a Mac control-click asks for one.
            let eventType: NSEvent.EventType = kind == "down" ? .leftMouseDown : kind == "drag" ? .leftMouseDragged : kind == "move" ? .mouseMoved : .leftMouseUp
            let flags: NSEvent.ModifierFlags = value["control"] as? Bool == true ? [.control] : []
            mouseNumber += 1
            // NSWindow.sendEvent drops mouse events for an unordered window, so route as AppKit
            // does after its own step: hit test, then the view gets the NSEvent. A drag or release
            // goes to the view that took the press.
            if kind == "down" || kind == "move" { mouseTarget = root.hitTest(root.convert(point, to: root.superview)) }
            if kind == "down" { pressed(mouseTarget, at: point) }
            if let event = NSEvent.mouseEvent(with: eventType, location: root.convert(point, to: nil), modifierFlags: flags,
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: root.window?.windowNumber ?? 0,
                context: nil, eventNumber: mouseNumber, clickCount: value["clicks"] as? Int ?? 1, pressure: kind == "up" ? 0 : 1), let target = mouseTarget {
                switch kind {
                case "down": target.mouseDown(with: event)
                case "drag": target.mouseDragged(with: event)
                case "move": target.mouseMoved(with: event)
                default: target.mouseUp(with: event)
                }
            }
            return ["target": mouseTarget.map { String(describing: type(of: $0)) } ?? "nil"]
        // Where the window's content is on screen, with the origin at the top left of the main
        // screen, and its place among the app's windows front to back (-1 while it is off screen):
        // what a space drag measures each window against.
        case "frame":
            guard let window = root.window else { return ["frame": NSNull(), "order": -1, "visible": false] }
            return ["frame": topLeftFrame(root), "order": NSApp.orderedWindows.firstIndex(of: window) ?? -1, "visible": window.isVisible]
        // Put the content's top left corner at `point` in the same coordinates. After this call
        // returns, as `fit_window` does: GPUI's resize callback cannot reach its app during the call.
        case "move_window":
            guard let a = value["point"] as? [Double], a.count == 2, root.window != nil else { return ["moved": false] }
            DispatchQueue.main.async { [weak root] in
                guard let root, let window = root.window else { return }
                let content = window.convertToScreen(root.convert(root.bounds, to: nil))
                var frame = window.frame
                let (left, top) = (frame.minX - content.minX, frame.maxY - content.maxY)
                frame.origin.x = a[0] + left
                frame.origin.y = mainScreenTop() - a[1] + top - frame.height
                window.setFrame(frame, display: true)
            }
            return ["moved": "queued"]
        case "hit":
            let a = value["point"] as! [Double]
            var point = NSPoint(x: a[0], y: a[1])
            if !root.isFlipped { point.y = root.bounds.height - point.y }
            let hit = root.hitTest(root.convert(point, to: root.superview))
            let host = owner(of: hit)
            return ["class": hit.map { String(describing: type(of: $0)) } ?? "nil", "pane": host?.id ?? NSNull()]
        case "qr":
            // The modules of a QR code for `text`, one row per line of "1"s (dark) and "0"s, with
            // CoreImage's one-module quiet zone; what the Devices page draws as cells. With
            // `copy`, the code goes on the clipboard as an image instead (the pairing window's Copy image).
            let text = value["text"] as? String ?? ""
            if value["copy"] as? Bool == true {
                let copied = PairingClipboard.copyImage(text)
                log("pairing code image \(copied ? "copied to the clipboard" : "could not be drawn")")
                return ["copied": copied]
            }
            guard let filter = CIFilter(name: "CIQRCodeGenerator") else { return ["rows": []] }
            filter.setValue(Data(text.utf8), forKey: "inputMessage")
            filter.setValue("M", forKey: "inputCorrectionLevel")
            guard let image = filter.outputImage, let cg = CIContext().createCGImage(image, from: image.extent) else { return ["rows": []] }
            let (w, h) = (cg.width, cg.height)
            var pixels = [UInt8](repeating: 0, count: w * h)
            guard let context = CGContext(data: &pixels, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w, space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue) else { return ["rows": []] }
            context.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
            // CoreGraphics draws with its origin at the bottom: the first row in memory is the last line.
            let rows: [String] = (0..<h).reversed().map { y in String((0..<w).map { pixels[y * w + $0] < 128 ? "1" : "0" }) }
            return ["rows": rows]
        case "clipboard_concealed":
            // The pairing window's Copy code: a concealed clipboard item, cleared after `clear_after_ms`
            // if nothing else was copied (PairingWindow.swift's writeConcealed).
            let text = value["text"] as? String ?? ""
            let ms = value["clear_after_ms"] as? Int ?? 60_000
            concealedClear?.cancel()
            concealedClear = PairingClipboard.writeConcealed(text, life: .milliseconds(ms)) { [weak self] cleared in
                self?.log(cleared ? "pairing code cleared from the clipboard after \(ms) ms" : "clipboard holds something else now; left alone")
                self?.concealedClear = nil
            }
            log("pairing code copied to the clipboard as a concealed item; clearing it in \(ms) ms")
            return ["copied": true, "clear_after_ms": ms]
        case "symbol":
            // An SF Symbol drawn to PNG pixels for the GPUI view, which has no symbol font of its own.
            return symbolPNG(value, scale: Double(root.window?.backingScaleFactor ?? 2))
        case "pin":
            // The title row's pin: the window floats over every other while it is on (`setPinned`).
            let on = value["on"] as? Bool == true
            root.window?.level = on ? .floating : .normal
            return ["pinned": root.window?.level == .floating]
        case "snapshot":
            let directory = value["directory"] as! String
            snapshotsPending = 0
            for host in panes.values where !host.slot.isHidden {
                if let web = host.web {
                    snapshotsPending += 1
                    web.takeSnapshot(with: nil) { image, error in
                        _ = self.png(image, "\(directory)/\(host.id)-web.png")
                        if let error { self.errors.append(error.localizedDescription) }
                        self.snapshotsPending -= 1
                    }
                }
                if let terminal = host.terminal { _ = png(terminalPixels(terminal), "\(directory)/\(host.id)-terminal.png") }
                if host.pageShown, let view = host.pageView, let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
                    view.cacheDisplay(in: view.bounds, to: rep)
                    if let data = rep.representation(using: .png, properties: [:]) { try? data.write(to: URL(fileURLWithPath: "\(directory)/\(host.id)-page.png")) }
                }
            }
        default: break
        }
        return status()
    }

    func status() -> [String: Any] {
        var out: [String: Any] = ["pid": ProcessInfo.processInfo.processIdentifier, "errors": errors,
            "snapshots_pending": snapshotsPending,
            "resizes_held": resizesHeld, "live_resize": liveResizing,
            "root_focused": root.window?.firstResponder === root, "held_before_key": heldBeforeKey ?? "",
            "first_responder": root.window?.firstResponder.map { String(describing: type(of: $0)) } ?? "nil",
            "key_window": root.window?.isKeyWindow ?? false, "visible": root.window?.isVisible ?? false,
            "app_active": NSApp.isActive, "native_subviews": root.subviews.count,
            "socket": socket.map { "state \($0.state.rawValue) code \($0.closeCode.rawValue)" } ?? "none", "term_outputs": termOutputs, "sends": sends, "logs": logs]
        var panesOut: [String: Any] = [:]
        for (id, host) in panes {
            let f = host.slot.frame
            var entry: [String: Any] = ["kind": host.kind, "hidden": host.slot.isHidden, "alpha": Double(host.slot.alphaValue),
                "frame_top": [f.minX, root.isFlipped ? f.minY : root.bounds.height - f.maxY, f.width, f.height],
                "masked": host.slot.layer?.mask != nil, "holes": host.holesTop, "mask_rects": host.slot.shown,
                "focused": root.window?.firstResponder.map { $0 === host.content || (($0 as? NSView)?.isDescendant(of: host.content) ?? false) } ?? false,
                "view_identity": String(describing: Unmanaged.passUnretained(host.content).toOpaque())]
            if let b = host.browser {
                entry["url"] = b.url; entry["title"] = b.title; entry["loading"] = b.loading; entry["can_go_back"] = b.canGoBack
                entry["can_go_forward"] = b.canGoForward; entry["status"] = b.status ?? NSNull(); entry["text"] = b.text ?? ""
                entry["failure"] = b.failure.map { ["address": $0.address, "message": $0.message] } ?? NSNull()
                entry["loads"] = host.loads
            } else if host.kind == "web" {
                entry["loads"] = host.loads; entry["url"] = host.url ?? ""; entry["reloads"] = host.reloads
                entry["status"] = host.status ?? NSNull(); entry["store"] = host.store?.uuidString ?? ""; entry["text"] = host.text ?? ""
                if let page = host.page {
                    entry["page"] = ["state": page.stateName, "native": host.pageShown, "notice": page.notice ?? "", "unsupported": page.unsupported,
                                     "artifact": host.artifact ?? "", "types": Set(page.surface?.components.values.map(\.type) ?? []).sorted(),
                                     "components": page.surface?.components.count ?? 0, "diagnostics": page.diagnostics]
                }
            }
            else { entry["viewport"] = host.session?.readViewportText() ?? ""; entry["grids"] = host.grids; entry["sent_grids"] = host.sent; entry["lagged_reports"] = host.lagged; entry["writes"] = host.writes
                   entry["pixels"] = host.terminal?.layer?.contents != nil
                   let space = host.space ?? ""
                   entry["space"] = host.space ?? NSNull()
                   entry["config"] = controllers[space]?.renderedConfig ?? ""
                   entry["theme"] = (looks[space] ?? .standard).theme.rawValue
                   entry["restyled"] = restyled[space] ?? NSNull()
                   entry["font_size"] = host.terminal?.fontSize.map { Double($0) } ?? NSNull() }
            panesOut[id] = entry
        }
        out["panes"] = panesOut
        return out
    }
}

/// `name` at `size` points in `color` (sRGB 0...1 and alpha), at `scale` unless the value names
/// one: the PNG's bytes as hex and its size in points. The Swift app draws the same symbols through
/// SwiftUI's Image(systemName:).
@MainActor func symbolPNG(_ value: [String: Any], scale fallback: Double) -> [String: Any] {
    let name = value["name"] as? String ?? ""
    let size = value["size"] as? Double ?? 13
    var rgba = value["color"] as? [Double] ?? []
    if rgba.count < 3 { rgba = [1, 1, 1, 1] }
    let scale = value["scale"] as? Double ?? fallback
    let color = NSColor(srgbRed: rgba[0], green: rgba[1], blue: rgba[2], alpha: rgba.count > 3 ? rgba[3] : 1)
    let weight: NSFont.Weight = switch value["weight"] as? String {
    case "medium": .medium
    case "semibold": .semibold
    case "bold": .bold
    default: .regular
    }
    let configuration = NSImage.SymbolConfiguration(pointSize: size, weight: weight).applying(NSImage.SymbolConfiguration(paletteColors: [color]))
    guard let base = NSImage(systemSymbolName: name, accessibilityDescription: nil), let image = base.withSymbolConfiguration(configuration) else {
        return ["missing": true]
    }
    let points = image.size
    let (w, h) = (Int((points.width * scale).rounded(.up)), Int((points.height * scale).rounded(.up)))
    guard w > 0, h > 0, let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8, samplesPerPixel: 4,
                                                     hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else {
        return ["missing": true]
    }
    rep.size = points
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    image.draw(in: NSRect(origin: .zero, size: points), from: .zero, operation: .sourceOver, fraction: 1)
    NSGraphicsContext.restoreGraphicsState()
    guard let data = rep.representation(using: .png, properties: [:]) else { return ["missing": true] }
    return ["png_hex": data.map { String(format: "%02x", $0) }.joined(), "width": points.width, "height": points.height]
}

/// What the app holds outside every window (window id -1): the menu bar item, which lives while
/// the app runs, with or without a window (WheelApp.swift's MenuBarExtra), and the Dock tile's badge.
@MainActor final class AppNative: NSObject {
    let emitter: Emit
    let hidden: Bool
    var item: NSStatusItem?
    var badge = 0
    var update = false
    var label = ""
    var dock: String?
    var clicks = 0
    var rightClicks = 0
    /// The right-click menu's items as the client sent them last, and their titles ("-" a separator).
    var menuItems: [[String: Any]] = []
    var menuTitles: [String] = []
    /// A question a hidden check answers itself (`answer_alert`): a hidden app shows no alert.
    var pendingAlert: [String: Any]?

    init(hidden: Bool, emit: @escaping Emit) {
        self.hidden = hidden
        self.emitter = emit
        super.init()
    }

    func emit(_ value: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: value), let string = String(data: data, encoding: .utf8) else { return }
        string.withCString { emitter(-1, $0) }
    }

    /// Make the item, or redraw it: its image counts `badge` spaces no window shows (MenuBarIcon),
    /// `label` is what VoiceOver reads, and `dock` the Dock tile's badge (none at zero). In the
    /// check's test mode the item exists but is never in the menu bar, and the Dock tile is left alone.
    func statusItem(_ value: [String: Any]) -> [String: Any] {
        let count = value["badge"] as? Int ?? 0
        let waiting = value["update"] as? Bool ?? false
        if item == nil {
            let made = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
            // The check's item saves its hidden state under its own name, never the app's.
            if hidden { made.autosaveName = "wheel-gpui-check" }
            made.isVisible = !hidden
            made.button?.target = self
            made.button?.action = #selector(pressed(_:))
            // A right press sends the action too, keeping whatever the left press sends it on.
            if let button = made.button {
                let previous = button.sendAction(on: .leftMouseUp)
                button.sendAction(on: NSEvent.EventTypeMask(rawValue: UInt64(previous)).union(.rightMouseDown))
            }
            item = made
            badge = -1
        }
        if count != badge || waiting != update {
            badge = count
            update = waiting
            item?.button?.image = MenuBarIcon.image(background: count, update: waiting)
        }
        label = value["label"] as? String ?? "Wheel"
        item?.button?.setAccessibilityLabel(label)
        let dockLabel = value["dock"] as? String
        if !hidden, dock != dockLabel { NSApp.dockTile.badgeLabel = dockLabel }
        dock = dockLabel
        return state()
    }

    /// Where the item's button is on the main screen, in points from that screen's top left: the
    /// popover opens under it.
    func frame() -> [Double] {
        guard let window = item?.button?.window, let screen = NSScreen.screens.first else { return [] }
        let f = window.frame, s = screen.frame
        return [f.minX - s.minX, s.maxY - f.maxY, f.width, f.height]
    }

    @objc func pressed(_ sender: Any?) {
        if let type = NSApp.currentEvent?.type, type == .rightMouseDown || type == .rightMouseUp {
            rightPressed()
            return
        }
        clicks += 1
        emit(["event": "status_item", "button": "left", "frame": frame()])
    }

    func rightPressed() {
        rightClicks += 1
        emit(["event": "status_item", "button": "right", "frame": frame()])
    }

    /// The right-click menu from `items` (`id`, `title`, `key`; id "-" a separator). Opened under
    /// the item after this call returns, so the menu's tracking loop never runs inside the client's
    /// call; a pick comes back as a `status_menu` event. A hidden item builds it and opens nothing.
    func statusMenu(_ value: [String: Any]) -> [String: Any] {
        menuItems = value["items"] as? [[String: Any]] ?? []
        let menu = NSMenu()
        for entry in menuItems {
            let id = entry["id"] as? String ?? ""
            if id == "-" {
                menu.addItem(.separator())
                continue
            }
            let item = NSMenuItem(title: entry["title"] as? String ?? id, action: #selector(chose(_:)), keyEquivalent: entry["key"] as? String ?? "")
            item.target = self
            item.representedObject = id
            menu.addItem(item)
        }
        menuTitles = menu.items.map { $0.isSeparatorItem ? "-" : $0.title }
        if !hidden, let statusItem = item, let button = statusItem.button {
            DispatchQueue.main.async {
                statusItem.menu = menu
                button.performClick(nil)
                statusItem.menu = nil
            }
        }
        return ["menu": menuTitles]
    }

    @objc func chose(_ sender: NSMenuItem) {
        emit(["event": "status_menu", "item": sender.representedObject as? String ?? ""])
    }

    /// Ask `message` with `buttons` in an alert, after this call returns; the answer comes back as
    /// an `alert` event with the button's index. A hidden app keeps the question for the check.
    func alert(_ value: [String: Any]) -> [String: Any] {
        if hidden {
            pendingAlert = value.filter { $0.key != "op" && $0.key != "window" }
            return ["alert": pendingAlert ?? [:]]
        }
        let id = value["id"] as? String ?? ""
        let message = value["message"] as? String ?? ""
        let informative = value["informative"] as? String ?? ""
        let buttons = value["buttons"] as? [String] ?? ["OK"]
        let activate = value["activate"] as? Bool ?? true
        DispatchQueue.main.async { [weak self] in
            let alert = NSAlert()
            alert.messageText = message
            alert.informativeText = informative
            alert.alertStyle = .warning
            for title in buttons { alert.addButton(withTitle: title) }
            if activate { NSApp.activate() }
            let index = alert.runModal().rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue
            self?.emit(["event": "alert", "id": id, "button": index])
        }
        return ["alert": "shown"]
    }

    /// Size the window holding `view` to `width` by `height` points of content, keeping its top
    /// edge where it is (AppKit keeps the bottom one): the popover grows down from under the item.
    /// After this call returns, as GPUI's own resize does: the client's resize callback cannot
    /// reach its app while the client's call into the bridge holds it, and drops the size.
    func fitWindow(_ value: [String: Any]) -> [String: Any] {
        guard let raw = (value["view"] as? NSNumber)?.uintValue, let pointer = UnsafeRawPointer(bitPattern: raw) else { return ["fitted": false] }
        let view = Unmanaged<NSView>.fromOpaque(pointer).takeUnretainedValue()
        guard view.window != nil else { return ["fitted": false] }
        let width = value["width"] as? Double
        let height = value["height"] as? Double
        DispatchQueue.main.async { [weak view] in
            guard let window = view?.window else { return }
            var frame = window.frame
            let top = frame.maxY
            let content = NSRect(x: 0, y: 0, width: width ?? Double(window.contentLayoutRect.width), height: height ?? Double(window.contentLayoutRect.height))
            frame.size = window.frameRect(forContentRect: content).size
            frame.origin.y = top - frame.height
            window.setFrame(frame, display: true)
        }
        return ["fitted": "queued"]
    }

    /// Where `view` is on screen, top left origin (the popover's frame for a space drag).
    func viewFrame(_ value: [String: Any]) -> [String: Any] {
        guard let raw = (value["view"] as? NSNumber)?.uintValue, let pointer = UnsafeRawPointer(bitPattern: raw) else { return ["frame": NSNull()] }
        let view = Unmanaged<NSView>.fromOpaque(pointer).takeUnretainedValue()
        guard view.window != nil else { return ["frame": NSNull()] }
        return ["frame": topLeftFrame(view)]
    }

    /// The app's icon as a PNG, `size` points square at the screen's scale (ServerHeader's icon).
    func appIcon(_ value: [String: Any]) -> [String: Any] {
        let size = value["size"] as? Double ?? 36
        let scale = Double(NSScreen.main?.backingScaleFactor ?? 2)
        let pixels = Int((size * scale).rounded(.up))
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8, samplesPerPixel: 4,
                                         hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else { return ["missing": true] }
        rep.size = NSSize(width: size, height: size)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSApp.applicationIconImage.draw(in: NSRect(x: 0, y: 0, width: size, height: size), from: .zero, operation: .sourceOver, fraction: 1)
        NSGraphicsContext.restoreGraphicsState()
        guard let data = rep.representation(using: .png, properties: [:]) else { return ["missing": true] }
        return ["png_hex": data.map { String(format: "%02x", $0) }.joined(), "width": size, "height": size]
    }

    func state() -> [String: Any] {
        let size = item?.button?.image?.size ?? .zero
        return ["status_item": item != nil, "in_menu_bar": item?.isVisible ?? false, "badge": badge, "update": update, "label": label,
                "dock": dock ?? NSNull(), "image": [size.width, size.height], "template": item?.button?.image?.isTemplate ?? false, "clicks": clicks,
                "right_clicks": rightClicks, "menu": menuTitles, "alert": pendingAlert ?? NSNull()]
    }

    func command(_ value: [String: Any]) -> [String: Any] {
        switch value["op"] as? String {
        case "status_item": return statusItem(value)
        // The check presses the item as a click does; a hidden item has no window to click in.
        case "press_status_item":
            if value["button"] as? String == "right" { rightPressed() } else { pressed(nil) }
            return state()
        case "status_menu": return statusMenu(value)
        // The check picks an item as the menu's action does.
        case "choose_status_menu":
            emit(["event": "status_menu", "item": value["item"] as? String ?? ""])
            return state()
        case "alert": return alert(value)
        // The check answers the question a hidden app kept.
        case "answer_alert":
            guard let asked = pendingAlert else { return state() }
            pendingAlert = nil
            emit(["event": "alert", "id": asked["id"] as? String ?? "", "button": value["button"] as? Int ?? -1])
            return state()
        case "fit_window": return fitWindow(value)
        case "view_frame": return viewFrame(value)
        case "app_icon": return appIcon(value)
        case "symbol": return symbolPNG(value, scale: Double(NSScreen.main?.backingScaleFactor ?? 2))
        case "pointer_on_item":
            let on = item?.button?.window.map { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) } ?? false
            return ["pointer_on_item": on && item?.isVisible == true]
        default: return state()
        }
    }
}
/// The main screen's top edge in AppKit's screen coordinates: y grows down from it in the
/// frames the client measures (a hidden check host may have no screen; 0 keeps the sums whole).
@MainActor func mainScreenTop() -> Double { Double(NSScreen.screens.first?.frame.maxY ?? 0) }
/// `view`'s rectangle on screen as [x, y, width, height] with the origin at the main screen's top left.
@MainActor func topLeftFrame(_ view: NSView) -> [Double] {
    guard let window = view.window else { return [0, 0, 0, 0] }
    let r = window.convertToScreen(view.convert(view.bounds, to: nil))
    return [Double(r.minX), mainScreenTop() - Double(r.maxY), Double(r.width), Double(r.height)]
}
@MainActor private var app: AppNative?
/// The check's stand-in for the system's Reduce Motion (`reduce_motion` with `set`); nil reads the system's.
@MainActor private var reduceMotionOverride: Bool?
@MainActor func reduceMotion() -> Bool { reduceMotionOverride ?? NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

/// One Native per GPUI window, by the id `attach` hands back.
@MainActor private var natives: [Int32: Native] = [:]
@MainActor private var nextWindow: Int32 = 1
@_cdecl("wheel_native_attach") @MainActor
public func attach(_ pointer: UnsafeMutableRawPointer, _ hidden: Int32, _ emit: @escaping Emit) -> Int32 {
    let id = nextWindow
    nextWindow += 1
    natives[id] = Native(root: Unmanaged<NSView>.fromOpaque(pointer).takeUnretainedValue(), window: id, hidden: hidden != 0, emit: emit)
    return id
}
/// The app's own half, window id -1: the menu bar item. Its frames go to `emit` with window -1.
@_cdecl("wheel_native_app_attach") @MainActor
public func appAttach(_ hidden: Int32, _ emit: @escaping Emit) {
    if app == nil { app = AppNative(hidden: hidden != 0, emit: emit) }
}
@_cdecl("wheel_native_command") @MainActor
public func command(_ pointer: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar> {
    do {
        let value = try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as! [String: Any]
        let window = Int32(value["window"] as? Int ?? 0)
        var result: [String: Any]
        if value["op"] as? String == "detach" {
            natives.removeValue(forKey: window)?.detach()
            result = ["detached": true]
        } else if window == -1 {
            result = app?.command(value) ?? ["detached": true]
        } else {
            // GPUI may render before the bridge attaches to the window.
            result = natives[window]?.command(value) ?? ["detached": true]
        }
        let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
        return strdup(String(decoding: data, as: UTF8.self))!
    } catch { return strdup("{\"error\":\"JSON bridge failed\"}")! }
}
@_cdecl("wheel_native_free")
public func releaseString(_ pointer: UnsafeMutableRawPointer) { free(pointer) }
