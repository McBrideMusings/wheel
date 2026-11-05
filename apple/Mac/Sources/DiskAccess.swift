import AppKit
import SwiftUI
import WheelKit
import WheelUI

/// The folder access check. macOS charges what a terminal or agent reads to the program that
/// started it: Wheel Server, which launchd starts as its own program, for everything it spawns.
/// The app opens no guarded folder itself (ADR-0020), so only the server needs a grant, and a
/// grant does not reach a process that is already running a denied folder's listing.
extension AppModel {

    /// Whether the running server can open every guarded folder, as it reported; nil while not connected.
    var serverDiskAccess: Bool? {
        guard connection == .connected, let host = serverHost else { return nil }
        return host.folders.allSatisfy(\.allowed)
    }

    /// The screen shows while the server lacks access, until the person closes it; it leaves by
    /// itself when the server can open the folders. A server not connected yet counts as unknown.
    var diskAccessScreenShown: Bool {
        !diskAccessScreenDismissed && serverDiskAccess == false
    }

    func dismissDiskAccessScreen() {
        diskAccessScreenDismissed = true
        Log.info("folder access screen closed; server \(describe(serverDiskAccess))")
    }

    private func describe(_ access: Bool?) -> String {
        access.map { $0 ? "granted" : "not granted" } ?? "unknown"
    }

    /// Looks at what the server reported and keeps watching while the screen is up.
    func checkDiskAccess() async {
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

    /// Opens System Settings at Files & Folders, where Wheel Server's four folder switches are.
    /// macOS lets no program grant these itself.
    func offerDiskAccess(path: String) {
        Log.info("folder access: opening Files & Folders for \(path)")
        FullDiskAccess.openFolders()
    }
}

/// What the buttons do; macOS lets no program grant or request folder access itself.
enum FullDiskAccess {
    /// The Files & Folders pane of System Settings.
    static func openFolders() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders") {
            NSWorkspace.shared.open(url)
        }
    }

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

/// The sheet that says whether Wheel Server can open the folders macOS guards, with its exact
/// path, and offers the pane that grants them.
struct DiskAccessScreen: View {
    var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Wheel Server needs access to your folders").font(.title3).fontWeight(.semibold)
            Text("Terminals and agents in Wheel get “Operation not permitted” in the folders macOS guards until Wheel Server may open them. macOS asks the first time a folder is used; the button opens System Settings at Privacy & Security › Files & Folders, where Wheel Server has switches for Desktop, Documents, Downloads and network volumes.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if model.serverHost == nil || model.connection != .connected {
                DiskAccessRow(
                    title: "Wheel Server", path: "Waiting for the server to connect", granted: nil,
                    missingNote: "", offer: {})
            } else if let exe = model.serverHost?.exe {
                DiskAccessRow(
                    title: "Wheel Server", path: exe, granted: model.serverDiskAccess,
                    missingNote: model.diskAccessChangedSinceStart
                        ? "Access was changed after the server started, and a running server keeps the access it started with. Restart the server to apply it. That ends every terminal and agent."
                        : "A running server keeps the access it started with, so after you switch a folder on, restart the server. That ends every terminal and agent.",
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
                        Button("Open Files & Folders", action: offer)
                    } else {
                        Button("Open Files & Folders", action: offer).buttonStyle(.borderedProminent)
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
