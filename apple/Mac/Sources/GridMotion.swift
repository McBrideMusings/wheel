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
    private var bodies: [String: Int] = [:]
    private var lastBodyAt: CFTimeInterval?
    private var resizes = 0
    private var link: CADisplayLink?

    func start() {
        stop()
        frameMillis = []
        bodies = [:]
        resizes = 0
        lastTick = nil
        lastBodyAt = nil
        startedAt = CACurrentMediaTime()
        stoppedAt = 0
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
        Log.info("grid metrics: recording")
    }

    func stop() {
        guard recording else { return }
        recording = false
        stoppedAt = CACurrentMediaTime()
        link?.invalidate()
        link = nil
        Log.info("grid metrics: stopped after \(Int((stoppedAt - startedAt) * 1000)) ms, \(frameMillis.count) frames, \(bodies.values.reduce(0, +)) body evaluations, \(resizes) resizes")
    }

    @objc private func tick(_ link: CADisplayLink) {
        let now = CACurrentMediaTime()
        if let lastTick { frameMillis.append((now - lastTick) * 1000) }
        lastTick = now
    }

    func body(of pane: String) {
        guard recording else { return }
        bodies[pane, default: 0] += 1
        lastBodyAt = CACurrentMediaTime()
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
            "bodies": bodies,
            "bodies_total": bodies.values.reduce(0, +),
            "last_body_ms": rounded(lastBodyAt.map { max($0 - startedAt, 0) * 1000 } ?? 0),
            "term_resizes": resizes,
        ]
    }
}
