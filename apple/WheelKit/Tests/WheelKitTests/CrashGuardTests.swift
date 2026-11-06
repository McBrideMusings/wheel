import Foundation
import Testing
@testable import WheelKit

@Suite(.serialized) struct CrashGuardTests {
    /// The test runner delivers a raised signal to another thread a few milliseconds later; a real
    /// trap is synchronous. Wait for the handler's file.
    private func raiseAndWait(_ sig: Int32, for file: URL) {
        raise(sig)
        for _ in 0 ..< 200 where !FileManager.default.fileExists(atPath: file.path) { usleep(10_000) }
    }

    private func scratch() -> URL {
        FileManager.default.temporaryDirectory.appending(path: "crashguard-\(UUID().uuidString)")
    }

    /// A signal the handler is installed for writes what was marked when it arrived, and the next
    /// launch reads it once and finds the file gone.
    @Test func aSignalWritesTheMarkedSpaceAndPaneForTheNextLaunch() throws {
        let file = scratch(), reports = scratch()
        try FileManager.default.createDirectory(at: reports, withIntermediateDirectories: true)
        let report = reports.appending(path: "Wheel-2026-11-06-084433.ips")
        try Data("{}".utf8).write(to: report)
        CrashGuard.reraises = false
        CrashGuard.install(file: file, signals: [SIGUSR2])
        CrashGuard.mark(space: "s4371")
        CrashGuard.mark(pane: "p4854")
        raiseAndWait(SIGUSR2, for: file)

        let crash = try #require(CrashGuard.Crash.read(file: file, reports: reports))
        #expect(crash == CrashGuard.Crash(space: "s4371", pane: "p4854", report: report))
        #expect(!FileManager.default.fileExists(atPath: file.path))
        #expect(CrashGuard.Crash.read(file: file, reports: reports) == nil)
        CrashGuard.clear()
    }

    /// Nothing marked, nothing to quarantine: a crash outside any view build names no one.
    @Test func aCrashWithNothingMarkedQuarantinesNothing() {
        let file = scratch()
        CrashGuard.reraises = false
        CrashGuard.install(file: file, signals: [SIGUSR2])
        raiseAndWait(SIGUSR2, for: file)
        #expect(FileManager.default.fileExists(atPath: file.path))
        #expect(CrashGuard.Crash.read(file: file, reports: scratch()) == nil)
    }

    @Test func aRunThatEndsCleanlyLeavesNoFile() {
        let file = scratch()
        CrashGuard.install(file: file, signals: [SIGUSR2])
        CrashGuard.mark(space: "s1")
        #expect(CrashGuard.Crash.read(file: file, reports: scratch()) == nil)
    }

    @Test func aMarkLongerThanTheBufferIsCutNotOverrun() throws {
        let file = scratch()
        CrashGuard.reraises = false
        CrashGuard.install(file: file, signals: [SIGUSR2])
        CrashGuard.mark(pane: String(repeating: "p", count: 1000))
        raiseAndWait(SIGUSR2, for: file)
        let crash = try #require(CrashGuard.Crash.read(file: file, reports: scratch()))
        #expect(crash.pane?.count == 159 - "pane:".count)
        CrashGuard.clear()
    }

    /// A pane marked while space A was built must not be named when space B's layout crashes.
    @Test func aNewSpacesMarkDropsThePreviousSpacesPane() throws {
        let file = scratch()
        CrashGuard.reraises = false
        CrashGuard.install(file: file, signals: [SIGUSR2])
        CrashGuard.mark(space: "sA")
        CrashGuard.mark(pane: "pA")
        CrashGuard.mark(space: "sB")
        raiseAndWait(SIGUSR2, for: file)
        let crash = try #require(CrashGuard.Crash.read(file: file, reports: scratch()))
        #expect(crash.space == "sB" && crash.pane == nil)
        CrashGuard.clear()
    }

    @MainActor @Test func theQuarantineHoldsTheNamedPaneOrElseTheSpaceUntilReleased() {
        let quarantine = Quarantine()
        quarantine.hold(.init(space: "s1", pane: "p2", report: nil))
        #expect(quarantine.isolates(pane: "p2") && !quarantine.isolates(space: "s1"))
        quarantine.hold(.init(space: "s3", pane: nil, report: nil))
        #expect(quarantine.isolates(space: "s3"))
        quarantine.release(pane: "p2")
        quarantine.release(space: "s3")
        #expect(!quarantine.isolates(pane: "p2") && !quarantine.isolates(space: "s3"))
    }
}
