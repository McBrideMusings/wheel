import AppKit
import SwiftUI
import WheelKit
import WheelUI

/// The Folder access box of the General tab: whether Wheel Server can list Downloads, Desktop and
/// Documents. macOS charges a background daemon's access to Wheel and cannot ask the person, so a
/// folder nobody granted fails with "Operation not permitted" inside every terminal and agent.
struct FolderAccessSettings: View {
    let host: HostAccess?
    /// Access was changed after the server started: the one thing left is a restart.
    var changedSinceStart = false
    var restart: (() -> Void)?

    static func title(_ host: HostAccess) -> String {
        let denied = host.folders.filter { !$0.allowed && !$0.waiting }
        let waiting = host.folders.filter(\.waiting)
        if denied.isEmpty, !waiting.isEmpty {
            let names = waiting.map { URL(filePath: $0.path).lastPathComponent }.joined(separator: ", ")
            return "Wheel Server is waiting for your answer to the macOS prompt for \(names)."
        }
        if denied.isEmpty { return "Wheel Server can open Downloads, Desktop and Documents." }
        let names = denied.map { URL(filePath: $0.path).lastPathComponent }.joined(separator: ", ")
        return "Wheel Server cannot open \(names)."
    }

    private var denied: Bool { host?.folders.contains { !$0.allowed && !$0.waiting } ?? false }

    private var restartCommand: String {
        "\(Bundle.main.bundlePath)/Contents/Resources/wheel-server.sh restart"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Folder access").font(ChatFont.body).fontWeight(.semibold)
            VStack(alignment: .leading, spacing: 4) {
                if let host {
                    ForEach(host.folders, id: \.path) { folder in
                        HStack(spacing: 12) {
                            Text(URL(filePath: folder.path).lastPathComponent).font(ChatFont.body).frame(width: 96, alignment: .leading)
                            Text(folder.allowed ? "allowed" : folder.waiting ? "waiting for permission" : "denied").font(ChatFont.body)
                                .foregroundStyle(folder.allowed ? Color.green : Color.orange)
                            Spacer()
                        }
                    }
                    if denied, changedSinceStart, let restart {
                        Text("Folder access was changed after the server started, and a running server keeps the access it started with. Restart the server to apply it. That ends every terminal and agent.")
                            .foregroundStyle(.secondary)
                        HStack {
                            Button("Restart Server", action: restart).buttonStyle(.borderedProminent)
                            Button("Open Files & Folders") { FullDiskAccess.openFolders() }
                        }
                    } else if denied {
                        Text("Terminals and agents in Wheel get “Operation not permitted” there, because macOS charges their access to the server. Switch Wheel Server on under Privacy & Security › Files & Folders, then restart the server so it picks the grant up:")
                            .foregroundStyle(.secondary)
                        Text(host.exe).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                        Text(restartCommand).foregroundStyle(.secondary).textSelection(.enabled)
                        HStack {
                            Button("Open Files & Folders") { FullDiskAccess.openFolders() }
                            Button("Copy Path") { FullDiskAccess.copyPath(host.exe) }
                            Button("Show in Finder") { FullDiskAccess.reveal(host.exe) }
                        }
                    }
                } else {
                    Text("Wheel Server has not said yet.").foregroundStyle(.secondary)
                }
            }
            .font(ChatFont.line)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14).padding(.vertical, 10)
            .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
        }
    }
}
