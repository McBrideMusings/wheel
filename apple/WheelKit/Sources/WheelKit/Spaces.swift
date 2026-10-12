import Foundation

/// The daemon's layout (proto/src/lib.rs SpaceInfo, Node, PaneKind). The daemon owns it
/// (ADR-0001): a client renders the latest `spaces` broadcast and changes it only by sending ops.

/// One project's workspace: a directory and a split tree of panes.
public struct SpaceInfo: Decodable, Sendable, Equatable, Identifiable {
    public let id: String
    public let name: String
    public let cwd: String
    public let root: Node
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
    /// `web` needs a folder, sent beside it as `dir`.
    case empty, terminal, web
}

/// What a pane shows.
public enum PaneKind: Sendable, Equatable {
    /// Not chosen yet: the pane asks what it shows.
    case empty
    case terminal(term: String)
    /// The HTML in folder `dir`, served by the daemon at `/artifacts/<pane>/`.
    case web(dir: String)
    /// A kind this build does not know, or a known one it could not read. `raw` is every field
    /// the daemon sent beside the pane's `node` and `id`, kept so the client can show it; the
    /// daemon saves it back unchanged.
    case unknown(kind: String, raw: [String: JSONValue])

    public var name: String {
        switch self {
        case .empty: "empty"
        case .terminal: "terminal"
        case .web: "web"
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
            switch (raw["kind"], raw["term"], raw["dir"]) {
            case (.string("empty"), _, _): self = .pane(id: id, kind: .empty)
            case let (.string("terminal"), .string(term), _): self = .pane(id: id, kind: .terminal(term: term))
            case let (.string("web"), _, .string(dir)): self = .pane(id: id, kind: .web(dir: dir))
            case let (.string(kind), _, _): self = .pane(id: id, kind: .unknown(kind: kind, raw: raw))
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
