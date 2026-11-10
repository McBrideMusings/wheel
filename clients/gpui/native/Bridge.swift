import AppKit
import WebKit
import IOSurface
import QuartzCore
import GhosttyTerminal

// All C entrypoints are called on GPUI's main/AppKit thread. This is an isolated
// experiment, not a replacement for Wheel's scheme handler or daemon client.
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

@MainActor final class Native: NSObject, WKNavigationDelegate {
    let root: NSView
    let web = WKWebView(frame: .zero)
    let terminal = AppTerminalView(frame: .zero)
    let slots = [Slot(), Slot()]
    let controller = TerminalController(configSource: .generated("font-size = 14\nkeybind = clear\nbackground = #17201c\n"))
    var session: InMemoryTerminalSession!
    var socket: URLSessionWebSocketTask!
    var sendTask: Task<Void, Never>?
    var savedResponder: NSResponder?
    var proxyResponder: NSResponder?
    var js: Any = NSNull()
    var errors: [String] = []
    var loads: [String] = []
    var writes: [String] = []
    var grids: [[Int]] = []
    var webSnapshot = "idle"
    var imeMarked = false
    var imeAfterCommit = false
    var overlay: CGRect?
    var mouseNumber = 0
    var mouseTarget: NSView?
    var imeLog: [String] = []
    var monitor: Any?
    var savedItems: [[NSPasteboard.PasteboardType: Data]] = []

    init(root: NSView, hidden: Bool) {
        self.root = root
        super.init()
        // Test mode: no Dock icon, activation, ordering or key-window call, deaf to the pointer.
        if hidden {
            NSApp.setActivationPolicy(.prohibited)
            root.window?.ignoresMouseEvents = true
        }
        for slot in slots { slot.wantsLayer = true; root.addSubview(slot) }
        slots[0].addSubview(web)
        web.navigationDelegate = self
        session = InMemoryTerminalSession(write: { [weak self] data in
            Task { @MainActor in
                guard let self else { return }
                self.writes.append(String(decoding: data, as: UTF8.self))
                self.send(["op":"term_input", "id":self.term, "data":data.base64EncodedString()])
            }
        }, resize: { [weak self] viewport in
            Task { @MainActor in
                guard let self else { return }
                self.grids.append([Int(viewport.columns), Int(viewport.rows)])
                self.send(["op":"term_resize", "id":self.term, "cols":viewport.columns, "rows":viewport.rows])
            }
        })
        terminal.controller = controller
        terminal.configuration = TerminalSurfaceOptions(backend: .inMemory(session))
        slots[1].addSubview(terminal)
        if let url = ProcessInfo.processInfo.environment["WHEEL_NATIVE_WS"] { connect(url) }
        if !term.isEmpty { send(["op":"term_attach", "id":term]) }
    }
    func connect(_ url: String) {
        socket = URLSession.shared.webSocketTask(with: URL(string: url)!)
        socket.resume()
        receive()
    }
    var term = ProcessInfo.processInfo.environment["WHEEL_NATIVE_TERM"] ?? ""
    func send(_ value: [String:Any]) {
        guard socket != nil, let data = try? JSONSerialization.data(withJSONObject: value), let string = String(data: data, encoding: .utf8) else { return }
        // URLSession sends from independently spawned tasks may overtake each
        // other: preserve key/Return and resize ordering on the wire.
        let previous = sendTask
        sendTask = Task { @MainActor in
            await previous?.value
            do { try await socket.send(.string(string)) }
            catch { errors.append(error.localizedDescription) }
        }
    }
    func receive() {
        Task { @MainActor in
            do {
                let message = try await socket.receive()
                let data: Data
                switch message { case .string(let s): data = Data(s.utf8); case .data(let d): data = d; @unknown default: return }
                if let value = try JSONSerialization.jsonObject(with: data) as? [String:Any],
                   value["ev"] as? String == "term_output", let encoded = value["data"] as? String,
                   let bytes = Data(base64Encoded: encoded) { session.receive(bytes) }
                receive()
            } catch { errors.append(error.localizedDescription) }
        }
    }
    func layout(_ index: Int, _ rect: CGRect) {
        var frame = rect
        if !root.isFlipped { frame.origin.y = root.bounds.height - rect.maxY }
        let slot = slots[index]
        slot.frame = frame
        let child = index == 0 ? web : terminal as NSView
        child.frame = slot.bounds
        if index == 1 {
            terminal.setSurfaceVisible(true)
        }
        slot.cut(overlay)
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { loads.append(webView.title ?? "") }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: any Error) { errors.append(error.localizedDescription) }
    // Ghostty replaces the initial CAMetalLayer with IOSurfaceLayer. Read the
    // real renderer surface currently attached to the view, never redraw text.
    func terminalPixels() -> NSImage? {
        guard let contents = terminal.layer?.contents,
              CFGetTypeID(contents as CFTypeRef) == IOSurfaceGetTypeID() else { return nil }
        let surface = unsafeBitCast(contents as AnyObject, to: IOSurfaceRef.self)
        guard IOSurfaceLock(surface, [], nil) == 0 else { return nil }
        defer { IOSurfaceUnlock(surface, [], nil) }
        let width = IOSurfaceGetWidth(surface), height = IOSurfaceGetHeight(surface)
        let row = IOSurfaceGetBytesPerRow(surface)
        let address = IOSurfaceGetBaseAddress(surface)
        let data = Data(bytes:address, count:row * height)
        guard let provider = CGDataProvider(data:data as CFData),
              let cg = CGImage(width:width, height:height, bitsPerComponent:8, bitsPerPixel:32,
                bytesPerRow:row, space:CGColorSpaceCreateDeviceRGB(),
                bitmapInfo:CGBitmapInfo(rawValue:CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue),
                provider:provider, decode:nil, shouldInterpolate:false, intent:.defaultIntent) else { return nil }
        return NSImage(cgImage:cg, size:CGSize(width:width, height:height))
    }
    func png(_ image: NSImage?, _ path: String) -> Bool {
        guard let tiff = image?.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff), let png = bitmap.representation(using: .png, properties: [:]) else { return false }
        do { try png.write(to: URL(fileURLWithPath: path)); return true } catch { errors.append(error.localizedDescription); return false }
    }
    // Hash of the live Ghostty IOSurface, and what the masked slot composites.
    // rect is terminal-local top-left points.
    func probe(_ rect: [Double]?) -> [String:Any] {
        var result: [String:Any] = [:]
        if let contents = terminal.layer?.contents, CFGetTypeID(contents as CFTypeRef) == IOSurfaceGetTypeID() {
            let surface = unsafeBitCast(contents as AnyObject, to: IOSurfaceRef.self)
            IOSurfaceLock(surface, .readOnly, nil)
            defer { IOSurfaceUnlock(surface, .readOnly, nil) }
            let w = IOSurfaceGetWidth(surface), h = IOSurfaceGetHeight(surface), row = IOSurfaceGetBytesPerRow(surface)
            let base = IOSurfaceGetBaseAddress(surface).assumingMemoryBound(to: UInt8.self)
            let sx = Double(w) / max(terminal.bounds.width, 1), sy = Double(h) / max(terminal.bounds.height, 1)
            func hash(_ r: CGRect) -> String {
                var acc: UInt64 = 1469598103934665603
                let x0 = max(0, Int(r.minX * sx)), x1 = min(w, Int(r.maxX * sx)), y0 = max(0, Int(r.minY * sy)), y1 = min(h, Int(r.maxY * sy))
                if x0 < x1 { for y in y0..<max(y0, y1) { for b in (y * row + x0 * 4)..<(y * row + x1 * 4) { acc = (acc ^ UInt64(base[b])) &* 1099511628211 } } }
                return String(acc, radix: 16)
            }
            result["surface"] = [w, h]
            result["whole"] = hash(CGRect(x: 0, y: 0, width: terminal.bounds.width, height: terminal.bounds.height))
            if let a = rect {
                let hole = CGRect(x: a[0], y: a[1], width: a[2], height: a[3])
                result["under_hole"] = hash(hole)
                result["outside_hole"] = hash(CGRect(x: 0, y: 0, width: terminal.bounds.width, height: max(0, hole.minY)))
            }
        }
        // Composite the terminal slot's layer tree, mask included, into a bitmap.
        let slot = slots[1]
        let w = Int(slot.bounds.width), h = Int(slot.bounds.height)
        if w > 0, h > 0, let layer = slot.layer,
           let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                               space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) {
            layer.render(in: ctx)
            let data = ctx.data!.assumingMemoryBound(to: UInt8.self)
            func opaque(_ r: CGRect) -> Double {
                var n = 0, total = 0
                for y in max(0, Int(r.minY))..<min(h, Int(r.maxY)) { for x in max(0, Int(r.minX))..<min(w, Int(r.maxX)) { total += 1; if data[(y * w + x) * 4 + 3] > 0 { n += 1 } } }
                return total == 0 ? -1 : Double(n) / Double(total)
            }
            result["geometry_flipped"] = layer.isGeometryFlipped
            result["composited_whole_opaque"] = opaque(CGRect(x: 0, y: 0, width: w, height: h))
            if slot.hole != .zero {
                // bitmap rows run bottom-up; slot.hole is top-left points.
                let top = CGRect(x: slot.hole.minX, y: slot.hole.minY, width: slot.hole.width, height: slot.hole.height)
                let flipped = CGRect(x: slot.hole.minX, y: CGFloat(h) - slot.hole.maxY, width: slot.hole.width, height: slot.hole.height)
                result["composited_hole_opaque_topleft"] = opaque(top)
                result["composited_hole_opaque_bottomleft"] = opaque(flipped)
            }
        }
        return result
    }
    func command(_ value: [String:Any]) -> [String:Any] {
        switch value["op"] as? String {
        case "hide":
            let hidden = value["hidden"] as? Bool == true
            if hidden, let responder = root.window?.firstResponder as? NSView,
               responder === web || responder.isDescendant(of:web) || responder === terminal || responder.isDescendant(of:terminal) {
                proxyResponder = responder
            }
            slots.forEach { $0.isHidden = hidden }
            if !hidden, let responder = proxyResponder {
                _ = root.window?.makeFirstResponder(responder)
                proxyResponder = nil
            }
        case "load": web.load(URLRequest(url: URL(string: value["url"] as! String)!))
        case "js":
            js = NSNull()
            web.evaluateJavaScript(value["source"] as! String) { result, error in
                if let error { self.errors.append(error.localizedDescription) }
                self.js = result ?? NSNull()
            }
        case "focus":
            let target = value["target"] as! Int
            let responder: NSResponder = target == 0 ? web : target == 1 ? terminal : root
            return ["accepted":root.window?.makeFirstResponder(responder) ?? false]
        case "key":
            let text = value["text"] as! String
            if let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                windowNumber: root.window?.windowNumber ?? 0, context: nil, characters: text,
                charactersIgnoringModifiers: text, isARepeat: false, keyCode: UInt16(value["code"] as? Int ?? 0)) {
                root.window?.firstResponder?.keyDown(with: event)
            }
        case "ime":
            terminal.setMarkedText("に", selectedRange: NSRange(location: 1, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
            imeMarked = terminal.hasMarkedText()
            terminal.insertText("日本", replacementRange: NSRange(location: NSNotFound, length: 0))
            imeAfterCommit = terminal.hasMarkedText()
            terminal.unmarkText()
        case "overlay":
            if value["open"] as? Bool == true {
                savedResponder = root.window?.firstResponder
                let a = value["rect"] as! [Double]
                var rect = CGRect(x:a[0], y:a[1], width:a[2], height:a[3])
                if !root.isFlipped { rect.origin.y = root.bounds.height - rect.maxY }
                overlay = rect
                _ = root.window?.makeFirstResponder(root)
            } else {
                overlay = nil
                _ = root.window?.makeFirstResponder(savedResponder)
            }
            slots.forEach { $0.cut(overlay) }
        case "attach_term":
            term = value["id"] as! String
            if socket == nil, let url = value["url"] as? String { connect(url) }
            send(["op":"term_attach", "id":term])
        case "slot":
            slots[value["index"] as! Int].isHidden = value["hidden"] as? Bool == true
        case "pty":
            send(["op":"term_input", "id":term, "data":Data((value["text"] as! String).utf8).base64EncodedString()])
        case "post_key":
            // Real NSEvents handed to NSWindow.sendEvent: hit testing, first-responder
            // routing and the responders' own key handling run as for hardware events;
            // nothing touches the OS keyboard or the user's focus. GPUI's headless run
            // loop never dequeues NSApplication events, so NSApp.currentEvent stays nil
            // and posting through NSApp panics GPUI's delegate lookup.
            let text = value["text"] as! String
            let code = UInt16(value["code"] as? Int ?? 0)
            let flags: NSEvent.ModifierFlags = value["command"] as? Bool == true ? [.command] : []
            for type in [NSEvent.EventType.keyDown, .keyUp] {
                if let event = NSEvent.keyEvent(with: type, location: .zero, modifierFlags: flags,
                    timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: root.window?.windowNumber ?? 0,
                    context: nil, characters: text, charactersIgnoringModifiers: text, isARepeat: false, keyCode: code) {
                    root.window?.sendEvent(event)
                }
            }
        case "post_mouse":
            let kind = value["kind"] as! String
            let a = value["point"] as! [Double]
            var point = NSPoint(x:a[0], y:a[1])
            if !root.isFlipped { point.y = root.bounds.height - point.y }
            let eventType: NSEvent.EventType = kind == "down" ? .leftMouseDown : kind == "drag" ? .leftMouseDragged : .leftMouseUp
            mouseNumber += 1
            // NSWindow.sendEvent drops mouse events for an unordered window, so route as
            // AppKit does after its own step: hit test, then the view gets the NSEvent.
            // A drag or release goes to the view that took the press.
            if kind == "down" { mouseTarget = root.hitTest(root.convert(point, to: root.superview)) }
            if let event = NSEvent.mouseEvent(with: eventType, location: root.convert(point, to: nil), modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: root.window?.windowNumber ?? 0,
                context: nil, eventNumber: mouseNumber, clickCount: 1, pressure: kind == "up" ? 0 : 1), let target = mouseTarget {
                switch kind { case "down": target.mouseDown(with: event); case "drag": target.mouseDragged(with: event); default: target.mouseUp(with: event) }
            }
            return ["target": mouseTarget.map { String(describing: type(of: $0)) } ?? "nil"]
        case "ime_on_next_key":
            // An input method calls these during keyDown dispatch. A local monitor runs
            // inside that dispatch, so NSApp.currentEvent is set for Ghostty's guard.
            imeLog = []
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self else { return event }
                if let monitor = self.monitor { NSEvent.removeMonitor(monitor); self.monitor = nil }
                let none = NSRange(location: NSNotFound, length: 0)
                self.terminal.setMarkedText("に", selectedRange: NSRange(location: 1, length: 0), replacementRange: none)
                self.imeLog.append("marked:\(self.terminal.hasMarkedText())")
                self.terminal.insertText("日本", replacementRange: none)
                self.imeLog.append("after_commit_marked:\(self.terminal.hasMarkedText())")
                return nil
            }
        case "clipboard":
            let pasteboard = NSPasteboard.general
            switch value["action"] as! String {
            case "save":
                savedItems = (pasteboard.pasteboardItems ?? []).map { item in
                    Dictionary(uniqueKeysWithValues: item.types.compactMap { t in item.data(forType: t).map { (t, $0) } })
                }
            case "restore":
                pasteboard.clearContents()
                for entry in savedItems {
                    let item = NSPasteboardItem()
                    for (t, d) in entry { item.setData(d, forType: t) }
                    pasteboard.writeObjects([item])
                }
            case "set": pasteboard.clearContents(); pasteboard.setString(value["text"] as! String, forType: .string)
            case "copy_selection": return ["copied":terminal.copySelectedTextToPasteboard(), "text":pasteboard.string(forType: .string) ?? ""]
            case "paste_web": return ["sent":NSApp.sendAction(Selector(("paste:")), to: web, from: nil)]
            default: break
            }
            return ["text":pasteboard.string(forType: .string) ?? ""]
        case "probe":
            return probe(value["rect"] as? [Double])
        case "snapshot":
            let directory = value["directory"] as! String
            webSnapshot = "pending"
            web.takeSnapshot(with: nil) { image, error in
                self.webSnapshot = self.png(image, directory + "/web.png") ? "saved" : "failed"
                if let error { self.errors.append(error.localizedDescription) }
            }
            let metal = png(terminalPixels(), directory + "/terminal.png")
            let appkit = png(terminal.snapshotImage(), directory + "/terminal-appkit.png")
            var windowSaved = false
            if let rep = root.bitmapImageRepForCachingDisplay(in:root.bounds) {
                root.cacheDisplay(in:root.bounds, to:rep)
                let image = NSImage(size:root.bounds.size)
                image.addRepresentation(rep)
                windowSaved = png(image, directory + "/root-appkit.png")
            }
            return ["metal_saved":metal, "appkit_saved":appkit,"root_appkit_saved":windowSaved]
        case "hit":
            let a = value["point"] as! [Double]
            var point = NSPoint(x:a[0], y:a[1])
            if !root.isFlipped { point.y = root.bounds.height - point.y }
            let hit = root.hitTest(root.convert(point, to: root.superview))
            return ["class":hit.map { String(describing: type(of:$0)) } ?? "nil",
                    "web":hit.map { $0 === web || $0.isDescendant(of:web) } ?? false,
                    "terminal":hit.map { $0 === terminal || $0.isDescendant(of:terminal) } ?? false]
        default: break
        }
        return ["pid":ProcessInfo.processInfo.processIdentifier, "loads":loads, "errors":errors,
                "js":js, "writes":writes, "grids":grids,
                "viewport":session.readViewportText() ?? "", "ime_marked":imeMarked,"ime_log":imeLog,
                "ime_after_commit":imeAfterCommit, "snapshot":webSnapshot,
                "first_responder":root.window?.firstResponder.map { String(describing:type(of:$0)) } ?? "nil",
                "key_window":root.window?.isKeyWindow ?? false, "visible":root.window?.isVisible ?? false,
                "app_active":NSApp.isActive, "native_subviews":root.subviews.count,
                "frames_top":slots.map { [$0.frame.minX, root.isFlipped ? $0.frame.minY : root.bounds.height - $0.frame.maxY, $0.frame.width, $0.frame.height] },"frames":slots.map { [$0.frame.minX, $0.frame.minY, $0.frame.width, $0.frame.height] },
                "overlay_masked":slots.map { $0.layer?.mask != nil },
                "holes":slots.map { [$0.hole.minX, $0.hole.minY, $0.hole.width, $0.hole.height] },
                "slots_hidden":slots.map { $0.isHidden },
                "web_identity":String(describing:Unmanaged.passUnretained(web).toOpaque()),
                "terminal_identity":String(describing:Unmanaged.passUnretained(terminal).toOpaque()),
                "terminal_layer":String(describing:type(of:terminal.layer!)), "terminal_pixels":terminal.layer?.contents != nil]
    }
}

@MainActor private var native: Native?
@_cdecl("wheel_native_attach") @MainActor
public func attach(_ pointer: UnsafeMutableRawPointer, _ hidden: Int32) { native = Native(root: Unmanaged<NSView>.fromOpaque(pointer).takeUnretainedValue(), hidden: hidden != 0) }
@_cdecl("wheel_native_layout") @MainActor
public func layout(_ index: Int32, _ x: Double, _ y: Double, _ width: Double, _ height: Double) { native?.layout(Int(index), CGRect(x:x, y:y, width:width, height:height)) }
@_cdecl("wheel_native_command") @MainActor
public func command(_ pointer: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar> {
    do {
        let value = try JSONSerialization.jsonObject(with: Data(String(cString:pointer).utf8)) as! [String:Any]
        // GPUI may render before the bridge attaches to the window.
        let result = native?.command(value) ?? ["detached":true]
        let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
        return strdup(String(decoding:data, as:UTF8.self))!
    } catch { return strdup("{\"error\":\"JSON bridge failed\"}")! }
}
@_cdecl("wheel_native_free")
public func releaseString(_ pointer: UnsafeMutableRawPointer) { free(pointer) }
