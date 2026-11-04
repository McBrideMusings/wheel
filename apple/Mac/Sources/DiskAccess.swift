import AppKit
import SwiftUI
import WheelKit
import WheelUI

/// The Full Disk Access check. macOS charges what a terminal or agent reads to the program that
/// started it: Wheel.app for itself, and wheeld, which launchd starts as its own program, for
/// everything it spawns. So both need the grant, and a grant to one covers neither the other nor
/// a process that is already running (a running wheeld keeps the access it started with).
extension AppModel {
    /// The app bundle this app runs from.
    static var appPath: String { Bundle.main.bundlePath }

    /// Whether the running server can open every guarded folder, as it reported; nil while not connected.
    var serverDiskAccess: Bool? {
        guard connection == .connected, let host = serverHost else { return nil }
        return host.folders.allSatisfy(\.allowed)
    }

    /// The screen shows while this app or the server lacks access, until the person closes it; it
    /// leaves by itself when both can open the folders. A server not connected yet counts as
    /// unknown, not as denied, so the app's own state alone can show it.
    var diskAccessScreenShown: Bool {
        guard !diskAccessScreenDismissed, let app = appDiskAccess else { return false }
        return !app || serverDiskAccess == false
    }

    func dismissDiskAccessScreen() {
        diskAccessScreenDismissed = true
        Log.info("full disk access screen closed; app \(describe(appDiskAccess)), server \(describe(serverDiskAccess))")
    }

    private func describe(_ access: Bool?) -> String {
        access.map { $0 ? "granted" : "not granted" } ?? "unknown"
    }

    /// The folders to look at: the ones the server reports, else the ones it checks by default.
    private var guardedNames: [String] {
        serverHost?.folders.map { URL(filePath: $0.path).lastPathComponent } ?? ["Downloads", "Desktop", "Documents"]
    }

    /// Looks at the guarded folders from this app. A folder that is not there is not a denial.
    func checkDiskAccess() async {
        let names = guardedNames
        let granted = await Task.detached { Self.appCanOpen(names) }.value
        if granted != appDiskAccess {
            Log.info("full disk access: app \(describe(granted)) at \(Self.appPath)")
            appDiskAccess = granted
        }
        if diskAccessScreenShown { keepWatchingDiskAccess() }
        await lookForGrantSinceStart()
    }

    /// While the server cannot open the folders, asks the system log whether macOS recorded a
    /// change to the server's Full Disk Access after it started. The look takes about 5 s, so it
    /// runs at most once every 10 s, and the answer clears when the server can open them.
    private func lookForGrantSinceStart() async {
        guard serverDiskAccess == false, let exe = serverHost?.exe, let started = serverStarted else {
            if diskAccessChangedSinceStart { diskAccessChangedSinceStart = false }
            return
        }
        if diskAccessChangedSinceStart { return }
        if let last = lastGrantLook, Date().timeIntervalSince(last) < 10 { return }
        lastGrantLook = Date()
        let changed = await Task.detached { Self.accessChanged(path: exe, since: started) }.value
        if changed != diskAccessChangedSinceStart {
            Log.info("full disk access: \(exe) changed since the server started \(started): \(changed)")
            diskAccessChangedSinceStart = changed
        }
    }

    /// Whether tccd logged a Full Disk Access change for the program at `path` after `since`.
    nonisolated static func accessChanged(path: String, since: Date) -> Bool {
        let format = DateFormatter()
        format.locale = Locale(identifier: "en_US_POSIX")
        format.dateFormat = "yyyy-MM-dd HH:mm:ss"
        let process = Process()
        process.executableURL = URL(filePath: "/usr/bin/log")
        process.arguments = [
            "show", "--start", format.string(from: since), "--style", "compact",
            "--predicate",
            "subsystem == \"com.apple.TCC\" AND eventMessage CONTAINS \"kTCCServiceSystemPolicyAllFiles\" AND eventMessage CONTAINS \"type=Modify\" AND eventMessage CONTAINS \"\(path)\"",
        ]
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        guard (try? process.run()) != nil else { return false }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return String(decoding: data, as: UTF8.self).contains("type=Modify")
    }

    nonisolated static func appCanOpen(_ names: [String]) -> Bool {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return names.allSatisfy { name in
            guard let dir = opendir(home.appending(path: name).path) else {
                return !(errno == EPERM || errno == EACCES)
            }
            closedir(dir)
            return true
        }
    }

    /// Looks once at launch, and again whenever the app becomes active (a grant is made in another
    /// app). The model owns this, not a window, so closing a window never ends it.
    func startDiskAccessCheck() {
        guard diskAccessObserver == nil else { return }
        diskAccessObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in await self?.checkDiskAccess() }
        }
        Task { await checkDiskAccess() }
    }

    /// While the screen is up, looks every 2 s so a grant shows when it lands; the loop ends when
    /// the screen goes.
    private func keepWatchingDiskAccess() {
        guard diskAccessTask == nil else { return }
        diskAccessTask = Task { @MainActor in
            while diskAccessScreenShown {
                try? await Task.sleep(for: .seconds(2))
                await checkDiskAccess()
            }
            diskAccessTask = nil
        }
    }

    /// Opens System Settings at Full Disk Access and selects the file in Finder, to drag into the
    /// list. macOS lets no program grant or request this itself.
    func offerDiskAccess(path: String) {
        Log.info("full disk access: opening the pane and revealing \(path)")
        FullDiskAccess.openPane()
        FullDiskAccess.reveal(path)
    }
}

/// What the buttons do; macOS lets no program grant or request Full Disk Access itself.
enum FullDiskAccess {
    /// The Full Disk Access pane of System Settings.
    static func openPane() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles") {
            NSWorkspace.shared.open(url)
        }
    }

    /// Puts a path on the pasteboard, for the file chooser behind the pane's + button.
    static func copyPath(_ path: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(path, forType: .string)
    }

    /// Selects the file in Finder, to drag into the pane's list.
    static func reveal(_ path: String) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(filePath: path)])
    }
}

/// The sheet that says which of Wheel.app and wheeld can open the folders macOS guards, each with
/// its exact path, and offers the missing ones.
struct DiskAccessScreen: View {
    var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Wheel needs Full Disk Access").font(.title3).fontWeight(.semibold)
            Text("Terminals and agents in Wheel get “Operation not permitted” in the folders macOS guards until both programs below have it. macOS lets no program grant it itself: the button opens System Settings at Privacy & Security › Full Disk Access and shows the program in Finder. Drag it into the list, or press + and paste its path.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            DiskAccessRow(
                title: "Wheel app", path: AppModel.appPath, granted: model.appDiskAccess,
                missingNote: "If this stays not granted after you add it, quit Wheel and open it again.",
                offer: { model.offerDiskAccess(path: AppModel.appPath) })
            if model.serverHost == nil || model.connection != .connected {
                DiskAccessRow(
                    title: "Wheel server (wheeld)", path: "Waiting for the server to connect", granted: nil,
                    missingNote: "", offer: {})
            } else if let exe = model.serverHost?.exe {
                DiskAccessRow(
                    title: "Wheel server (wheeld)", path: exe, granted: model.serverDiskAccess,
                    missingNote: model.diskAccessChangedSinceStart
                        ? "Access was changed after the server started, and a running server keeps the access it started with. Restart the server to apply it. That ends every terminal and agent."
                        : "A running server keeps the access it started with, so after you add it, restart the server. That ends every terminal and agent.",
                    offer: { model.offerDiskAccess(path: exe) },
                    restart: model.diskAccessChangedSinceStart && model.serverControl?.unavailable == nil
                        ? { model.restartServer() } : nil)
            }
            HStack {
                Spacer()
                Button("Not Now") { model.dismissDiskAccessScreen() }.keyboardShortcut(.cancelAction)
            }
        }
        .padding(20)
        .frame(width: 560)
    }
}

private struct DiskAccessRow: View {
    let title: String
    let path: String
    let granted: Bool?
    let missingNote: String
    let offer: () -> Void
    var restart: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Image(systemName: granted == true ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                    .foregroundStyle(granted == true ? Color.green : Color.orange)
                Text(title).fontWeight(.semibold)
                Text(granted == true ? "Granted" : granted == false ? "Not granted" : "Checking…")
                    .foregroundStyle(.secondary)
            }
            Text(path).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
            if granted == false {
                Text(missingNote).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                HStack {
                    if let restart {
                        Button("Restart Server", action: restart).buttonStyle(.borderedProminent)
                            .help("Restart the server; every terminal and agent ends")
                        Button("Open Full Disk Access and Show in Finder", action: offer)
                    } else {
                        Button("Open Full Disk Access and Show in Finder", action: offer).buttonStyle(.borderedProminent)
                    }
                    Button("Copy Path") { FullDiskAccess.copyPath(path) }
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
    }
}
