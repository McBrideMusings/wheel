import BenchCore
import SwiftUI
import UIKit

// Launched with `-spec <engine:model>`; runs the bench on the bundled clips and writes Documents/<spec>.json.
@main
struct BenchApp: App {
    @State private var lines: [String] = []
    var body: some Scene {
        WindowGroup {
            ScrollView { Text(lines.suffix(40).joined(separator: "\n")).font(.caption.monospaced()).frame(maxWidth: .infinity, alignment: .leading) }
                .padding()
                .task { await go() }
        }
    }

    @MainActor func go() async {
        guard let spec = UserDefaults.standard.string(forKey: "spec") else { lines = ["no -spec"]; return }
        UIApplication.shared.isIdleTimerDisabled = true
        UIDevice.current.isBatteryMonitoringEnabled = true
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let res = Bundle.main.resourceURL!
        let name = spec.replacingOccurrences(of: ":", with: "_").replacingOccurrences(of: "/", with: "_")
        let tag = UserDefaults.standard.string(forKey: "tag") ?? "pass"
        let out = docs.appendingPathComponent("\(name).\(tag).json")
        let log = docs.appendingPathComponent("\(name).\(tag).log")
        FileManager.default.createFile(atPath: log.path, contents: nil)
        let h = try? FileHandle(forWritingTo: log)
        let say: @Sendable (String) -> Void = { s in
            h?.write((s + "\n").data(using: .utf8)!)
            Task { @MainActor in lines.append(s) }
        }
        say("battery \(UIDevice.current.batteryLevel) state \(UIDevice.current.batteryState.rawValue) thermal \(ProcessInfo.processInfo.thermalState.rawValue)")
        do {
            try await run(spec: spec, clipsDir: res, modelsDir: res, cacheDir: docs, out: out, log: say)
            say("DONE")
        } catch {
            say("ERROR \(error)")
        }
        try? h?.close()
    }
}
