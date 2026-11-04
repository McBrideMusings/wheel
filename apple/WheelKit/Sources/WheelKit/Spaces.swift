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
    /// The pane this client has focused in this space. The daemon keeps no focus (ADR-0022), so it
    /// is not decoded: a client sets it as the person moves between panes, and carries it over when
    /// a new layout arrives. Until it is set, it is the first pane.
    public var focus: String
    /// The highest status among its panes; nil when no pane runs an agent. Settable for
    /// `status_changed`.
    public var status: PaneStatus?
    /// Each pane's status, by pane id; a pane that runs no agent has none.
    public var paneStatuses: [String: PaneStatus]
    /// Made to run with no window (`wheel space new --background`): a window never shows it until
    /// the person brings it forward.
    public let background: Bool
    /// Panes popped out of the grid: each tree is a floating window over it. Where those windows
    /// sit and how big they are is the client's own (ADR-0022).
    public var popped: [Node]
    /// The space's tray: one pane leaf each, running and shown nowhere.
    public var backgrounded: [Node]

    public init(id: String, name: String, cwd: String, root: Node, focus: String,
                status: PaneStatus? = nil, paneStatuses: [String: PaneStatus] = [:], background: Bool = false,
                popped: [Node] = [], backgrounded: [Node] = []) {
        self.id = id
        self.name = name
        self.cwd = cwd
        self.root = root
        self.focus = focus
        self.status = status
        self.paneStatuses = paneStatuses
        self.background = background
        self.popped = popped
        self.backgrounded = backgrounded
    }

    private enum K: String, CodingKey { case id, name, cwd, root, status, background, popped, backgrounded }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        cwd = try c.decode(String.self, forKey: .cwd)
        root = try c.decode(Node.self, forKey: .root)
        focus = root.readingOrder.first?.id ?? ""
        // A status this build can't read is no status, not a failed layout frame.
        status = try? c.decodeIfPresent(PaneStatus.self, forKey: .status)
        popped = try c.decodeIfPresent([Node].self, forKey: .popped) ?? []
        backgrounded = try c.decodeIfPresent([Node].self, forKey: .backgrounded) ?? []
        var statuses = (try? c.decode(StatusTree.self, forKey: .root).statuses) ?? [:]
        for key in [K.popped, K.backgrounded] {
            for tree in (try? c.decodeIfPresent([StatusTree].self, forKey: key)) ?? [] { statuses.merge(tree.statuses) { $1 } }
        }
        paneStatuses = statuses
        background = try c.decodeIfPresent(Bool.self, forKey: .background) ?? false
    }
}

/// Where a pane is in its space: in the grid, popped out, or in the tray.
public enum PaneSite: String, Sendable, Equatable {
    case grid, popped, backgrounded
}

extension SpaceInfo {
    /// Every pane the space holds, wherever it is: the grid, each popped tree, then the tray.
    public var allPanes: [(id: String, kind: PaneKind)] {
        root.panes + popped.flatMap(\.panes) + backgrounded.flatMap(\.panes)
    }

    /// Where `pane` is, or nil when the space does not hold it.
    public func site(of pane: String) -> PaneSite? {
        if root.panes.contains(where: { $0.id == pane }) { return .grid }
        if popped.contains(where: { $0.panes.contains { $0.id == pane } }) { return .popped }
        if backgrounded.contains(where: { $0.panes.contains { $0.id == pane } }) { return .backgrounded }
        return nil
    }

    /// The pane this client keeps focused when the space's tree becomes `root`: the same pane while it
    /// is there, else the pane that now sits where it was in reading order (a closed pane's neighbour).
    public func focusKept(in root: Node) -> String {
        let now = root.readingOrder.map(\.id)
        if now.contains(focus) { return focus }
        guard !now.isEmpty else { return focus }
        let was = self.root.readingOrder.map(\.id).firstIndex(of: focus) ?? 0
        return now[min(was, now.count - 1)]
    }

    /// Most layers a pane-count stack draws.
    public static let stackLimit = 4

    /// Layers a pane-count stack draws: one per pane, at most `stackLimit`.
    public var stackLayers: Int { min(max(root.panes.count, 1), Self.stackLimit) }

    /// The first pane's kind, which the front layer's glyph shows.
    public var firstPaneKind: PaneKind { root.panes.first?.kind ?? .empty }
}

/// The `status` fields of a split tree, by pane id, read apart from the layout.
private struct StatusTree: Decodable {
    var statuses: [String: PaneStatus] = [:]

    private enum K: String, CodingKey { case id, status, a, b }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        if let status = try? c.decodeIfPresent(PaneStatus.self, forKey: .status) {
            statuses[try c.decode(String.self, forKey: .id)] = status
        }
        for side in [K.a, K.b] {
            if let tree = try c.decodeIfPresent(StatusTree.self, forKey: side) { statuses.merge(tree.statuses) { $1 } }
        }
    }
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

/// What `pane_start` puts in a pane: a shell, or an agent running the daemon's agent
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
/// (`claude`, `codex`) and the id of its conversation.
public struct TermAgent: Sendable, Equatable {
    public let name: String
    public let conversation: String
    /// How full the agent's context window is, as the daemon read it from the agent's transcript.
    public let context: ContextUse?

    public init(name: String, conversation: String, context: ContextUse? = nil) {
        self.name = name
        self.conversation = conversation
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
    /// An agent the pane owns, running the program named `harness`: it ends when the pane
    /// closes or changes kind. `home` is the configuration directory the pane saved for its own
    /// agent; nil when the pane runs under the agent's `agent.<name>.home` setting.
    case agent(agent: String, harness: String, home: String? = nil)
    /// A kind this build does not know, or a known one it could not read. `raw` is every field
    /// the daemon sent beside the pane's `node` and `id`, kept so the client can show it; the
    /// daemon saves it back unchanged.
    case unknown(kind: String, raw: [String: JSONValue])

    /// Whether changing this pane to another kind ends a shell or an agent, which a client
    /// asks about first.
    public var endsAnAgent: Bool {
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

    /// Whether `other` has the same panes in the same splits: ids, axes and ratios, whatever the
    /// panes show. A status report (an agent's context use, a browser's URL) changes a pane's kind
    /// but never the shape.
    public func sameShape(as other: Node) -> Bool {
        switch (self, other) {
        case let (.pane(a, _), .pane(b, _)): a == b
        case let (.split(ia, xa, ra, aa, ba), .split(ib, xb, rb, ab, bb)):
            ia == ib && xa == xb && ra == rb && aa.sameShape(as: ab) && ba.sameShape(as: bb)
        default: false
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
            raw["status"] = nil
            // A pane this build can't read — an unknown kind, or a known one missing its fields —
            // is shown as unsupported rather than failing the whole layout frame.
            switch (raw["kind"], raw["term"], raw["name"], raw["agent"]) {
            case (.string("empty"), _, _, _): self = .pane(id: id, kind: .empty)
            case let (.string("terminal"), .string(term), _, _):
                var agent: TermAgent?
                if case let .object(fields)? = raw["agent"], case let .string(name)? = fields["name"], case let .string(conversation)? = fields["conversation"] {
                    var context: ContextUse?
                    if case let .object(c)? = fields["context"], case let .number(used)? = c["used"], case let .number(size)? = c["size"], used.isFinite, size.isFinite, used >= 0, size >= 0, used < 1e15, size < 1e15 {
                        context = ContextUse(used: UInt64(used), size: UInt64(size))
                    }
                    agent = TermAgent(name: name, conversation: conversation, context: context)
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
            case let (.string("agent"), _, _, .string(agent)):
                if case let .string(harness) = raw["harness"] ?? .null {
                    var home: String?
                    if case let .string(h)? = raw["home"] { home = h }
                    self = .pane(id: id, kind: .agent(agent: agent, harness: harness, home: home))
                } else {
                    self = .pane(id: id, kind: .unknown(kind: "agent", raw: raw))
                }
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
