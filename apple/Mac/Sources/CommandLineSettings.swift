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
    /// Something asked for a check while one was running; it runs once that one ends.
    private var again = false
    /// Why wheeld could not finish the shell integration; nil when it did.
    @Published var shellProblem: String?
    /// The PATH wheeld runs with, which it took from the person's login shell; nil until wheeld
    /// has said. "On PATH" means on that one: an app opened from Finder starts with only the
    /// system's, and wheeld is the one place that asks the shell.
    @Published var loginPath: LoginPath? {
        // A check already shown was made against the PATH before this one.
        didSet { if loginPath != oldValue, detail != nil { refresh() } }
    }

    private var program: URL? {
        let url = Bundle.main.bundleURL.appending(path: "Contents/Helpers/wheel")
        return FileManager.default.isExecutableFile(atPath: url.path) ? url : nil
    }

    /// Ask where the link stands.
    func refresh() { run(["install", "--status"]) }

    /// Link the bundled `wheel` into `~/.local/bin`.
    func install() { run(["install"]) }

    private func run(_ arguments: [String]) {
        guard !running else {
            // A check already under way may be using a PATH wheeld has since replaced.
            again = true
            return
        }
        guard let program else {
            state = "unavailable"
            detail = "This build of the app carries no wheel command."
            return
        }
        running = true
        let login = loginPath
        Task {
            let (code, text) = await Task.detached {
                var environment = ProcessInfo.processInfo.environment
                if let login { environment["PATH"] = login.path }
                return Self.exec(program, arguments, environment)
            }.value
            running = false
            // "<state>: <link> -> <target>"; a failure prints "wheel: <reason>" on stderr.
            if code == 0, let colon = text.firstIndex(of: ":") {
                state = String(text[..<colon])
                detail = text
                // Without the shell's PATH, only where the link points is known.
                let why = if let login { login.problem } else { "wheeld has not said which PATH it runs with yet." }
                if let why, state == "installed" || state == "not on PATH" {
                    state = "unknown"
                    detail = "\(text[text.index(after: colon)...].trimmingCharacters(in: .whitespaces))\nwheeld: \(why)"
                }
            } else {
                state = "unknown"
                detail = text.isEmpty ? "wheel install exited \(code)" : text
            }
            if again {
                again = false
                refresh()
            }
            Log.info("command line: wheel \(arguments.joined(separator: " ")) with wheeld's PATH (\(login?.how ?? "none yet")) -> exit \(code), state \(state): \(detail ?? "")")
        }
    }

    private nonisolated static func exec(_ program: URL, _ arguments: [String], _ environment: [String: String]) -> (Int32, String) {
        let process = Process()
        process.executableURL = program
        process.arguments = arguments
        process.environment = environment
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
        return (process.terminationStatus, trim(process.terminationStatus == 0 ? data : errData))
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
