#if os(macOS)
import AppKit
import AVFoundation
import GameController
import os
import SwiftUI
import WheelKit

/// The video streams plugin pages show (`VideoStream{stream}`, ADR-0053) on the Mac: each asks wheeld for its socket with
/// `plugin_stream`, reads the backend's frames from it, decodes video with VideoToolbox onto an `AVSampleBufferDisplayLayer`,
/// plays audio through `AVAudioEngine`, and writes the person's keys, pointer and controller events back. A host makes one,
/// hands it every wheeld message (`handle`) and calls `connected` on each new connection.
@MainActor
public final class VideoStreams {
    private let send: (ClientMsg) -> Void
    private let nextReq: () -> UInt64
    private var waiting: [UInt64: StreamSession] = [:]
    private var live: [ObjectIdentifier: StreamSession] = [:]

    public init(send: @escaping (ClientMsg) -> Void, nextReq: @escaping () -> UInt64) {
        self.send = send
        self.nextReq = nextReq
    }

    /// The view of stream `stream` inside plugin pane `pane`'s page; `input` sends the person's input on it to the backend.
    public func view(pane: String, stream: String, input: Bool) -> AnyView {
        AnyView(VideoStreamHostView(streams: self, pane: pane, stream: stream, input: input).id("\(pane)/\(stream)"))
    }

    /// Ask again for every stream shown: wheeld may have restarted its backends.
    public func connected() {
        waiting = [:]
        for session in live.values { open(session) }
    }

    /// Take one wheeld message; true when it was for a stream.
    @discardableResult
    public func handle(_ msg: ServerMsg) -> Bool {
        switch msg {
        case let .pluginStream(req, _, _, socket):
            guard let session = waiting.removeValue(forKey: req) else { return false }
            session.connect(socket)
        case let .error(req?, _, message):
            guard let session = waiting.removeValue(forKey: req) else { return false }
            session.fail(message)
        default: return false
        }
        return true
    }

    /// What the debug state lists for plugin pane `pane`: each stream, its socket, the frames decoded, the audio chunks
    /// played, the input events sent and why it shows nothing.
    public func state(pane: String) -> [[String: Any]] {
        live.values.filter { $0.pane == pane }.map { $0.state }
    }

    /// A key typed on stream `stream` of plugin pane `pane`, as its view takes one: a key down and a key up event for
    /// `characters` handed to the view's own handlers, on the first such stream that takes input. For a check, which never
    /// sends real input; false when no such stream
    /// is shown.
    public func type(pane: String, stream: String, characters: String) -> Bool {
        guard let view = live.values.first(where: { $0.pane == pane && $0.stream == stream && $0.input })?.view else { return false }
        for type in [NSEvent.EventType.keyDown, .keyUp] {
            guard let event = NSEvent.keyEvent(with: type, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                               windowNumber: view.window?.windowNumber ?? 0, context: nil, characters: characters,
                                               charactersIgnoringModifiers: characters, isARepeat: false, keyCode: 0) else { return false }
            if type == .keyDown { view.keyDown(with: event) } else { view.keyUp(with: event) }
        }
        return true
    }

    /// A left click at the middle of stream `stream` of plugin pane `pane`, as its view takes one: a mouse down and up event
    /// handed to the view's own handlers, on the first such stream that takes input. For a check; false when no such stream is
    /// shown or the view is in no window.
    public func click(pane: String, stream: String) -> Bool {
        guard let view = live.values.first(where: { $0.pane == pane && $0.stream == stream && $0.input })?.view,
              let window = view.window else { return false }
        let middle = view.convert(NSPoint(x: view.bounds.midX, y: view.bounds.midY), to: nil)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            guard let event = NSEvent.mouseEvent(with: type, location: middle, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                                 windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)
            else { return false }
            if type == .leftMouseDown { view.mouseDown(with: event) } else { view.mouseUp(with: event) }
        }
        return true
    }

    func appear(_ session: StreamSession) {
        live[ObjectIdentifier(session)] = session
        session.retry = { [weak self, weak session] in
            guard let self, let session, self.live[ObjectIdentifier(session)] != nil else { return }
            self.open(session)
        }
        open(session)
    }

    func disappear(_ session: StreamSession) {
        live[ObjectIdentifier(session)] = nil
        waiting = waiting.filter { $0.value !== session }
        session.close()
    }

    private func open(_ session: StreamSession) {
        let req = nextReq()
        waiting[req] = session
        send(.pluginStream(req: req, pane: session.pane, stream: session.stream))
    }
}

/// What a stream's reader counts, read on the main actor for the note and the debug state.
final class StreamStats: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: Int] = [:]
    func add(_ key: String, _ n: Int = 1) { lock.withLock { values[key, default: 0] += n } }
    subscript(_ key: String) -> Int { lock.withLock { values[key] ?? 0 } }
}

/// One stream shown in a page: its view, and the connection to the backend while one is open.
@MainActor @Observable
final class StreamSession {
    let pane: String
    let stream: String
    let input: Bool
    /// Why the stream shows no picture: connecting, refused, ended. Nil once frames show.
    private(set) var note: String?
    @ObservationIgnored let view: StreamView
    @ObservationIgnored let stats = StreamStats()
    @ObservationIgnored private var link: StreamLink?
    @ObservationIgnored private var socket: String?
    @ObservationIgnored private var error: String?
    @ObservationIgnored private var wait: Duration = .seconds(1)
    @ObservationIgnored var retry: (() -> Void)?
    /// The audio's gain, 0 to 1 (`VideoStream{volume, muted}`); kept across reconnects.
    @ObservationIgnored var gain: Float = 1 {
        didSet { link?.gain = gain }
    }

    init(pane: String, stream: String, input: Bool) {
        self.pane = pane
        self.stream = stream
        self.input = input
        self.note = "Connecting to \(stream)"
        view = StreamView(frame: .zero)
        view.takesInput = input
        view.send = { [weak self] kind, event in self?.send(kind, event) }
        if input { StreamControllers.shared.add(self) }
    }

    isolated deinit { link?.close() }

    var state: [String: Any] {
        var o: [String: Any] = ["stream": stream, "frames": stats["frames"], "audio": stats["audio"], "inputs": stats["inputs"],
                                "bytes": stats["bytes"]]
        if let socket { o["socket"] = socket }
        if let error { o["error"] = error }
        if let size = link?.videoSize { o["video"] = [Int(size.width), Int(size.height)] }
        return o
    }

    func connect(_ path: String) {
        link?.close()
        socket = path
        let link = StreamLink(path: path, renderer: view.display.sampleBufferRenderer, stats: stats)
        link.onFirstFrame = { [weak self] in Task { @MainActor in self?.shown() } }
        link.onEnd = { [weak self, weak link] why in Task { @MainActor in if let link { self?.ended(link, why) } } }
        view.videoSize = { [weak link] in link?.videoSize }
        link.gain = gain
        self.link = link
        link.start()
    }

    func fail(_ message: String) {
        error = message
        note = message
        link?.close()
        link = nil
    }

    func close() {
        link?.close()
        link = nil
    }

    private func shown() {
        error = nil
        note = nil
        wait = .seconds(1)
    }

    /// The connection ended: say why, and ask wheeld again after a wait that doubles to 30 s, since a restarted backend
    /// serves the stream again on a new socket.
    private func ended(_ ended: StreamLink, _ why: String) {
        guard ended === link else { return }
        link = nil
        error = why
        note = why
        let wait = self.wait
        self.wait = min(wait * 2, .seconds(30))
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: wait)
            self?.retry?()
        }
    }

    func send(_ kind: StreamFrame.Input, _ event: [String: Any]) {
        guard input, let link else { return }
        link.write(StreamFrame.input(kind, event))
        stats.add("inputs")
    }

    /// Whether controller events go here: the stream takes input and its window is the key one.
    var wantsControllers: Bool { input && link != nil && view.window?.isKeyWindow == true }
}

/// One open connection to a stream socket: a thread reads and decodes, a queue writes.
final class StreamLink: @unchecked Sendable {
    private let path: String
    private let renderer: AVSampleBufferVideoRenderer
    private let stats: StreamStats
    private let writes = DispatchQueue(label: "wheel.videostream.write")
    private let lock = NSLock()
    private var fd: Int32 = -1
    private var closed = false
    private var size: CGSize?
    /// Whether this connection has shown a frame yet; each new connection reports its first.
    private var showing = false
    /// The audio's gain, 0 (silent) to 1, as the pane set it; read on the read thread before each chunk plays.
    private var gainValue: Float = 1
    var gain: Float {
        get { lock.withLock { gainValue } }
        set { lock.withLock { gainValue = newValue } }
    }
    var onFirstFrame: (@Sendable () -> Void)?
    var onEnd: (@Sendable (String) -> Void)?

    init(path: String, renderer: AVSampleBufferVideoRenderer, stats: StreamStats) {
        self.path = path
        self.renderer = renderer
        self.stats = stats
    }

    /// The video's size in pixels, once parameter sets came.
    var videoSize: CGSize? { lock.withLock { size } }

    func start() {
        let thread = Thread { [self] in run() }
        thread.name = "wheel.videostream.read"
        thread.qualityOfService = .userInteractive
        thread.start()
    }

    /// Ends the connection; the read thread sees the end and returns without reporting it.
    func close() {
        lock.withLock {
            closed = true
            if fd >= 0 { shutdown(fd, SHUT_RDWR) }
        }
    }

    func write(_ data: Data) {
        writes.async { [self] in
            let fd = lock.withLock { closed ? -1 : self.fd }
            guard fd >= 0 else { return }
            data.withUnsafeBytes { raw in
                var at = 0
                while at < raw.count {
                    let n = Darwin.write(fd, raw.baseAddress! + at, raw.count - at)
                    if n <= 0 { return }
                    at += n
                }
            }
        }
    }

    private func run() {
        let why = read()
        let fd = lock.withLock { () -> Int32 in let fd = self.fd; self.fd = -1; return fd }
        writes.sync {}
        if fd >= 0 { Darwin.close(fd) }
        renderer.flush(removingDisplayedImage: false, completionHandler: nil)
        if !lock.withLock({ closed }) { onEnd?(why) }
    }

    /// A decoded frame, on VideoToolbox's thread: onto the layer at once.
    private func show(_ sample: CMSampleBuffer) {
        if renderer.status == .failed { renderer.flush() }
        renderer.enqueue(sample)
        stats.add("frames")
        let first = lock.withLock { () -> Bool in
            defer { showing = true }
            return !showing
        }
        if first { onFirstFrame?() }
    }

    /// Connects and reads until the stream ends; returns why it ended.
    private func read() -> String {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return "no socket: \(String(cString: strerror(errno)))" }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
            Darwin.close(fd)
            return "the stream socket path is too long: \(path)"
        }
        withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: bytes) }
        let connected = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard connected == 0 else {
            let why = String(cString: strerror(errno))
            Darwin.close(fd)
            return "The backend does not serve \(URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent) yet (\(why))"
        }
        let open = lock.withLock { () -> Bool in
            guard !closed else { return false }
            self.fd = fd
            return true
        }
        guard open else { Darwin.close(fd); return "closed" }

        let audio = StreamAudio()
        defer { audio.stop() }
        let decoder = StreamDecoder { [weak self] sample in self?.show(sample) }
        var reader = StreamFrameReader()
        var buffer = [UInt8](repeating: 0, count: 1 << 16)
        while true {
            let n = buffer.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress!, $0.count) }
            if n == 0 { return "The stream ended" }
            if n < 0 { return "The stream ended: \(String(cString: strerror(errno)))" }
            stats.add("bytes", n)
            let frames: [StreamFrame]
            do { frames = try reader.push(Data(buffer[0..<n])) } catch { return "The stream broke: \(error)" }
            for frame in frames {
                switch frame {
                case let .parameterSets(codec, units):
                    if !decoder.parameterSets(codec: codec, units: units) { return decoder.lastError ?? "bad parameter sets" }
                    if let d = decoder.dimensions { lock.withLock { size = CGSize(width: Int(d.width), height: Int(d.height)) } }
                case let .video(time, units):
                    decoder.decode(time: time, units: units)
                case let .audio(rate, channels, pcm):
                    audio.gain = gain
                    if audio.play(rate: Double(rate), channels: Int(channels), pcm: pcm) { stats.add("audio") }
                case .unknown:
                    break
                }
            }
        }
    }
}

/// A stream's audio: interleaved s16 PCM converted to float into a ring the engine's render thread pulls from through an
/// `AVAudioSourceNode`. Nothing here calls a player's `play()`, which raises an Objective-C exception Swift cannot catch when
/// the engine has not run an I/O cycle yet. The ring holds at most `backlogSeconds`; past it the oldest samples go, so sound
/// never lags the picture. `play` runs on the read thread.
final class StreamAudio {
    private let engine = AVAudioEngine()
    private var source: AVAudioSourceNode?
    private var rate = 0.0
    private var channels = 0
    private var running = false
    private let ring = StreamAudioRing()
    static let backlogSeconds = 0.25
    /// The mixer's output gain, 0 to 1.
    var gain: Float = 1 {
        didSet { if gain != oldValue { engine.mainMixerNode.outputVolume = gain } }
    }

    /// Queues one chunk; false when no audio device took the stream.
    func play(rate: Double, channels: Int, pcm: Data) -> Bool {
        if rate != self.rate || channels != self.channels { start(rate: rate, channels: channels) }
        guard running else { return false }
        let frames = pcm.count / (2 * channels)
        var samples = [Float](repeating: 0, count: frames * channels)
        pcm.withUnsafeBytes { raw in
            for i in 0..<(frames * channels) {
                samples[i] = Float(Int16(littleEndian: raw.loadUnaligned(fromByteOffset: i * 2, as: Int16.self))) / 32768
            }
        }
        ring.push(samples, keep: Int(rate * Self.backlogSeconds) * channels)
        return true
    }

    private func start(rate: Double, channels: Int) {
        stop()
        self.rate = rate
        self.channels = channels
        guard let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: rate, channels: AVAudioChannelCount(channels),
                                         interleaved: false) else { return }
        let ring = self.ring
        let node = AVAudioSourceNode(format: format) { _, _, frameCount, buffers -> OSStatus in
            ring.pull(into: UnsafeMutableAudioBufferListPointer(buffers), frames: Int(frameCount), channels: channels)
            return noErr
        }
        engine.attach(node)
        engine.connect(node, to: engine.mainMixerNode, format: format)
        // A hidden check (`WHEEL_MUTE=1`) plays through the engine at volume 0: the audio path runs, nobody hears it.
        engine.mainMixerNode.outputVolume = ProcessInfo.processInfo.environment["WHEEL_MUTE"] == "1" ? 0 : gain
        source = node
        do {
            try engine.start()
            running = true
        } catch {
            running = false
        }
    }

    func stop() {
        engine.stop()
        if let source { engine.detach(source) }
        source = nil
        running = false
        ring.clear()
    }
}

/// Interleaved samples waiting to play, shared by the read thread and the render thread.
final class StreamAudioRing: Sendable {
    private struct Queue {
        var samples: [Float] = []
        var head = 0
    }
    private let queue = OSAllocatedUnfairLock(initialState: Queue())

    func push(_ more: [Float], keep: Int) {
        queue.withLock { q in
            q.samples.removeFirst(q.head)
            q.head = 0
            q.samples += more
            if q.samples.count > keep { q.samples.removeFirst(q.samples.count - keep) }
        }
    }

    /// Fills each channel's buffer with the next `frames` frames, silence where none wait.
    func pull(into buffers: UnsafeMutableAudioBufferListPointer, frames: Int, channels: Int) {
        // Unchecked: the buffers are the render thread's own and never leave this call.
        queue.withLockUnchecked { q in
            let have = min(frames, (q.samples.count - q.head) / max(channels, 1))
            for (ch, buffer) in buffers.enumerated() {
                guard let out = buffer.mData?.assumingMemoryBound(to: Float.self) else { continue }
                for i in 0..<frames { out[i] = i < have && ch < channels ? q.samples[q.head + i * channels + ch] : 0 }
            }
            q.head += have * channels
        }
    }

    func clear() {
        queue.withLock { $0 = Queue() }
    }
}

/// Game controllers, read with GameController and sent to every stream that takes input in the key window.
@MainActor
final class StreamControllers {
    static let shared = StreamControllers()
    private var sessions: [WeakSession] = []
    private var watching = false

    private struct WeakSession { weak var session: StreamSession? }

    func add(_ session: StreamSession) {
        sessions = sessions.filter { $0.session != nil } + [WeakSession(session: session)]
        guard !watching else { return }
        watching = true
        GCController.controllers().forEach(watch)
        NotificationCenter.default.addObserver(forName: .GCControllerDidConnect, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { GCController.controllers().forEach(StreamControllers.shared.watch) }
        }
        NotificationCenter.default.addObserver(forName: .GCControllerDidDisconnect, object: nil, queue: .main) { note in
            guard let pad = note.object as? GCController else { return }
            let index = pad.playerIndex.rawValue
            MainActor.assumeIsolated { StreamControllers.shared.broadcast(["kind": "disconnected", "controller": index]) }
        }
    }

    private func watch(_ pad: GCController) {
        if pad.playerIndex == .indexUnset {
            let used = Set(GCController.controllers().map(\.playerIndex.rawValue))
            if let free = (0..<4).first(where: { !used.contains($0) }) { pad.playerIndex = GCControllerPlayerIndex(rawValue: free) ?? .indexUnset }
        }
        pad.handlerQueue = .main
        pad.extendedGamepad?.valueChangedHandler = { gamepad, _ in
            let event = StreamControllers.event(gamepad, index: pad.playerIndex.rawValue)
            MainActor.assumeIsolated { StreamControllers.shared.broadcast(event) }
        }
    }

    private func broadcast(_ event: [String: Any]) {
        for s in sessions.compactMap(\.session) where s.wantsControllers { s.send(.controller, event) }
    }

    /// A controller's whole state: every button pressed or not, and each stick and trigger from -1 to 1 (triggers 0 to 1).
    nonisolated static func event(_ g: GCExtendedGamepad, index: Int) -> [String: Any] {
        let buttons: [String: Bool] = [
            "a": g.buttonA.isPressed, "b": g.buttonB.isPressed, "x": g.buttonX.isPressed, "y": g.buttonY.isPressed,
            "lb": g.leftShoulder.isPressed, "rb": g.rightShoulder.isPressed,
            "l3": g.leftThumbstickButton?.isPressed ?? false, "r3": g.rightThumbstickButton?.isPressed ?? false,
            "menu": g.buttonMenu.isPressed, "options": g.buttonOptions?.isPressed ?? false, "home": g.buttonHome?.isPressed ?? false,
            "up": g.dpad.up.isPressed, "down": g.dpad.down.isPressed, "left": g.dpad.left.isPressed, "right": g.dpad.right.isPressed,
        ]
        let axes: [String: Double] = [
            "lx": Double(g.leftThumbstick.xAxis.value), "ly": Double(g.leftThumbstick.yAxis.value),
            "rx": Double(g.rightThumbstick.xAxis.value), "ry": Double(g.rightThumbstick.yAxis.value),
            "lt": Double(g.leftTrigger.value), "rt": Double(g.rightTrigger.value),
        ]
        return ["kind": "state", "controller": index, "buttons": buttons, "axes": axes]
    }
}

/// The view a stream draws in: the display layer, fitted to the view with the video's aspect (or filling it, cropped,
/// when `fills`), and the keys and pointer events on it, sent as input frames. A pointer's `x` and `y` are in the
/// video's pixels, from its top left.
final class StreamView: NSView {
    let display = AVSampleBufferDisplayLayer()
    var takesInput = true
    var send: ((StreamFrame.Input, [String: Any]) -> Void)?
    var videoSize: () -> CGSize? = { nil }
    var fills = false {
        didSet { display.videoGravity = fills ? .resizeAspectFill : .resizeAspect }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.cgColor
        display.videoGravity = .resizeAspect
        layer?.addSublayer(display)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        display.frame = bounds
        CATransaction.commit()
    }

    override var acceptsFirstResponder: Bool { takesInput }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { takesInput }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect], owner: self))
    }

    /// Where `event` falls in the video, in its pixels from the top left; nil before the video's size is known.
    func videoPoint(_ event: NSEvent) -> CGPoint? {
        guard let size = videoSize(), size.width > 0, size.height > 0 else { return nil }
        let rect = Self.videoRect(size, in: bounds, fills: fills)
        let p = convert(event.locationInWindow, from: nil)
        let x = min(max((p.x - rect.minX) / rect.width, 0), 1) * size.width
        let y = min(max((isFlipped ? p.y - rect.minY : rect.maxY - p.y) / rect.height, 0), 1) * size.height
        return CGPoint(x: x.rounded(), y: y.rounded())
    }

    /// Where a picture of `size` draws in `bounds`: fitted inside, or filling it with the overflow past its edges.
    static func videoRect(_ size: CGSize, in bounds: CGRect, fills: Bool) -> CGRect {
        guard fills else { return AVMakeRect(aspectRatio: size, insideRect: bounds) }
        let scale = max(bounds.width / size.width, bounds.height / size.height)
        let w = size.width * scale, h = size.height * scale
        return CGRect(x: bounds.midX - w / 2, y: bounds.midY - h / 2, width: w, height: h)
    }

    private func pointer(_ kind: String, _ event: NSEvent, button: Int? = nil) {
        guard takesInput, let at = videoPoint(event) else { return }
        var o: [String: Any] = ["kind": kind, "x": Int(at.x), "y": Int(at.y)]
        if let button { o["button"] = button }
        send?(.pointer, o)
    }

    override func mouseDown(with event: NSEvent) {
        guard takesInput else { return super.mouseDown(with: event) }
        window?.makeFirstResponder(self)
        pointer("down", event, button: 0)
    }
    override func mouseUp(with event: NSEvent) { pointer("up", event, button: 0) }
    override func rightMouseDown(with event: NSEvent) {
        guard takesInput else { return super.rightMouseDown(with: event) }
        window?.makeFirstResponder(self)
        pointer("down", event, button: 1)
    }
    override func rightMouseUp(with event: NSEvent) { pointer("up", event, button: 1) }
    override func otherMouseDown(with event: NSEvent) { pointer("down", event, button: event.buttonNumber) }
    override func otherMouseUp(with event: NSEvent) { pointer("up", event, button: event.buttonNumber) }
    override func mouseMoved(with event: NSEvent) { pointer("move", event) }
    override func mouseDragged(with event: NSEvent) { pointer("move", event) }
    override func rightMouseDragged(with event: NSEvent) { pointer("move", event) }
    override func otherMouseDragged(with event: NSEvent) { pointer("move", event) }
    override func scrollWheel(with event: NSEvent) {
        guard takesInput else { return super.scrollWheel(with: event) }
        send?(.pointer, ["kind": "scroll", "dx": Double(event.scrollingDeltaX), "dy": Double(event.scrollingDeltaY),
                         "precise": event.hasPreciseScrollingDeltas])
    }

    override func keyDown(with event: NSEvent) {
        guard takesInput else { return super.keyDown(with: event) }
        send?(.key, Self.key("down", event))
    }
    override func keyUp(with event: NSEvent) {
        guard takesInput else { return super.keyUp(with: event) }
        send?(.key, Self.key("up", event))
    }
    override func flagsChanged(with event: NSEvent) {
        guard takesInput else { return super.flagsChanged(with: event) }
        send?(.key, ["kind": "modifiers", "code": Int(event.keyCode), "modifiers": Self.modifiers(event.modifierFlags)])
    }

    /// A key event: `code` is the Mac's virtual key code, `key` the characters it types without modifiers.
    static func key(_ kind: String, _ event: NSEvent) -> [String: Any] {
        ["kind": kind, "code": Int(event.keyCode), "key": event.charactersIgnoringModifiers ?? "", "repeat": event.isARepeat,
         "modifiers": modifiers(event.modifierFlags)]
    }

    static func modifiers(_ flags: NSEvent.ModifierFlags) -> [String] {
        [(NSEvent.ModifierFlags.shift, "shift"), (.control, "control"), (.option, "option"), (.command, "command"), (.capsLock, "capslock")]
            .filter { flags.contains($0.0) }.map(\.1)
    }
}

private struct VideoStreamHostView: View {
    let streams: VideoStreams
    @Environment(\.paneVideoStreamFills) private var fills
    @Environment(\.paneVideoStreamGain) private var gain
    @State private var session: StreamSession

    init(streams: VideoStreams, pane: String, stream: String, input: Bool) {
        self.streams = streams
        _session = State(initialValue: StreamSession(pane: pane, stream: stream, input: input))
    }

    var body: some View {
        StreamViewHost(view: session.view, fills: fills)
            .overlay {
                if let note = session.note {
                    Text(note)
                        .font(.footnote)
                        .foregroundStyle(.white.opacity(0.8))
                        .multilineTextAlignment(.center)
                        .padding(8)
                }
            }
            .onAppear {
                session.gain = gain
                streams.appear(session)
            }
            .onChange(of: gain) { session.gain = gain }
            .onDisappear { streams.disappear(session) }
    }
}

private struct StreamViewHost: NSViewRepresentable {
    let view: StreamView
    let fills: Bool
    func makeNSView(context: Context) -> StreamView {
        view.fills = fills
        return view
    }
    func updateNSView(_ view: StreamView, context: Context) {
        if view.fills != fills { view.fills = fills }
    }
}
#endif
