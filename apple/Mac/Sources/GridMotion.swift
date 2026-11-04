import AppKit
import QuartzCore
import SwiftUI

/// How long the grid's layout animations last (zoom, split, close, swap, pane-drag preview;
/// not the sidebar's space reorder). `WHEEL_SLOW_ANIMATIONS=<n>` multiplies their duration by `n`,
/// so a person can watch a layout change slowly; unset, empty, zero or not a number means 1.
enum GridMotion {
    static let slowdown: Double = {
        guard let raw = ProcessInfo.processInfo.environment["WHEEL_SLOW_ANIMATIONS"], let n = Double(raw), n > 0 else { return 1 }
        return n
    }()

    /// The duration of a grid layout change at normal speed, in seconds.
    static let layoutDuration = 0.22

    /// The duration a grid animation runs, in seconds, after `WHEEL_SLOW_ANIMATIONS`.
    static var duration: Double { layoutDuration * slowdown }

    /// The animation every grid layout change uses.
    static var layout: Animation { .smooth(duration: duration) }
}

/// What a layout change costs, counted while a person or a script asks: the gaps between the
/// display's frames on the main thread, how often each pane's view body runs, and how many
/// terminal resizes go to wheeld. Recording is off until `start`, so a body evaluation outside a
/// measurement costs one flag test.
@MainActor
final class GridMetrics: NSObject {
    static let shared = GridMetrics()

    private(set) var recording = false
    private var startedAt: CFTimeInterval = 0
    private var stoppedAt: CFTimeInterval = 0
    private var lastTick: CFTimeInterval?
    private var frameMillis: [Double] = []
    /// Each frame over 34 ms: when it ended, in ms since `start`, and how long it took.
    private var longFrames: [(at: Double, ms: Double)] = []
    /// When each window layout change was committed, in ms since `start`.
    private var commits: [Double] = []
    /// Each switch a press made: ms from the mouse-down to the window layout, to the first frame
    /// (the selection and the title) and to the frame after the grid shows the space.
    private var switches: [[String: Int]] = []
    private var bodies: [String: Int] = [:]
    private var lastBodyAt: CFTimeInterval?
    private var resizes = 0
    private var link: CADisplayLink?
    /// Each stretch the main thread spent awake over 16 ms: when it ended, in ms since `start`, and how long.
    private var busy: [(at: Double, ms: Double)] = []
    private var wokeAt: CFTimeInterval?
    private var observer: CFRunLoopObserver?
    /// The main thread's CPU time at `start` and at `stop`, in ms: what the recording cost it,
    /// whatever else the machine was running.
    private var cpuAtStart: Double = 0
    private var cpuAtStop: Double = 0

    func start() {
        stop()
        frameMillis = []
        longFrames = []
        commits = []
        switches = []
        busy = []
        wokeAt = nil
        bodies = [:]
        resizes = 0
        lastTick = nil
        lastBodyAt = nil
        startedAt = CACurrentMediaTime()
        stoppedAt = 0
        cpuAtStart = Self.mainThreadCPU()
        recording = true
        // The display link is delivered on the main run loop, so a tick that arrives late means the
        // main thread was busy: the gap between two ticks is the frame's time.
        if let screen = NSScreen.main ?? NSScreen.screens.first {
            let link = screen.displayLink(target: self, selector: #selector(tick(_:)))
            link.add(to: .main, forMode: .common)
            self.link = link
        } else {
            Log.info("grid metrics: no screen, so no frame gaps are recorded")
        }
        // The main run loop wakes, works and sleeps again; a stretch awake longer than a frame is
        // main-thread work, where a long frame with no such stretch was spent outside it.
        let observer = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.afterWaiting.rawValue | CFRunLoopActivity.beforeWaiting.rawValue, true, 0) { [weak self] _, activity in
            MainActor.assumeIsolated { self?.runLoop(activity) }
        }
        CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
        self.observer = observer
        Log.info("grid metrics: recording")
    }

    func stop() {
        guard recording else { return }
        recording = false
        stoppedAt = CACurrentMediaTime()
        cpuAtStop = Self.mainThreadCPU()
        link?.invalidate()
        link = nil
        if let observer { CFRunLoopObserverInvalidate(observer) }
        observer = nil
        Log.info("grid metrics: stopped after \(Int((stoppedAt - startedAt) * 1000)) ms, \(frameMillis.count) frames, \(bodies.values.reduce(0, +)) body evaluations, \(resizes) resizes")
    }

    @objc private func tick(_ link: CADisplayLink) {
        let now = CACurrentMediaTime()
        if let lastTick {
            let ms = (now - lastTick) * 1000
            frameMillis.append(ms)
            if ms > 34 { longFrames.append(((now - startedAt) * 1000, ms)) }
        }
        lastTick = now
    }

    /// This thread's user and system CPU time in ms; called on the main thread.
    private static func mainThreadCPU() -> Double {
        var info = thread_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<thread_basic_info>.size / MemoryLayout<natural_t>.size)
        let port = mach_thread_self()
        defer { mach_port_deallocate(mach_task_self_, port) }
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { thread_info(port, thread_flavor_t(THREAD_BASIC_INFO), $0, &count) }
        }
        guard kr == KERN_SUCCESS else { return 0 }
        func ms(_ t: time_value_t) -> Double { Double(t.seconds) * 1000 + Double(t.microseconds) / 1000 }
        return ms(info.user_time) + ms(info.system_time)
    }

    private func runLoop(_ activity: CFRunLoopActivity) {
        let now = CACurrentMediaTime()
        if activity == .afterWaiting {
            wokeAt = now
        } else if let wokeAt {
            let ms = (now - wokeAt) * 1000
            if ms > 16 { busy.append(((now - startedAt) * 1000, ms)) }
            self.wokeAt = nil
        }
    }

    func body(of pane: String) {
        guard recording else { return }
        bodies[pane, default: 0] += 1
        lastBodyAt = CACurrentMediaTime()
    }

    func windowsCommitted() {
        if recording { commits.append((CACurrentMediaTime() - startedAt) * 1000) }
    }

    func switched(windows: Int, frame: Int, grid: Int) {
        if recording { switches.append(["press_to_windows_ms": windows, "press_to_frame_ms": frame, "press_to_grid_ms": grid]) }
    }

    func resizeSent() {
        if recording { resizes += 1 }
    }

    /// The numbers so far, or at `stop`.
    func report() -> [String: Any] {
        let end = recording ? CACurrentMediaTime() : stoppedAt
        let sorted = frameMillis.sorted()
        func percentile(_ p: Double) -> Double {
            sorted.isEmpty ? 0 : sorted[min(sorted.count - 1, Int(Double(sorted.count) * p))]
        }
        func rounded(_ x: Double) -> Double { (x * 100).rounded() / 100 }
        return [
            "recording": recording,
            "slowdown": GridMotion.slowdown,
            "animation_ms": rounded(GridMotion.duration * 1000),
            "duration_ms": rounded(max(end - startedAt, 0) * 1000),
            "frames": sorted.count,
            "frame_ms": [
                "mean": rounded(sorted.isEmpty ? 0 : sorted.reduce(0, +) / Double(sorted.count)),
                "p50": rounded(percentile(0.5)), "p95": rounded(percentile(0.95)), "max": rounded(sorted.last ?? 0),
                "over_17ms": sorted.filter { $0 > 17 }.count, "over_34ms": sorted.filter { $0 > 34 }.count,
            ] as [String: Any],
            "long_frames": longFrames.map { ["at_ms": rounded($0.at), "ms": rounded($0.ms)] },
            "commits_ms": commits.map(rounded),
            "switches": switches,
            "main_cpu_ms": rounded((recording ? Self.mainThreadCPU() : cpuAtStop) - cpuAtStart),
            "busy": busy.map { ["at_ms": rounded($0.at), "ms": rounded($0.ms)] },
            "bodies": bodies,
            "bodies_total": bodies.values.reduce(0, +),
            "last_body_ms": rounded(lastBodyAt.map { max($0 - startedAt, 0) * 1000 } ?? 0),
            "term_resizes": resizes,
        ]
    }
}

/// Calls back once, on the main thread, at the next display frame, with its time
/// (`ProcessInfo.systemUptime`, the clock event timestamps use).
@MainActor
final class FrameWaiter: NSObject {
    private static var waiting: [FrameWaiter] = []
    private var link: CADisplayLink?
    private let done: (TimeInterval) -> Void

    private init(_ done: @escaping (TimeInterval) -> Void) { self.done = done }

    static func next(_ done: @escaping (TimeInterval) -> Void) {
        // With no screen there is no frame to wait for: the next run loop turn stands in for it.
        guard let screen = NSScreen.main ?? NSScreen.screens.first else {
            DispatchQueue.main.async { done(ProcessInfo.processInfo.systemUptime) }
            return
        }
        let waiter = FrameWaiter(done)
        let link = screen.displayLink(target: waiter, selector: #selector(tick(_:)))
        link.add(to: .main, forMode: .common)
        waiter.link = link
        waiting.append(waiter)
    }

    @objc private func tick(_: CADisplayLink) {
        link?.invalidate()
        link = nil
        Self.waiting.removeAll { $0 === self }
        done(ProcessInfo.processInfo.systemUptime)
    }
}
