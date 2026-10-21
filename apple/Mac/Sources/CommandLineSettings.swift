import Synchronization
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
            let (login, code, text) = await Task.detached {
                // "On PATH" means on the PATH the person's terminals have: an app opened from
                // Finder starts with only the system's.
                let login = LoginShell.path()
                var environment = ProcessInfo.processInfo.environment
                if case .path(let path) = login { environment["PATH"] = path }
                let (code, text) = Self.exec(program, arguments, environment)
                return (login, code, text)
            }.value
            running = false
            // "<state>: <link> -> <target>"; a failure prints "wheel: <reason>" on stderr.
            if code == 0, let colon = text.firstIndex(of: ":") {
                state = String(text[..<colon])
                detail = text
                // Without the shell's PATH, only where the link points is known.
                if case .failed(let why) = login, state == "installed" || state == "not on PATH" {
                    state = "unknown"
                    detail = "\(text[text.index(after: colon)...].trimmingCharacters(in: .whitespaces))\n\(why)"
                }
            } else {
                state = "unknown"
                detail = text.isEmpty ? "wheel install exited \(code)" : text
            }
            Log.info("command line: wheel \(arguments.joined(separator: " ")) -> exit \(code), state \(state): \(detail ?? "")")
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

/// The PATH the person's terminals have, as their interactive login shell sets it. Their startup
/// files are theirs, so they may wait on input or start something that never exits, and an
/// interactive zsh ignores SIGTERM: the shell runs in a session of its own, with no terminal and
/// stdin on /dev/null, and its whole process group is killed with SIGKILL once it has printed
/// PATH, after `limit`, or when the app quits.
enum LoginShell {
    enum Answer: Sendable {
        case path(String)
        case failed(String)
    }

    static let limit: Duration = .seconds(4)
    /// Groups still running, killed if the app quits first.
    private static let groups = Mutex<Set<pid_t>>([])
    private static let killOnQuit: Void = {
        _ = NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: nil
        ) { _ in
            // Under the lock, so a group cannot be reaped, and its pid reused, while it is signalled.
            groups.withLock { running in
                for group in running {
                    kill(-group, SIGKILL)
                    Log.info("command line: app quitting; killed shell group \(group)")
                }
            }
        }
    }()

    /// Runs `$SHELL -ilc` printing PATH between two record separators; blocks for up to `limit`.
    static func path() -> Answer {
        _ = killOnQuit
        let shell = ProcessInfo.processInfo.environment["SHELL"] ?? "/bin/zsh"
        let name = URL(filePath: shell).lastPathComponent
        var pipe: [Int32] = [0, 0]
        guard Darwin.pipe(&pipe) == 0 else { return .failed("could not ask \(name) for PATH: \(String(cString: strerror(errno)))") }
        defer { close(pipe[0]) }

        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        // A new session makes the shell lead its own group, with no terminal to wait on; no other
        // descriptor of the app's leaks into it.
        let setup = [
            posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSID | POSIX_SPAWN_CLOEXEC_DEFAULT)),
            posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0),
            posix_spawn_file_actions_adddup2(&actions, pipe[1], 1),
            posix_spawn_file_actions_addopen(&actions, 2, "/dev/null", O_WRONLY, 0),
        ]
        if let code = setup.first(where: { $0 != 0 }) {
            close(pipe[1])
            return .failed("could not prepare \(shell): \(String(cString: strerror(code)))")
        }

        let argv = [shell, "-ilc", #"printf '\036%s\036' "$PATH""#].map { strdup($0) } + [nil]
        defer { argv.forEach { free($0) } }
        var pid: pid_t = 0
        let spawned = groups.withLock { running in
            let code = posix_spawn(&pid, shell, &actions, &attributes, argv, environ)
            if code == 0 { running.insert(pid) }
            return code
        }
        close(pipe[1])
        guard spawned == 0 else { return .failed("could not start \(shell): \(String(cString: strerror(spawned)))") }
        Log.info("command line: asking \(shell) -il for PATH, pid \(pid)")

        let start = ContinuousClock.now
        var bytes: [UInt8] = []
        var chunk = [UInt8](repeating: 0, count: 4096)
        var answer: Answer?
        var ended = false
        while answer == nil, !ended {
            let parts = bytes.split(separator: 0x1E, omittingEmptySubsequences: false)
            if parts.count >= 3 {
                answer = .path(String(decoding: parts[1], as: UTF8.self))
                break
            }
            let left = start + limit - .now
            if left <= .zero { break }
            var poller = pollfd(fd: pipe[0], events: Int16(POLLIN), revents: 0)
            let ready = poll(&poller, 1, Int32((left / .milliseconds(1)).rounded(.up)))
            if ready < 0, errno != EINTR { break }
            guard ready > 0 else { continue }
            let count = read(pipe[0], &chunk, chunk.count)
            if count > 0 {
                bytes += chunk[..<count]
                // Startup files that print without end never reach PATH; stop reading them.
                if bytes.count > 256 * 1024 {
                    answer = .failed("\(name) -il printed over 256 KB without PATH, so it was stopped; something in its startup files prints without end.")
                }
            } else if count == 0 || errno != EINTR {
                ended = true
            }
        }
        // The shell may be done while something its startup files began is not. The pid leaves
        // the set before it is reaped, so the quit hook never signals a reused pid.
        kill(-pid, SIGKILL)
        groups.withLock { _ = $0.remove(pid) }
        var status: Int32 = 0
        while waitpid(pid, &status, 0) < 0, errno == EINTR {}
        let took = (ContinuousClock.now - start) / .milliseconds(1)
        let result = answer ?? .failed(ended
            ? "\(name) -il exited without printing PATH, so where it looks for commands is unknown."
            : "\(name) -il did not finish within \(limit.components.seconds) s, so it was stopped; something in its startup files waits for input.")
        switch result {
        case .path(let path): Log.info("command line: \(name) gave PATH in \(Int(took)) ms: \(path)")
        case .failed(let why): Log.info("command line: no PATH after \(Int(took)) ms, killed group \(pid): \(why)")
        }
        return result
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
