import AppKit
import Combine
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

/// A pane's container. Its hole is a rectangle (an overlay's) that neither draws nor hit-tests.
final class Slot: NSView {
    override var isFlipped: Bool { true }
    var hole = CGRect.zero
    override func hitTest(_ point: NSPoint) -> NSView? {
        let local = convert(point, from: superview)
        if hole.contains(local) { return nil }
        return super.hitTest(point)
    }
    func cut(_ rect: CGRect?) {
        hole = rect.map { convert($0, from: superview) } ?? .zero
        let path = CGMutablePath()
        path.addRect(bounds)
        if !hole.isEmpty { path.addRect(hole) }
        let mask = CAShapeLayer()
        mask.path = path
        mask.fillRule = .evenOdd
        layer?.mask = mask
    }
}

@MainActor final class PaneHost {
    let id: String
    let kind: String
    let slot = Slot()
    var web: WKWebView?
    var terminal: AppTerminalView?
    var session: InMemoryTerminalSession?
    var termID: String?
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
    var writes: [String] = []
    init(id: String, kind: String) { self.id = id; self.kind = kind; slot.wantsLayer = true }
    var content: NSView { (pageShown ? pageView : nil) ?? web ?? terminal! }
}

/// A frame the bridge sends the Rust side on its own initiative: the C callback `attach` was
/// given, with the bridge's window id and one JSON object.
public typealias Emit = @convention(c) (Int32, UnsafePointer<CChar>) -> Void

@MainActor final class Native: NSObject, WKNavigationDelegate {
    let root: NSView
    let window: Int32
    let emitter: Emit
    let controller = TerminalController(configSource: .generated("font-size = 14\nkeybind = clear\nbackground = #17201c\n"))
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
    var mouseNumber = 0
    var heldBeforeKey: String?
    var mouseTarget: NSView?
    var snapshotsPending = 0
    var termOutputs = 0
    var sends: [String] = []
    var monitor: Any?
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
        monitor = nil
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
            let terminal = AppTerminalView(frame: .zero)
            host.termID = term
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
                    self.send(["op": "term_resize", "id": term, "cols": viewport.columns, "rows": viewport.rows])
                }
            })
            terminal.controller = controller
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
    func place(_ value: [String: Any]) {
        guard let host = panes[value["pane"] as! String] else { return }
        host.slot.isHidden = value["hidden"] as? Bool == true
        let a = value["rect"] as! [Double]
        var frame = CGRect(x: a[0], y: a[1], width: a[2], height: a[3])
        if !root.isFlipped { frame.origin.y = root.bounds.height - frame.maxY }
        host.slot.frame = frame
        host.content.frame = host.slot.bounds
        host.pageView?.frame = host.slot.bounds
        host.web?.frame = host.slot.bounds
        host.terminal?.setSurfaceVisible(!host.slot.isHidden)
        host.slot.cut(overlay)
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
            panes.values.forEach { $0.slot.cut(overlay) }
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
            let eventType: NSEvent.EventType = kind == "down" ? .leftMouseDown : kind == "drag" ? .leftMouseDragged : .leftMouseUp
            mouseNumber += 1
            // NSWindow.sendEvent drops mouse events for an unordered window, so route as AppKit
            // does after its own step: hit test, then the view gets the NSEvent. A drag or release
            // goes to the view that took the press.
            if kind == "down" { mouseTarget = root.hitTest(root.convert(point, to: root.superview)) }
            if let event = NSEvent.mouseEvent(with: eventType, location: root.convert(point, to: nil), modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: root.window?.windowNumber ?? 0,
                context: nil, eventNumber: mouseNumber, clickCount: 1, pressure: kind == "up" ? 0 : 1), let target = mouseTarget {
                switch kind { case "down": target.mouseDown(with: event); case "drag": target.mouseDragged(with: event); default: target.mouseUp(with: event) }
            }
            return ["target": mouseTarget.map { String(describing: type(of: $0)) } ?? "nil"]
        case "hit":
            let a = value["point"] as! [Double]
            var point = NSPoint(x: a[0], y: a[1])
            if !root.isFlipped { point.y = root.bounds.height - point.y }
            let hit = root.hitTest(root.convert(point, to: root.superview))
            let owner = panes.values.first { host in hit.map { $0 === host.content || $0.isDescendant(of: host.content) } ?? false }
            return ["class": hit.map { String(describing: type(of: $0)) } ?? "nil", "pane": owner?.id ?? NSNull()]
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
            "root_focused": root.window?.firstResponder === root, "held_before_key": heldBeforeKey ?? "",
            "first_responder": root.window?.firstResponder.map { String(describing: type(of: $0)) } ?? "nil",
            "key_window": root.window?.isKeyWindow ?? false, "visible": root.window?.isVisible ?? false,
            "app_active": NSApp.isActive, "native_subviews": root.subviews.count,
            "socket": socket.map { "state \($0.state.rawValue) code \($0.closeCode.rawValue)" } ?? "none", "term_outputs": termOutputs, "sends": sends, "logs": logs]
        var panesOut: [String: Any] = [:]
        for (id, host) in panes {
            let f = host.slot.frame
            var entry: [String: Any] = ["kind": host.kind, "hidden": host.slot.isHidden,
                "frame_top": [f.minX, root.isFlipped ? f.minY : root.bounds.height - f.maxY, f.width, f.height],
                "masked": host.slot.layer?.mask != nil,
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
            else { entry["viewport"] = host.session?.readViewportText() ?? ""; entry["grids"] = host.grids; entry["writes"] = host.writes
                   entry["pixels"] = host.terminal?.layer?.contents != nil }
            panesOut[id] = entry
        }
        out["panes"] = panesOut
        return out
    }
}

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
@_cdecl("wheel_native_command") @MainActor
public func command(_ pointer: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar> {
    do {
        let value = try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as! [String: Any]
        let window = Int32(value["window"] as? Int ?? 0)
        var result: [String: Any]
        if value["op"] as? String == "detach" {
            natives.removeValue(forKey: window)?.detach()
            result = ["detached": true]
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
