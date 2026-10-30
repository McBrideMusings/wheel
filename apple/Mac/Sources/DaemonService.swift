import Foundation
import ServiceManagement

/// The wheeld this app carries (`Mac/embed-wheeld.sh` puts it in the bundle): which daemon instance
/// it is, where it listens, and the launch agent that runs it. The app registers that agent with
/// SMAppService, so installing the app installs its daemon and deleting the app removes it.
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

    var agent: SMAppService { .agent(plistName: "\(label).plist") }

    /// The bundled launch agent's bytes, as registered last. launchd keeps the definition it was
    /// given at registration, so a deploy that changes the agent (its port, its arguments) must
    /// register it again. One that only rebuilds wheeld leaves the running daemon alone: the app
    /// offers a restart when the bundled source differs (ADR-0020).
    private var agentPlist: Data? {
        try? Data(contentsOf: Bundle.main.bundleURL.appending(path: "Contents/Library/LaunchAgents/\(label).plist"))
    }
    private var registeredKey: String { "registeredAgent.\(label)" }

    /// Whether launchd holds a job for the agent now (`launchctl print` exits 0).
    private var loaded: Bool {
        let print = Process()
        print.executableURL = URL(filePath: "/bin/launchctl")
        print.arguments = ["print", "gui/\(getuid())/\(label)"]
        print.standardOutput = FileHandle.nullDevice
        print.standardError = FileHandle.nullDevice
        guard (try? print.run()) != nil else { return true }
        print.waitUntilExit()
        return print.terminationStatus == 0
    }

    /// Register the agent unless macOS already runs this definition of it. A person who switched
    /// it off in Login Items is sent there, since only they can switch it back on.
    func register() {
        let current = agentPlist
        // `launchctl bootout` leaves the agent enabled in Login Items with no job in launchd, so
        // nothing starts the daemon until the agent is registered again.
        let why = current != UserDefaults.standard.data(forKey: registeredKey) ? "its launch agent changed"
            : !loaded ? "launchd has no job for it" : nil
        if agent.status == .enabled, let why {
            do {
                try agent.unregister()
                Log.info("daemon \(label): \(why); registering it again")
            } catch {
                Log.info("daemon \(label): unregister failed: \(error.localizedDescription)")
            }
        }
        defer {
            if agent.status == .enabled { UserDefaults.standard.set(current, forKey: registeredKey) }
        }
        switch agent.status {
        case .enabled:
            status = "enabled"
        case .requiresApproval:
            status = "waiting for approval in System Settings > General > Login Items"
            SMAppService.openSystemSettingsLoginItems()
        case .notRegistered, .notFound:
            do {
                try agent.register()
                status = agent.status == .enabled ? "registered" : "registered; status \(agent.status.rawValue)"
            } catch {
                status = "register failed: \(error.localizedDescription)"
            }
        @unknown default:
            status = "unknown status \(agent.status.rawValue)"
        }
        Log.info("daemon \(label) (\(instance) on \(listen)): \(status)")
    }

    /// Restart the daemon from this bundle: launchd ends the running wheeld and starts the program
    /// the agent names. It ends every terminal and agent session, so only the person asks for it.
    func restart() async -> String? {
        let label = label
        let failure: String? = await withCheckedContinuation { continuation in
            DispatchQueue.global().async {
                let restart = Process()
                restart.executableURL = URL(filePath: "/bin/launchctl")
                restart.arguments = ["kickstart", "-k", "gui/\(getuid())/\(label)"]
                do {
                    try restart.run()
                    restart.waitUntilExit()
                    continuation.resume(returning: restart.terminationStatus == 0 ? nil : "launchctl kickstart exited \(restart.terminationStatus)")
                } catch {
                    continuation.resume(returning: error.localizedDescription)
                }
            }
        }
        status = failure.map { "restart failed: \($0)" } ?? "restarted from the app"
        Log.info("daemon \(label): \(status)")
        return failure
    }
}
