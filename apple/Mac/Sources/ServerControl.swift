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
}

@MainActor
struct LaunchAgentControl: ServerControl {
    let service: DaemonService
    var unavailable: String? { nil }

    func stop() async -> String? { await service.stop() }
    func start() async -> String? { service.start() }
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
    /// The header's dot and word. The dot's colour is never the only signal: the word says the same.
    var serverStatus: ServerStatus {
        if serverStoppedByMenu { return ServerStatus(word: "Stopped", color: .gray) }
        switch connection {
        case .connecting: return ServerStatus(word: "Starting…", color: .yellow)
        case .unreachable: return ServerStatus(word: "Stopped", color: .gray)
        case .connected:
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
