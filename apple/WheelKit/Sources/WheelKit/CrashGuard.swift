import Foundation
import Observation

/// Names what the app was drawing when it died, so the next launch can leave that one space or pane
/// out instead of crashing on it again.
///
/// Swift cannot catch a trap (an inverted range, a bad index, a force-unwrap of nil): it ends the
/// process. So the app records, in two fixed buffers, the space and the pane it is working on, and a
/// handler for the fatal signals writes both to a file as the process dies. Nothing touches the disk
/// while the app runs well. The next launch reads that file once (`Crash.read`), and `Quarantine`
/// holds the named space and pane out until the person opens them anyway.
///
/// Marks come from the main thread only. A mark lasts until the main run loop next turns, so a crash
/// while a view tree is being built names the space or pane being built, and a crash at any other
/// time names nothing and quarantines nothing.
public enum CrashGuard {
    private static let capacity = 160
    /// Fixed C buffers: the signal handler may call no code that allocates or takes a lock.
    private nonisolated(unsafe) static let spaceSlot = UnsafeMutablePointer<CChar>.allocate(capacity: capacity)
    private nonisolated(unsafe) static let paneSlot = UnsafeMutablePointer<CChar>.allocate(capacity: capacity)
    private nonisolated(unsafe) static var path: UnsafeMutablePointer<CChar>?
    private nonisolated(unsafe) static var clearScheduled = false
    /// Off only in tests, which raise a signal the process must survive.
    nonisolated(unsafe) static var reraises = true

    /// The signals a trap, a bad access or an abort raises.
    public static let fatalSignals = [SIGTRAP, SIGILL, SIGSEGV, SIGBUS, SIGABRT, SIGFPE]

    /// Start recording to `file`. Call once, early, before any view is built.
    public static func install(file: URL, signals: [Int32] = fatalSignals) {
        spaceSlot.pointee = 0
        paneSlot.pointee = 0
        if let old = path { free(old) }
        path = strdup(file.path)
        for sig in signals {
            signal(sig) { sig in CrashGuard.record(signal: sig) }
        }
    }

    /// A new space starts its own work: a pane marked while another space was built is not this one's.
    public static func mark(space id: String) {
        paneSlot.pointee = 0
        set(spaceSlot, "space:\(id)")
    }

    public static func mark(pane id: String) { set(paneSlot, "pane:\(id)") }

    /// Forget both marks.
    public static func clear() {
        spaceSlot.pointee = 0
        paneSlot.pointee = 0
        clearScheduled = false
    }

    private static func set(_ slot: UnsafeMutablePointer<CChar>, _ text: String) {
        var i = 0
        for byte in text.utf8 where i < capacity - 1 {
            slot[i] = CChar(bitPattern: byte)
            i += 1
        }
        slot[i] = 0
        guard !clearScheduled else { return }
        clearScheduled = true
        DispatchQueue.main.async { clear() }
    }

    /// Runs in the signal handler: system calls and the fixed buffers only.
    private static func record(signal sig: Int32) {
        if let path {
            let fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0o600)
            if fd >= 0 {
                writeLine(fd, spaceSlot)
                writeLine(fd, paneSlot)
                close(fd)
            }
        }
        guard reraises else { return }
        signal(sig, SIG_DFL)
        raise(sig)
    }

    private static func writeLine(_ fd: Int32, _ slot: UnsafeMutablePointer<CChar>) {
        _ = write(fd, slot, strlen(slot))
        var newline: CChar = 10
        _ = write(fd, &newline, 1)
    }

    /// What the last run was drawing when it died.
    public struct Crash: Sendable, Equatable {
        public let space: String?
        public let pane: String?
        /// The crash report macOS wrote for it, when one is there yet.
        public let report: URL?

        /// Read and delete the file `install` was given; nil when the last run ended cleanly or
        /// died while drawing neither a space nor a pane.
        public static func read(file: URL, reports: URL = defaultReports) -> Crash? {
            guard let data = try? Data(contentsOf: file), let text = String(data: data, encoding: .utf8) else { return nil }
            try? FileManager.default.removeItem(at: file)
            let marks = text.split(separator: "\n").map(String.init)
            let space = marks.first { $0.hasPrefix("space:") }.map { String($0.dropFirst("space:".count)) }
            let pane = marks.first { $0.hasPrefix("pane:") }.map { String($0.dropFirst("pane:".count)) }
            guard space != nil || pane != nil else { return nil }
            return Crash(space: space, pane: pane, report: newestReport(in: reports))
        }

        public static let defaultReports = URL(fileURLWithPath: NSHomeDirectory())
            .appending(path: "Library/Logs/DiagnosticReports")

        private static func newestReport(in dir: URL) -> URL? {
            let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
            let reports = names.filter { $0.hasPrefix("Wheel-") && $0.hasSuffix(".ips") }.map { dir.appending(path: $0) }
            func modified(_ url: URL) -> Date {
                (try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast
            }
            return reports.max { modified($0) < modified($1) }
        }
    }
}

/// The spaces and panes the last crash named, held out of the grid until the person opens them.
@MainActor @Observable
public final class Quarantine {
    public static let shared = Quarantine()

    public private(set) var spaces: Set<String> = []
    public private(set) var panes: Set<String> = []
    public private(set) var report: URL?

    public init() {}

    /// Hold out what `crash` names: the pane when it names one, else the space.
    public func hold(_ crash: CrashGuard.Crash) {
        report = crash.report
        if let pane = crash.pane { panes.insert(pane) } else if let space = crash.space { spaces.insert(space) }
    }

    public func isolates(space id: String) -> Bool { spaces.contains(id) }

    public func isolates(pane id: String) -> Bool { panes.contains(id) }

    public func release(space id: String) { spaces.remove(id) }

    public func release(pane id: String) { panes.remove(id) }
}
