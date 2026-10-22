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
        guard let url = Bundle.main.url(forResource: "WheelDaemon", withExtension: "plist"),
              let info = NSDictionary(contentsOf: url) as? [String: Any],
              let label = info["Label"] as? String,
              let instance = info["Instance"] as? String,
              let listen = info["Listen"] as? String
        else { return nil }
        return DaemonService(label: label, instance: instance, listen: listen)
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

    /// Restart the daemon when it started before the wheeld in this bundle was built: launchd keeps
    /// running the old binary after a deploy replaces the app. Returns whether it restarted.
    func restartIfOlder(uptimeSecs: UInt64) -> Bool {
        let binary = Bundle.main.bundleURL.appending(path: "Contents/MacOS/wheeld")
        guard let built = (try? FileManager.default.attributesOfItem(atPath: binary.path))?[.modificationDate] as? Date
        else { return false }
        let started = Date().addingTimeInterval(-Double(uptimeSecs))
        guard built > started.addingTimeInterval(2) else { return false }
        let restart = Process()
        restart.executableURL = URL(filePath: "/bin/launchctl")
        restart.arguments = ["kickstart", "-k", "gui/\(getuid())/\(label)"]
        do {
            try restart.run()
            restart.waitUntilExit()
            status = "restarted: daemon started \(started), bundled wheeld built \(built); launchctl exit \(restart.terminationStatus)"
        } catch {
            status = "restart failed: \(error.localizedDescription)"
        }
        Log.info("daemon \(label): \(status)")
        return true
    }
}
