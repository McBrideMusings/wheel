import Foundation
import ServiceManagement
import SwiftUI
import WheelKit

/// How the app stops and starts the server it talks to. The server the app carries runs as a
/// launch agent registered through SMAppService: unregistering it stops the process and keeps
/// launchd from starting it again, registering it brings it back. A server the app did not
/// install (`WHEEL_URL` names another one) is stopped and started by the commands in
/// `WHEEL_SERVER_STOP` and `WHEEL_SERVER_START`, or not at all.
@MainActor
protocol ServerControl {
    /// Why the app cannot stop and start this server; nil when it can.
    var unavailable: String? { get }
    func stop() async -> String?
    func start() async -> String?
    func restart() async -> String?
}

@MainActor
struct LaunchAgentControl: ServerControl {
    let service: DaemonService
    var unavailable: String? { nil }

    func stop() async -> String? { await service.stop() }
    func start() async -> String? { service.start() }
    func restart() async -> String? { await service.restart() }
}

@MainActor
struct CommandControl: ServerControl {
    let stopCommand: String?
    let startCommand: String?

    var unavailable: String? {
        stopCommand == nil && startCommand == nil ? "This server was started by something other than Wheel, so Wheel cannot stop it." : nil
    }

    private func run(_ command: String?) async -> String? {
        guard let command else { return "No command is set for this." }
        return await withCheckedContinuation { continuation in
            DispatchQueue.global().async {
                let process = Process()
                process.executableURL = URL(filePath: "/bin/sh")
                process.arguments = ["-c", command]
                do {
                    try process.run()
                    process.waitUntilExit()
                    continuation.resume(returning: process.terminationStatus == 0 ? nil : "The command exited with \(process.terminationStatus).")
                } catch {
                    continuation.resume(returning: error.localizedDescription)
                }
            }
        }
    }

    func stop() async -> String? { await run(stopCommand) }
    func start() async -> String? { await run(startCommand) }
    func restart() async -> String? {
        if let failure = await stop() { return failure }
        return await start()
    }
}

extension DaemonService {
    /// End the server and stop launchd restarting it: the agent is unregistered, not killed.
    func stop() async -> String? {
        do {
            try await agent.unregister()
            status = "stopped from the menu"
            Log.info("daemon \(label): unregistered; launchd no longer keeps it running")
            return nil
        } catch {
            Log.info("daemon \(label): stop failed: \(error.localizedDescription)")
            return error.localizedDescription
        }
    }

    /// Register the agent again, which starts the server.
    func start() -> String? {
        register()
        return status.hasPrefix("register failed") || status.hasPrefix("waiting for approval") ? status : nil
    }
}

/// The server as the menu bar header reports it.
struct ServerStatus: Equatable {
    let word: String
    let color: Color
}

extension AppModel {
    /// The running server's source hash and the bundled one's, when the app carries its server and
    /// is connected to it; nil otherwise.
    var serverSources: (running: String?, bundled: String)? {
        guard connection == .connected, let bundled = service?.bundledSource, let host = serverHost else { return nil }
        return (host.source, bundled)
    }

    /// What the banner, the header and Settings say while a restart waits.
    static let serverUpdatedText = "The server was updated."

    /// A newer server is installed and waits for a restart: the bundled source differs from the
    /// running one, or the running one lacks a protocol feature this app has. The app never
    /// restarts it itself (ADR-0020); the menu bar dot, the banner and Restart Server follow this.
    var serverRestartWaiting: Bool {
        guard connection == .connected else { return false }
        if let sources = serverSources, sources.running != sources.bundled { return true }
        return serverProtocol < ServerProtocol.current
    }

    /// What a dismissed banner was about: the bundled source, or the protocol when there is none.
    private var serverBannerKey: String { service?.bundledSource ?? "protocol \(serverProtocol)" }

    /// The window's banner shows while a restart waits, until the person closes it for this server.
    var serverBannerShown: Bool { serverRestartWaiting && serverBannerDismissed != serverBannerKey }

    func dismissServerBanner() {
        serverBannerDismissed = serverBannerKey
        Prefs.store.set(serverBannerKey, forKey: "serverBannerDismissed")
        Log.info("server banner dismissed for \(serverBannerKey)")
    }

    /// One log line per connection: the server's protocol, and whether the running server is the
    /// bundled one.
    func logServerSource() {
        Log.info("server protocol \(serverProtocol), this app's \(ServerProtocol.current)")
        guard let service else { return }
        let running = serverHost?.source ?? "none reported"
        let bundled = service.bundledSource ?? "none recorded"
        Log.info("daemon \(service.label): running source \(running), bundled source \(bundled): \(serverRestartWaiting ? "different; restart offered" : "same; nothing to offer")")
    }

    /// The header's dot and word. The dot's colour is never the only signal: the word says the same.
    var serverStatus: ServerStatus {
        if serverStoppedByMenu { return ServerStatus(word: "Stopped", color: .gray) }
        switch connection {
        case .connecting: return ServerStatus(word: "Starting…", color: .yellow)
        case .unreachable: return ServerStatus(word: "Stopped", color: .gray)
        case .connected:
            if serverRestartWaiting { return ServerStatus(word: "Running · restart to update", color: .orange) }
            guard let info = settings.devices.info else { return ServerStatus(word: "Running", color: .green) }
            if info.problem != nil { return ServerStatus(word: "Remote listener failed", color: .orange) }
            if info.listen == nil { return ServerStatus(word: "Running · remote access off", color: .green) }
            return ServerStatus(word: "Running", color: .green)
        }
    }

    /// The lines under the header while it is open.
    var serverDetails: [String] {
        let count = spaces.count
        let spaceText = "\(count) \(count == 1 ? "space" : "spaces")"
        if serverStoppedByMenu {
            return ["Stopped from this menu. Terminals and agents in \(spaceText) are not running."]
        }
        guard connection == .connected else {
            if case let .unreachable(reason) = connection { return ["wheeld unreachable: \(reason)"] }
            return ["Connecting to wheeld…"]
        }
        var lines: [String] = []
        if serverRestartWaiting { lines.append(Self.serverUpdatedText) }
        if let started = serverStarted {
            let up = Duration.seconds(Int(Date().timeIntervalSince(started)))
            lines.append("wheeld up \(up.formatted(.units(allowed: [.days, .hours, .minutes], width: .wide, maximumUnitCount: 2))) · \(spaceText)")
        }
        if let info = settings.devices.info {
            if let problem = info.problem {
                lines.append("Remote listener failed: \(problem)")
            } else if let address = settings.devices.address, info.listen != nil {
                lines.append("Listening on \(address):\(info.port)")
            } else {
                lines.append("Remote access is off")
            }
        }
        let paired = settings.devices.paired
        lines.append(paired.isEmpty ? "No paired devices" : "\(paired.count) paired \(paired.count == 1 ? "device" : "devices"): \(paired.map(\.name).joined(separator: ", "))")
        return lines
    }

    /// Ask first, then stop: the confirmation sits in the header's details.
    func askToStopServer() { serverConfirming = true }
    func cancelStopServer() { serverConfirming = false }

    func stopServer() {
        serverConfirming = false
        guard let control = serverControl, control.unavailable == nil else { return }
        Log.info("server: stop asked for; \(spaces.count) spaces")
        Task {
            if let failure = await control.stop() {
                serverProblem = failure
                Log.info("server: stop failed: \(failure)")
                return
            }
            serverProblem = nil
            serverStoppedByMenu = true
            Prefs.store.set(true, forKey: "serverStopped")
            Log.info("server: stopped from the menu")
        }
    }

    /// Restart the server from the bundle, in one click.
    func restartServer() {
        guard let control = serverControl, control.unavailable == nil else { return }
        Log.info("server: restart asked for; \(spaces.count) spaces")
        Task {
            if let failure = await control.restart() {
                serverProblem = failure
                Log.info("server: restart failed: \(failure)")
                return
            }
            serverProblem = nil
            Log.info("server: restarted from the app")
        }
    }

    func startServer() {
        guard let control = serverControl, control.unavailable == nil else { return }
        Log.info("server: start asked for")
        Task {
            if let failure = await control.start() {
                serverProblem = failure
                Log.info("server: start failed: \(failure)")
                return
            }
            serverProblem = nil
            serverStoppedByMenu = false
            Prefs.store.set(false, forKey: "serverStopped")
            Log.info("server: started from the menu")
        }
    }
}
