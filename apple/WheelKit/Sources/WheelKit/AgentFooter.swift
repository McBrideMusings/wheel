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
    /// Background commands running.
    public let shells: UInt32
    /// Background subagents running.
    public let agents: UInt32

    public init(directory: String, branch: String? = nil, added: UInt32 = 0, removed: UInt32 = 0, shells: UInt32 = 0, agents: UInt32 = 0) {
        self.directory = directory
        self.branch = branch
        self.added = added
        self.removed = removed
        self.shells = shells
        self.agents = agents
    }

    private enum CodingKeys: String, CodingKey { case directory, branch, added, removed, shells, agents }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        directory = try c.decode(String.self, forKey: .directory)
        branch = try c.decodeIfPresent(String.self, forKey: .branch)
        added = try c.decodeIfPresent(UInt32.self, forKey: .added) ?? 0
        removed = try c.decodeIfPresent(UInt32.self, forKey: .removed) ?? 0
        shells = try c.decodeIfPresent(UInt32.self, forKey: .shells) ?? 0
        agents = try c.decodeIfPresent(UInt32.self, forKey: .agents) ?? 0
    }

    /// The running background tasks as the status line reads them, `1 shell · 2 agents`; nil when
    /// none run, so the line draws nothing at zero.
    public var backgroundText: String? {
        var parts: [String] = []
        if shells > 0 { parts.append(shells == 1 ? "1 shell" : "\(shells) shells") }
        if agents > 0 { parts.append(agents == 1 ? "1 agent" : "\(agents) agents") }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}
