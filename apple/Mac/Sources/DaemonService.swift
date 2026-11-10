import Foundation
import ServiceManagement

/// The wheeld this app carries (`Mac/embed-wheeld.sh` puts it in the bundle): which daemon instance
/// it is, where it listens, and the launch agent that runs it. The app copies the server out of
/// its bundle and loads that agent itself (`Mac/wheel-server.sh`), so macOS sees Wheel Server as
/// its own program and not as part of Wheel.
@MainActor
final class DaemonService {
    let label: String
    let instance: String
    let listen: String
    /// The hash of the source the bundled wheeld was built from (`wheeld --source-hash`, written
    /// by the embed phase); nil for a bundle built before it was written.
    let bundledSource: String?
    /// What the last registration or restart did, for `/state` and the log.
    var status = "not registered yet"

    private init(label: String, instance: String, listen: String, bundledSource: String?) {
        self.label = label
        self.instance = instance
        self.listen = listen
        self.bundledSource = bundledSource
    }

    /// The daemon this bundle carries; nil for a build without the embed phase.
    static func bundled() -> DaemonService? {
        guard let info = bundledInfo(),
              let label = info["Label"] as? String,
              let instance = info["Instance"] as? String,
              let listen = info["Listen"] as? String
        else { return nil }
        return DaemonService(label: label, instance: instance, listen: listen, bundledSource: info["SourceHash"] as? String)
    }

    /// The bundled daemon's instance name (`wheel`, `wheel-dev`), from any thread.
    nonisolated static var bundledInstance: String? { bundledInfo()?["Instance"] as? String }

    private nonisolated static func bundledInfo() -> [String: Any]? {
        Bundle.main.url(forResource: "WheelDaemon", withExtension: "plist").flatMap { NSDictionary(contentsOf: $0) as? [String: Any] }
    }

    var url: URL { URL(string: "ws://\(listen)/ws")! }

    var tokenFile: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appending(path: "Library/Application Support/\(instance)/token")
    }

    /// The script that copies the bundled server out of the app and runs it as its own launch
    /// agent (`Mac/wheel-server.sh`). The same script serves `tools/restart-server.sh`.
    private var script: URL? { Bundle.main.url(forResource: "wheel-server", withExtension: "sh") }

    /// Runs one verb of the script off the main thread; returns its output, or the failure.
    private func run(_ verb: String) async -> (ok: Bool, line: String) {
        guard let script else { return (false, "wheel-server.sh is not in this app") }
        return await withCheckedContinuation { continuation in
            DispatchQueue.global().async {
                let process = Process()
                process.executableURL = URL(filePath: "/bin/bash")
                process.arguments = [script.path, verb]
                let out = Pipe()
                process.standardOutput = out
                process.standardError = out
                do {
                    try process.run()
                    let data = out.fileHandleForReading.readDataToEndOfFile()
                    process.waitUntilExit()
                    let text = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
                    continuation.resume(returning: (process.terminationStatus == 0, text))
                } catch {
                    continuation.resume(returning: (false, error.localizedDescription))
                }
            }
        }
    }

    /// Copy this app's server into Application Support and load its launch agent unless launchd
    /// already runs one. A running server is never touched: a newer one waits for the owner's
    /// restart (ADR-0020).
    func register() async {
        let result = await run("start")
        status = result.ok ? result.line.split(separator: "\n").last.map(String.init) ?? "started" : "start failed: \(result.line)"
        Log.info("daemon \(label) (\(instance) on \(listen)): \(result.line)")
    }

    /// Run this app's server now: it ends the running wheeld and every terminal and agent, so only
    /// the person asks for it. A server that an earlier app registered through SMAppService from
    /// inside its bundle is unregistered first, so its login item does not outlive it.
    func restart() async -> String? {
        let legacy = SMAppService.agent(plistName: "\(label).plist")
        do {
            try await legacy.unregister()
            Log.info("daemon \(label): unregistered the login item an earlier app made")
        } catch {
            Log.info("daemon \(label): no earlier login item to unregister: \(error.localizedDescription)")
        }
        let result = await run("restart")
        status = result.ok ? "restarted from the app" : "restart failed: \(result.line)"
        Log.info("daemon \(label): \(result.line)")
        return result.ok ? nil : result.line
    }

    /// The version `rollback` would run: the script's answer when the agent is loaded and an
    /// older version than Current is kept; nil, with the script's reason logged, when not.
    func rollbackTarget() async -> String? {
        let result = await run("rollbackable")
        Log.info("daemon \(label): rollbackable: \(result.ok ? result.line : "no (\(result.line))")")
        return result.ok ? result.line : nil
    }

    /// Put the older version back as Current and run it.
    func rollback() async -> String? {
        let result = await run("rollback")
        status = result.ok ? "rolled back from the app" : "rollback failed: \(result.line)"
        Log.info("daemon \(label): \(result.line)")
        return result.ok ? nil : result.line
    }

    /// End the server and keep launchd from loading it again.
    func stop() async -> String? {
        let result = await run("stop")
        status = result.ok ? "stopped from the menu" : "stop failed: \(result.line)"
        Log.info("daemon \(label): \(result.line)")
        return result.ok ? nil : result.line
    }

    /// Load the agent again, which starts the server.
    func start() async -> String? {
        await register()
        return status.hasPrefix("start failed") ? status : nil
    }
}
