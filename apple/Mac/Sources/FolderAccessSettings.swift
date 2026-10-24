import AppKit
import SwiftUI
import WheelKit
import WheelUI

/// The Folder access box of the General tab: whether wheeld can list Downloads, Desktop and
/// Documents. macOS charges a background daemon's access to Wheel and cannot ask the person, so a
/// folder nobody granted fails with "Operation not permitted" inside every terminal and agent.
struct FolderAccessSettings: View {
    let host: HostAccess?

    static func title(_ host: HostAccess) -> String {
        let denied = host.folders.filter { !$0.allowed }
        if denied.isEmpty { return "wheeld can open Downloads, Desktop and Documents." }
        let names = denied.map { URL(filePath: $0.path).lastPathComponent }.joined(separator: ", ")
        return "wheeld cannot open \(names)."
    }

    private var denied: Bool { host?.folders.contains { !$0.allowed } ?? false }

    private var restartCommand: String {
        "launchctl kickstart -k gui/\(getuid())/\(DaemonService.bundled()?.label ?? "<daemon label>")"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Folder access").font(ChatFont.body).fontWeight(.semibold)
            VStack(alignment: .leading, spacing: 4) {
                if let host {
                    ForEach(host.folders, id: \.path) { folder in
                        HStack(spacing: 12) {
                            Text(URL(filePath: folder.path).lastPathComponent).font(ChatFont.body).frame(width: 96, alignment: .leading)
                            Text(folder.allowed ? "allowed" : "denied").font(ChatFont.body)
                                .foregroundStyle(folder.allowed ? Color.green : Color.orange)
                            Spacer()
                        }
                    }
                    if denied {
                        Text("Terminals and agents in Wheel get “Operation not permitted” there. Add Wheel under Privacy & Security › Full Disk Access, then restart the daemon so it picks the grant up: \(restartCommand)")
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                        Button("Open Full Disk Access") {
                            if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles") {
                                NSWorkspace.shared.open(url)
                            }
                        }
                    }
                } else {
                    Text("wheeld has not said yet.").foregroundStyle(.secondary)
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
