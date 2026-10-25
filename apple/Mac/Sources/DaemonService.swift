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
    /// What the last registration or restart did, for `/state` and the log.
    var status = "not registered yet"

    private init(label: String, instance: String, listen: String) {
        self.label = label
        self.instance = instance
        self.listen = listen
    }

    /// The daemon this bundle carries; nil for a build without the embed phase.
    static func bundled() -> DaemonService? {
        guard let info = bundledInfo(),
              let label = info["Label"] as? String,
              let instance = info["Instance"] as? String,
              let listen = info["Listen"] as? String
        else { return nil }
        return DaemonService(label: label, instance: instance, listen: listen)
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
    /// register it again; one that only rebuilds wheeld is handled by `restartIfOlder`.
    private var agentPlist: Data? {
        try? Data(contentsOf: Bundle.main.bundleURL.appending(path: "Contents/Library/LaunchAgents/\(label).plist"))
    }
    private var registeredKey: String { "registeredAgent.\(label)" }

    /// Register the agent unless macOS already runs this definition of it. A person who switched
    /// it off in Login Items is sent there, since only they can switch it back on.
    func register() {
        let current = agentPlist
        if agent.status == .enabled, current != UserDefaults.standard.data(forKey: registeredKey) {
            do {
                try agent.unregister()
                Log.info("daemon \(label): its launch agent changed; registering it again")
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

    /// Restarts so far this launch; a daemon that is still wrong after one is registered again, and
    /// after that left alone rather than restarted in a loop.
    private var restarts = 0

    /// Restart the daemon when it runs a different wheeld from the one in this bundle: launchd keeps
    /// running the old binary after a deploy replaces the app, and it can keep running one from the
    /// staging folder the deploy moved the old app into, which no longer exists. `exe` is the path
    /// the daemon reports running from; its start time is the fallback when it reports none.
    /// Returns whether it restarted.
    func restartIfOlder(uptimeSecs: UInt64, exe: String?) -> Bool {
        let binary = Bundle.main.bundleURL.appending(path: "Contents/MacOS/wheeld")
        let started = Date().addingTimeInterval(-Double(uptimeSecs))
        let built = (try? FileManager.default.attributesOfItem(atPath: binary.path))?[.modificationDate] as? Date
        let builtText = built.map { "\($0)" } ?? "unknown"
        let why: String
        if let exe, !exe.isEmpty, URL(filePath: exe).resolvingSymlinksInPath().path != binary.resolvingSymlinksInPath().path {
            why = "daemon runs \(exe), this bundle carries \(binary.path)"
        } else if let built, built > started.addingTimeInterval(2) {
            why = "daemon started \(started), bundled wheeld built \(built)"
        } else {
            // A deploy that did not rebuild wheeld leaves the running daemon alone: a restart ends every agent session.
            status = "kept: daemon runs \(exe ?? "an unreported path"), started \(started); bundled wheeld built \(builtText)"
            Log.info("daemon \(label): \(status)")
            return false
        }
        guard restarts < 2 else {
            status = "still wrong after \(restarts) restarts: \(why)"
            Log.info("daemon \(label): \(status)")
            return false
        }
        restarts += 1
        if restarts == 2 {
            // A plain restart brought the same program back: launchd holds the old path, so register again.
            try? agent.unregister()
            do {
                try agent.register()
                status = "registered again: \(why)"
            } catch {
                status = "register again failed: \(error.localizedDescription); \(why)"
                Log.info("daemon \(label): \(status)")
                return false
            }
            Log.info("daemon \(label): \(status)")
            return true
        }
        let restart = Process()
        restart.executableURL = URL(filePath: "/bin/launchctl")
        restart.arguments = ["kickstart", "-k", "gui/\(getuid())/\(label)"]
        do {
            try restart.run()
            restart.waitUntilExit()
            status = "restarted: \(why); launchctl exit \(restart.terminationStatus)"
        } catch {
            status = "restart failed: \(error.localizedDescription)"
            Log.info("daemon \(label): \(status)")
            return false
        }
        Log.info("daemon \(label): \(status)")
        return true
    }
}
