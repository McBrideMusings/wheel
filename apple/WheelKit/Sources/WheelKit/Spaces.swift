import Foundation

/// The daemon's layout (proto/src/lib.rs SpaceInfo, Node, PaneKind). The daemon owns it
/// (ADR-0001): a client renders the latest `spaces` broadcast and changes it only by sending ops.
/// One field arrives on its own — `pane_url`, a browser pane's new page, which changes far too
/// often to resend the whole layout for — and a client applies that to the tree it already holds.

/// One project's workspace: a directory and a split tree of panes.
public struct SpaceInfo: Decodable, Sendable, Equatable, Identifiable {
    public let id: String
    public let name: String
    public let cwd: String
    /// Settable so a client can apply a narrow broadcast — `pane_url` — to the tree it already
    /// holds, rather than waiting for the whole layout to be sent again.
    public var root: Node
    /// The pane that has focus in this space.
    public let focus: String
}

/// `horizontal` puts `a` left of `b`; `vertical` puts `a` above `b`.
public enum Axis: String, Decodable, Sendable {
    case horizontal, vertical
}

/// Which side of a pane a split puts the new pane on.
public enum Dir: String, Sendable, CaseIterable {
    case left, right, up, down
}

/// What `pane_set_kind` turns a pane into.
public enum NewPaneKind: String, Sendable, CaseIterable {
    /// `web` needs an artifact's name, sent beside it as `artifact` with an optional `scope`;
    /// `browser` takes an optional `url` and opens the `browser.homepage` setting without one.
    case empty, terminal, web, browser
}

/// What `pane_start` puts in a pane: a shell, or an agent session running the daemon's agent
/// command, both in the space's directory.
public enum StartKind: String, Sendable, CaseIterable {
    case terminal, agent
}

/// The two ways a pane shows a Claude Code conversation: wheel's agent chat, or the Claude Code
/// TUI in a terminal.
public enum ConversationView: String, Sendable, CaseIterable {
    case agent, terminal
}

/// The coding agent running in a terminal pane, as the daemon last sampled it: which one
/// (`claude`, `codex`) and its session's id.
public struct TermAgent: Sendable, Equatable {
    public let name: String
    public let session: String
    /// How full the session's context window is, as the daemon read it from the agent's transcript.
    public let context: ContextUse?

    public init(name: String, session: String, context: ContextUse? = nil) {
        self.name = name
        self.session = session
        self.context = context
    }

    /// Claude Code, whose conversation the pane can also show as a chat.
    public var isClaude: Bool { name == "claude" }
}

/// Which registry an artifact belongs to: `project` is the space's own, `machine` is every project
/// on this machine. The daemon resolves the folder; a client never sees or sends a path.
public enum ArtifactScope: String, Decodable, Sendable, CaseIterable {
    case project, machine
}

/// Which artifact a web pane shows: a scope and a name, never a path.
public struct ArtifactRef: Sendable, Equatable {
    public let scope: ArtifactScope
    public let name: String

    public init(scope: ArtifactScope, name: String) {
        self.scope = scope
        self.name = name
    }

    /// One plain name: not empty, no slash, not `.` or `..`, and not starting with `.` or `-`.
    /// The daemon's `ArtifactRef::new` applies the same rule (`proto/src/lib.rs`, `plain_name`),
    /// so a name this rejects is one the daemon would refuse too.
    static func isPlainName(_ name: String) -> Bool {
        // Bytes, not Characters: a combining mark after `-` makes one Character that `hasPrefix`
        // would miss, where the daemon's byte-level check refuses it.
        let bytes = name.utf8
        guard let first = bytes.first else { return false }
        return first != UInt8(ascii: ".") && first != UInt8(ascii: "-") && !bytes.contains(UInt8(ascii: "/"))
    }
}

/// What a pane shows.
public enum PaneKind: Sendable, Equatable {
    /// Not chosen yet: the pane asks what it shows.
    case empty
    /// A shell; `agent` is the coding agent running in it, when the daemon has seen one.
    case terminal(term: String, agent: TermAgent? = nil)
    /// The artifact `artifact` names, served by the daemon at `/artifacts/<pane>/`.
    case web(artifact: ArtifactRef)
    /// An ordinary browser on `url`: the open internet, with chrome, where a web pane is one page
    /// a project ships with none. The client reports navigation back so the layout follows it.
    case browser(url: String)
    /// An agent session the pane owns: it ends when the pane closes or changes kind.
    case agent(session: String)
    /// A kind this build does not know, or a known one it could not read. `raw` is every field
    /// the daemon sent beside the pane's `node` and `id`, kept so the client can show it; the
    /// daemon saves it back unchanged.
    case unknown(kind: String, raw: [String: JSONValue])

    /// Whether changing this pane to another kind ends a shell or an agent session, which a client
    /// asks about first.
    public var endsASession: Bool {
        switch self {
        case .terminal, .agent: true
        default: false
        }
    }

    public var name: String {
        switch self {
        case .empty: "empty"
        case .terminal: "terminal"
        case .web: "web"
        case .browser: "browser"
        case .agent: "agent"
        case let .unknown(kind, _): kind
        }
    }
}

/// A split tree, after Ghostty's: every leaf is a pane, every inner node splits its area in two.
public indirect enum Node: Sendable, Equatable {
    case pane(id: String, kind: PaneKind)
    /// `ratio` is the share of the split `a` takes.
    case split(id: String, axis: Axis, ratio: Double, a: Node, b: Node)

    public var id: String {
        switch self {
        case let .pane(id, _), let .split(id, _, _, _, _): id
        }
    }

    /// Every pane in the tree, in order.
    public var panes: [(id: String, kind: PaneKind)] {
        switch self {
        case let .pane(id, kind): [(id, kind)]
        case let .split(_, _, _, a, b): a.panes + b.panes
        }
    }

    /// The same tree with `pane` showing `kind`, for applying a broadcast that carries one pane's
    /// change instead of the whole layout. A tree without that pane comes back unchanged.
    public func replacing(pane: String, with kind: PaneKind) -> Node {
        switch self {
        case let .pane(id, _) where id == pane: .pane(id: id, kind: kind)
        case .pane: self
        case let .split(id, axis, ratio, a, b):
            .split(id: id, axis: axis, ratio: ratio,
                   a: a.replacing(pane: pane, with: kind), b: b.replacing(pane: pane, with: kind))
        }
    }
}

extension Node: Decodable {
    private enum K: String, CodingKey { case node, id, axis, ratio, a, b }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        let id = try c.decode(String.self, forKey: .id)
        switch try c.decode(String.self, forKey: .node) {
        case "split":
            self = .split(id: id, axis: try c.decode(Axis.self, forKey: .axis), ratio: try c.decode(Double.self, forKey: .ratio),
                          a: try c.decode(Node.self, forKey: .a), b: try c.decode(Node.self, forKey: .b))
        case "pane":
            var raw = try decoder.singleValueContainer().decode([String: JSONValue].self)
            raw["node"] = nil
            raw["id"] = nil
            // A pane this build can't read — an unknown kind, or a known one missing its fields —
            // is shown as unsupported rather than failing the whole layout frame.
            switch (raw["kind"], raw["term"], raw["name"], raw["session"]) {
            case (.string("empty"), _, _, _): self = .pane(id: id, kind: .empty)
            case let (.string("terminal"), .string(term), _, _):
                var agent: TermAgent?
                if case let .object(fields)? = raw["agent"], case let .string(name)? = fields["name"], case let .string(session)? = fields["session"] {
                    var context: ContextUse?
                    if case let .object(c)? = fields["context"], case let .number(used)? = c["used"], case let .number(size)? = c["size"], used.isFinite, size.isFinite, used >= 0, size >= 0, used < 1e15, size < 1e15 {
                        context = ContextUse(used: UInt64(used), size: UInt64(size))
                    }
                    agent = TermAgent(name: name, session: session, context: context)
                }
                self = .pane(id: id, kind: .terminal(term: term, agent: agent))
            case let (.string("web"), _, .string(name), _):
                if case let .string(scope) = raw["scope"] ?? .null, let scope = ArtifactScope(rawValue: scope), ArtifactRef.isPlainName(name) {
                    self = .pane(id: id, kind: .web(artifact: ArtifactRef(scope: scope, name: name)))
                } else {
                    self = .pane(id: id, kind: .unknown(kind: "web", raw: raw))
                }
            case (.string("browser"), _, _, _):
                if case let .string(url) = raw["url"] ?? .null {
                    self = .pane(id: id, kind: .browser(url: url))
                } else {
                    self = .pane(id: id, kind: .unknown(kind: "browser", raw: raw))
                }
            case let (.string("agent"), _, _, .string(session)): self = .pane(id: id, kind: .agent(session: session))
            case let (.string(kind), _, _, _): self = .pane(id: id, kind: .unknown(kind: kind, raw: raw))
            default: self = .pane(id: id, kind: .unknown(kind: "unnamed", raw: raw))
            }
        case let other:
            throw DecodingError.dataCorruptedError(forKey: .node, in: c, debugDescription: "unknown node \(other)")
        }
    }
}

/// Any JSON value, for fields this build does not model.
public enum JSONValue: Decodable, Sendable, Equatable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode([JSONValue].self) { self = .array(v) }
        else { self = .object(try c.decode([String: JSONValue].self)) }
    }

    /// The value as Foundation objects, for JSONSerialization.
    public var foundation: Any {
        switch self {
        case .null: NSNull()
        case let .bool(v): v
        case let .number(v): v
        case let .string(v): v
        case let .array(v): v.map(\.foundation)
        case let .object(v): v.mapValues(\.foundation)
        }
    }
}
