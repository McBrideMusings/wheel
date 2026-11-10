import AppKit
import WebKit
import IOSurface
import QuartzCore
import GhosttyTerminal

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
    var grids: [[Int]] = []
    var writes: [String] = []
    init(id: String, kind: String) { self.id = id; self.kind = kind; slot.wantsLayer = true }
    var content: NSView { (web ?? terminal)! }
}

@MainActor final class Native: NSObject, WKNavigationDelegate {
    let root: NSView
    let controller = TerminalController(configSource: .generated("font-size = 14\nkeybind = clear\nbackground = #17201c\n"))
    var panes: [String: PaneHost] = [:]
    var socket: URLSessionWebSocketTask?
    var sendTask: Task<Void, Never>?
    var errors: [String] = []
    var overlay: CGRect?
    var mouseNumber = 0
    var mouseTarget: NSView?
    var snapshotsPending = 0

    init(root: NSView, hidden: Bool) {
        self.root = root
        super.init()
        // Test mode: no Dock icon, activation, ordering or key-window call, deaf to the pointer.
        if hidden {
            NSApp.setActivationPolicy(.prohibited)
            root.window?.ignoresMouseEvents = true
        }
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
            do { try await socket.send(.string(string)) } catch { errors.append(error.localizedDescription) }
        }
    }
    func receive() {
        guard let socket else { return }
        Task { @MainActor in
            do {
                let message = try await socket.receive()
                let data: Data
                switch message { case .string(let s): data = Data(s.utf8); case .data(let d): data = d; @unknown default: return }
                if let value = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                   value["ev"] as? String == "term_output", let term = value["id"] as? String,
                   let encoded = value["data"] as? String, let bytes = Data(base64Encoded: encoded) {
                    for host in panes.values where host.termID == term { host.session?.receive(bytes) }
                }
                receive()
            } catch { errors.append(error.localizedDescription) }
        }
    }

    // MARK: panes
    func open(_ value: [String: Any]) {
        let id = value["pane"] as! String
        let kind = value["kind"] as! String
        if let existing = panes[id], existing.kind == kind {
            if kind == "web", let url = value["url"] as? String, url != existing.url { existing.url = url; existing.web?.load(URLRequest(url: URL(string: url)!)) }
            return
        }
        close(id)
        let host = PaneHost(id: id, kind: kind)
        root.addSubview(host.slot)
        if kind == "web" {
            let web = WKWebView(frame: .zero)
            web.navigationDelegate = self
            host.web = web
            host.slot.addSubview(web)
            if let url = value["url"] as? String { host.url = url; web.load(URLRequest(url: URL(string: url)!)) }
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
    func close(_ id: String) {
        guard let host = panes.removeValue(forKey: id) else { return }
        if let term = host.termID, !panes.values.contains(where: { $0.termID == term }) { send(["op": "term_detach", "id": term]) }
        host.slot.removeFromSuperview()
    }
    func place(_ value: [String: Any]) {
        guard let host = panes[value["pane"] as! String] else { return }
        host.slot.isHidden = value["hidden"] as? Bool == true
        let a = value["rect"] as! [Double]
        var frame = CGRect(x: a[0], y: a[1], width: a[2], height: a[3])
        if !root.isFlipped { frame.origin.y = root.bounds.height - frame.maxY }
        host.slot.frame = frame
        host.content.frame = host.slot.bounds
        host.terminal?.setSurfaceVisible(!host.slot.isHidden)
        host.slot.cut(overlay)
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        panes.values.first { $0.web === webView }?.loads.append(webView.title ?? "")
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
        case "place": place(value)
        case "focus":
            let host = panes[value["pane"] as! String]
            return ["accepted": host.map { root.window?.makeFirstResponder($0.content) ?? false } ?? false]
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
        case "post_key":
            // Real NSEvents to NSWindow.sendEvent: first-responder routing and the views' own key
            // handling run as for hardware events; nothing touches the OS keyboard or focus.
            let text = value["text"] as! String
            let code = UInt16(value["code"] as? Int ?? 0)
            for eventType in [NSEvent.EventType.keyDown, .keyUp] {
                if let event = NSEvent.keyEvent(with: eventType, location: .zero, modifierFlags: [],
                    timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: root.window?.windowNumber ?? 0,
                    context: nil, characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: code) {
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
            }
        default: break
        }
        return status()
    }

    func status() -> [String: Any] {
        var out: [String: Any] = ["pid": ProcessInfo.processInfo.processIdentifier, "errors": errors,
            "snapshots_pending": snapshotsPending,
            "first_responder": root.window?.firstResponder.map { String(describing: type(of: $0)) } ?? "nil",
            "key_window": root.window?.isKeyWindow ?? false, "visible": root.window?.isVisible ?? false,
            "app_active": NSApp.isActive, "native_subviews": root.subviews.count]
        var panesOut: [String: Any] = [:]
        for (id, host) in panes {
            let f = host.slot.frame
            var entry: [String: Any] = ["kind": host.kind, "hidden": host.slot.isHidden,
                "frame_top": [f.minX, root.isFlipped ? f.minY : root.bounds.height - f.maxY, f.width, f.height],
                "masked": host.slot.layer?.mask != nil,
                "focused": root.window?.firstResponder.map { $0 === host.content || (($0 as? NSView)?.isDescendant(of: host.content) ?? false) } ?? false,
                "view_identity": String(describing: Unmanaged.passUnretained(host.content).toOpaque())]
            if host.kind == "web" { entry["loads"] = host.loads; entry["url"] = host.url ?? "" }
            else { entry["viewport"] = host.session?.readViewportText() ?? ""; entry["grids"] = host.grids; entry["writes"] = host.writes
                   entry["pixels"] = host.terminal?.layer?.contents != nil }
            panesOut[id] = entry
        }
        out["panes"] = panesOut
        return out
    }
}

@MainActor private var native: Native?
@_cdecl("wheel_native_attach") @MainActor
public func attach(_ pointer: UnsafeMutableRawPointer, _ hidden: Int32) { native = Native(root: Unmanaged<NSView>.fromOpaque(pointer).takeUnretainedValue(), hidden: hidden != 0) }
@_cdecl("wheel_native_command") @MainActor
public func command(_ pointer: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar> {
    do {
        let value = try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as! [String: Any]
        // GPUI may render before the bridge attaches to the window.
        let result = native?.command(value) ?? ["detached": true]
        let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
        return strdup(String(decoding: data, as: UTF8.self))!
    } catch { return strdup("{\"error\":\"JSON bridge failed\"}")! }
}
@_cdecl("wheel_native_free")
public func releaseString(_ pointer: UnsafeMutableRawPointer) { free(pointer) }
