import Foundation

/// What the status line under an agent's composer shows besides the context and controls, held by
/// the daemon so every client shows the same. `AgentFooter` in `proto/src/lib.rs` is its twin.
public struct AgentFooter: Decodable, Equatable, Sendable {
    /// The directory the agent works in.
    public let directory: String
    /// The branch checked out there; nil outside a repository or on a detached head.
    public let branch: String?
    /// Lines added and removed against HEAD in the working tree, untracked files not counted.
    public let added: UInt32
    public let removed: UInt32
    /// The background tasks running, oldest first.
    public let tasks: [RunningTask]

    public init(directory: String, branch: String? = nil, added: UInt32 = 0, removed: UInt32 = 0, tasks: [RunningTask] = []) {
        self.directory = directory
        self.branch = branch
        self.added = added
        self.removed = removed
        self.tasks = tasks
    }

    private enum CodingKeys: String, CodingKey { case directory, branch, added, removed, tasks }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        directory = try c.decode(String.self, forKey: .directory)
        branch = try c.decodeIfPresent(String.self, forKey: .branch)
        added = try c.decodeIfPresent(UInt32.self, forKey: .added) ?? 0
        removed = try c.decodeIfPresent(UInt32.self, forKey: .removed) ?? 0
        tasks = try c.decodeIfPresent([RunningTask].self, forKey: .tasks) ?? []
    }

    /// The directory with the person's home folder written `~`, as the status line shows it.
    public var shownDirectory: String {
        let mine = NSHomeDirectory()
        return directory == mine || directory.hasPrefix(mine + "/") ? "~" + directory.dropFirst(mine.count) : directory
    }

    /// The running background tasks as the status line reads them, `1 shell · 2 agents`; nil when
    /// none run, so the line draws nothing at zero.
    public var backgroundText: String? {
        let shells = tasks.filter { $0.kind == .shell }.count
        let agents = tasks.count - shells
        var parts: [String] = []
        if shells > 0 { parts.append(shells == 1 ? "1 shell" : "\(shells) shells") }
        if agents > 0 { parts.append(agents == 1 ? "1 agent" : "\(agents) agents") }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}

/// One background task an agent has running, with only what the agent's own events say about it.
/// `BackgroundTask` in `proto/src/lib.rs` is its twin.
public struct RunningTask: Decodable, Equatable, Sendable, Identifiable {
    public enum Kind: String, Decodable, Sendable { case shell, agent }

    public let id: String
    public let kind: Kind
    /// The agent's one-line description; for Codex, the command.
    public let name: String
    /// The command a shell runs, or the brief an agent was given.
    public let command: String?
    /// The transcript tool call that started it.
    public let call: String?
    /// When the daemon first heard of it, unix ms.
    public let startedMs: UInt64
    public let outputPath: String?
    /// The last lines of output, at most eight, oldest first.
    public let tail: [String]
    public let outputBytes: UInt64
    /// Why the output could not be read, or that the agent named no file.
    public let outputError: String?
    /// When output last grew, unix ms.
    public let lastOutputMs: UInt64?
    public let toolUses: UInt32?
    public let lastTool: String?
    public let tokens: UInt64?

    public init(id: String, kind: Kind, name: String, command: String? = nil, call: String? = nil, startedMs: UInt64, outputPath: String? = nil, tail: [String] = [], outputBytes: UInt64 = 0, outputError: String? = nil, lastOutputMs: UInt64? = nil, toolUses: UInt32? = nil, lastTool: String? = nil, tokens: UInt64? = nil) {
        self.id = id; self.kind = kind; self.name = name; self.command = command; self.call = call
        self.startedMs = startedMs; self.outputPath = outputPath; self.tail = tail; self.outputBytes = outputBytes
        self.outputError = outputError; self.lastOutputMs = lastOutputMs; self.toolUses = toolUses
        self.lastTool = lastTool; self.tokens = tokens
    }

    private enum CodingKeys: String, CodingKey {
        case id, kind, name, command, call, tail
        case startedMs = "started_ms", outputPath = "output_path", outputBytes = "output_bytes"
        case outputError = "output_error", lastOutputMs = "last_output_ms", toolUses = "tool_uses"
        case lastTool = "last_tool", tokens
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = (try? c.decode(Kind.self, forKey: .kind)) ?? .shell
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        command = try c.decodeIfPresent(String.self, forKey: .command)
        call = try c.decodeIfPresent(String.self, forKey: .call)
        startedMs = try c.decodeIfPresent(UInt64.self, forKey: .startedMs) ?? 0
        outputPath = try c.decodeIfPresent(String.self, forKey: .outputPath)
        tail = try c.decodeIfPresent([String].self, forKey: .tail) ?? []
        outputBytes = try c.decodeIfPresent(UInt64.self, forKey: .outputBytes) ?? 0
        outputError = try c.decodeIfPresent(String.self, forKey: .outputError)
        lastOutputMs = try c.decodeIfPresent(UInt64.self, forKey: .lastOutputMs)
        toolUses = try c.decodeIfPresent(UInt32.self, forKey: .toolUses)
        lastTool = try c.decodeIfPresent(String.self, forKey: .lastTool)
        tokens = try c.decodeIfPresent(UInt64.self, forKey: .tokens)
    }

    /// Output older than this with the task still running reads as quiet.
    public static let quietAfter: TimeInterval = 5 * 60

    /// Seconds the task has run at `now`.
    public func elapsed(at now: Date) -> TimeInterval {
        max(0, now.timeIntervalSince1970 - Double(startedMs) / 1000)
    }

    /// Seconds since the output last grew (since the start when there is none yet), when that is
    /// over `quietAfter`; nil while output is recent. Subagents write no output file, so they are
    /// never quiet.
    public func quiet(at now: Date) -> TimeInterval? {
        guard kind == .shell else { return nil }
        let since = now.timeIntervalSince1970 - Double(lastOutputMs ?? startedMs) / 1000
        return since >= Self.quietAfter ? since : nil
    }

    /// The line the strip shows beside the name: the latest output line, a subagent's progress, or
    /// why there is none.
    public var latest: String {
        switch kind {
        case .agent:
            var parts: [String] = []
            if let toolUses { parts.append(toolUses == 1 ? "1 tool call" : "\(toolUses) tool calls") }
            if let lastTool { parts.append("last: \(lastTool)") }
            if let tokens { parts.append(String(format: "%.1fk tokens", Double(tokens) / 1000)) }
            return parts.isEmpty ? "started" : parts.joined(separator: " · ")
        case .shell:
            if let outputError { return outputError }
            return tail.last ?? "no output yet"
        }
    }

    /// `m:ss`, or `h:mm:ss` from an hour, as the strip and the transcript show a running time.
    public static func clock(_ seconds: TimeInterval) -> String {
        let s = Int(seconds)
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
    }
}
