#if os(macOS)
import Foundation
import Testing
@testable import WheelUI

/// One recording through the real model, a spoken clip standing in for the microphone. Needs the
/// model on this Mac (`Dictation.modelFolder`); skipped without it, since fetching 469 MB is not a test's job.
@MainActor @Suite struct DictationTests {
    @Test func aClipBecomesTheTextOfItsOwnConversation() async throws {
        let d = Dictation.shared
        try #require(d.modelOnDisk, "no model at \(Dictation.modelFolder.path)")
        let clip = FileManager.default.temporaryDirectory.appendingPathComponent("dictation-\(UUID().uuidString).wav")
        defer { try? FileManager.default.removeItem(at: clip) }
        let say = Process()
        say.executableURL = URL(fileURLWithPath: "/usr/bin/say")
        say.arguments = ["-o", clip.path, "--data-format=LEI16@16000", "Run cargo test in the daemon."]
        try say.run()
        say.waitUntilExit()
        setenv("WHEEL_DICTATION_FILE", clip.path, 1)
        defer { unsetenv("WHEEL_DICTATION_FILE") }

        let before = d.heard?.id ?? 0
        d.toggle(for: "w1")
        #expect(d.phase == .recording && d.owner == "w1")
        d.toggle(for: "w2")
        #expect(d.owner == "w1", "a second conversation cannot take the recorder")
        // The file is read on a task; stop once it has been.
        try await Task.sleep(for: .milliseconds(300))
        d.toggle(for: "w1")
        #expect(d.phase == .transcribing)
        // The first load compiles the model for the Neural Engine, up to a minute or more on a busy Mac.
        for _ in 0..<3000 where d.phase != .idle { try await Task.sleep(for: .milliseconds(100)) }
        let heard = try #require(d.heard)
        #expect(heard.id == before + 1 && heard.owner == "w1")
        #expect(heard.text.lowercased().contains("cargo test"), "heard \(heard.text)")
        #expect(d.owner == nil && d.problem == nil)
    }
}
#endif
