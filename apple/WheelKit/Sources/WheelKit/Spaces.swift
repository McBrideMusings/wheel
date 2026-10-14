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

/// What `pane_start` puts in a pane: a shell, or an agent session running the lead's agent
/// command, both in the space's directory.
public enum StartKind: String, Sendable, CaseIterable {
    case terminal, agent
}

/// Which root an artifact's folder sits under: `repo` is committed with the project, `local`
/// belongs to one checkout, `shared` is every project on this machine. The daemon owns all three
/// roots and resolves the folder; a client never sees or sends a path.
public enum ArtifactScope: String, Decodable, Sendable, CaseIterable {
    case repo, local, shared
}

/// Which artifact a web pane shows: a scope and a name, never a path.
public struct ArtifactRef: Sendable, Equatable {
    public let scope: ArtifactScope
    public let name: String

    public init(scope: ArtifactScope, name: String) {
        self.scope = scope
        self.name = name
    }
}

/// What a pane shows.
public enum PaneKind: Sendable, Equatable {
    /// Not chosen yet: the pane asks what it shows.
    case empty
    case terminal(term: String)
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

/// Who made a pane: the person (a client or the CLI) or the lead (its tools). The lead asks the
/// person before acting on a pane the person made.
public enum CreatedBy: String, Sendable, Equatable, Decodable {
    case person
    case lead
}

/// A split tree, after Ghostty's: every leaf is a pane, every inner node splits its area in two.
public indirect enum Node: Sendable, Equatable {
    case pane(id: String, kind: PaneKind, createdBy: CreatedBy = .person)
    /// `ratio` is the share of the split `a` takes.
    case split(id: String, axis: Axis, ratio: Double, a: Node, b: Node)

    public var id: String {
        switch self {
        case let .pane(id, _, _), let .split(id, _, _, _, _): id
        }
    }

    /// Every pane in the tree, in order.
    public var panes: [(id: String, kind: PaneKind)] {
        switch self {
        case let .pane(id, kind, _): [(id, kind)]
        case let .split(_, _, _, a, b): a.panes + b.panes
        }
    }

    /// The same tree with `pane` showing `kind`, for applying a broadcast that carries one pane's
    /// change instead of the whole layout. A tree without that pane comes back unchanged.
    public func replacing(pane: String, with kind: PaneKind) -> Node {
        switch self {
        case let .pane(id, _, createdBy) where id == pane: .pane(id: id, kind: kind, createdBy: createdBy)
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
            // A layout saved before panes recorded who made them reads as the person's.
            var createdBy = CreatedBy.person
            if case let .string(by) = raw.removeValue(forKey: "created_by") ?? .null, let by = CreatedBy(rawValue: by) {
                createdBy = by
            }
            // A pane this build can't read — an unknown kind, or a known one missing its fields —
            // is shown as unsupported rather than failing the whole layout frame.
            switch (raw["kind"], raw["term"], raw["name"], raw["session"]) {
            case (.string("empty"), _, _, _): self = .pane(id: id, kind: .empty, createdBy: createdBy)
            case let (.string("terminal"), .string(term), _, _): self = .pane(id: id, kind: .terminal(term: term), createdBy: createdBy)
            case let (.string("web"), _, .string(name), _):
                if case let .string(scope) = raw["scope"] ?? .null, let scope = ArtifactScope(rawValue: scope) {
                    self = .pane(id: id, kind: .web(artifact: ArtifactRef(scope: scope, name: name)), createdBy: createdBy)
                } else {
                    self = .pane(id: id, kind: .unknown(kind: "web", raw: raw), createdBy: createdBy)
                }
            case (.string("browser"), _, _, _):
                if case let .string(url) = raw["url"] ?? .null {
                    self = .pane(id: id, kind: .browser(url: url), createdBy: createdBy)
                } else {
                    self = .pane(id: id, kind: .unknown(kind: "browser", raw: raw), createdBy: createdBy)
                }
            case let (.string("agent"), _, _, .string(session)): self = .pane(id: id, kind: .agent(session: session), createdBy: createdBy)
            case let (.string(kind), _, _, _): self = .pane(id: id, kind: .unknown(kind: kind, raw: raw), createdBy: createdBy)
            default: self = .pane(id: id, kind: .unknown(kind: "unnamed", raw: raw), createdBy: createdBy)
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
