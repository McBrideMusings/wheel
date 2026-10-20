import SwiftUI
import WheelKit

/// The `wheel` command in `~/.local/bin`, as the bundled `wheel install` reports and sets it, and
/// whether wheeld wrote the shell integration its terminals and agents start with. Nothing here
/// decides what installed means: the answer is the first word `wheel install --status` prints.
@MainActor
final class CommandLineModel: ObservableObject {
    /// `installed`, `elsewhere`, `missing`, `not on PATH`, `unknown` (the check has not run or
    /// could not) or `unavailable` (this build carries no `wheel`).
    @Published private(set) var state = "unknown"
    /// The line `wheel install` printed, or why it failed.
    @Published private(set) var detail: String?
    @Published private(set) var running = false
    /// Why wheeld could not finish the shell integration; nil when it did.
    @Published var shellProblem: String?

    private var program: URL? {
        let url = Bundle.main.bundleURL.appending(path: "Contents/Helpers/wheel")
        return FileManager.default.isExecutableFile(atPath: url.path) ? url : nil
    }

    /// Ask where the link stands.
    func refresh() { run(["install", "--status"]) }

    /// Link the bundled `wheel` into `~/.local/bin`.
    func install() { run(["install"]) }

    private func run(_ arguments: [String]) {
        guard !running else { return }
        guard let program else {
            state = "unavailable"
            detail = "This build of the app carries no wheel command."
            return
        }
        running = true
        Task {
            let (code, text) = await Task.detached { Self.exec(program, arguments) }.value
            running = false
            // "<state>: <link> -> <target>"; a failure prints "wheel: <reason>" on stderr.
            if code == 0, let colon = text.firstIndex(of: ":") {
                state = String(text[..<colon])
                detail = text
            } else {
                state = "unknown"
                detail = text.isEmpty ? "wheel install exited \(code)" : text
            }
            Log.info("command line: wheel \(arguments.joined(separator: " ")) -> exit \(code), state \(state): \(detail ?? "")")
        }
    }

    /// Runs `wheel` in the person's interactive login shell, so "on PATH" means on the PATH their
    /// terminals have: an app opened from Finder starts with only the system's.
    private nonisolated static func exec(_ program: URL, _ arguments: [String]) -> (Int32, String) {
        let process = Process()
        process.executableURL = URL(filePath: ProcessInfo.processInfo.environment["SHELL"] ?? "/bin/zsh")
        process.arguments = ["-ilc", "exec \"$0\" \"$@\"", program.path] + arguments
        let out = Pipe()
        let err = Pipe()
        process.standardOutput = out
        process.standardError = err
        do {
            try process.run()
        } catch {
            return (-1, error.localizedDescription)
        }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        let errData = err.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        let trim = { (d: Data) in String(decoding: d, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines) }
        let text = trim(data)
        // An interactive shell may print its own notices on stderr; only a failure shows them.
        return (process.terminationStatus, process.terminationStatus == 0 ? text : trim(errData))
    }
}

/// The Command line box of the Global tab: where the `wheel` command stands, a button that links
/// it, and wheeld's own shell-integration problem when it has one.
struct CommandLineSettings: View {
    @ObservedObject var commandLine: CommandLineModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Command line").font(ChatFont.body).fontWeight(.semibold)
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Text("wheel").font(ChatFont.body).frame(width: 96, alignment: .leading)
                    Text(commandLine.state).font(ChatFont.body)
                        .foregroundStyle(commandLine.state == "installed" ? Color.green : Color.orange)
                    Spacer()
                    Button(commandLine.state == "installed" ? "Reinstall" : "Install") { commandLine.install() }
                        .disabled(commandLine.running || commandLine.state == "unavailable")
                }
                .padding(.horizontal, 14).padding(.vertical, 10)
                Divider().padding(.horizontal, 14)
                VStack(alignment: .leading, spacing: 4) {
                    if let problem = commandLine.shellProblem {
                        Label("wheeld: \(problem)", systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
                    }
                    if let detail = commandLine.detail {
                        Text(detail).font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
                            .textSelection(.enabled)
                    }
                    Text(help).foregroundStyle(.secondary)
                }
                .font(ChatFont.line)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 10)
            }
            .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.separator, lineWidth: 0.5))
        }
    }

    private var help: String {
        switch commandLine.state {
        case "installed": "wheel runs from any terminal."
        case "elsewhere": "Another wheel is linked in ~/.local/bin. Install replaces a link, never a real file."
        case "missing": "Install links this app’s wheel into ~/.local/bin."
        case "not on PATH": "The link is there but ~/.local/bin is not on your PATH."
        default: "Terminals and agents started by wheel always find it; this is for your own terminals."
        }
    }
}
