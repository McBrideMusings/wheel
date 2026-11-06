import Foundation
import Observation

// The client's whole model of a page, owned by Wheel: a flat list of components the server sent, and a data model of values.
// Server to client: `createSurface`, `updateComponents`, `updateDataModel`, `deleteSurface` (A2UI v0.9 messages).
// Client to server: `onAction` for a control that raises an action (a Button) and `onChange` for any value a control writes
// into the data model. Both reach the server as one tool call each; a control knows nothing of either (ADR-0033).

/// A JSON value, with the accessors a component's properties are read through.
public enum AnyCodable: Codable, Equatable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case null
    case array([AnyCodable])
    case dictionary([String: AnyCodable])

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let b = try? c.decode(Bool.self) { self = .bool(b) }
        else if let n = try? c.decode(Double.self) { self = .number(n) }
        else if let s = try? c.decode(String.self) { self = .string(s) }
        else if let a = try? c.decode([AnyCodable].self) { self = .array(a) }
        else { self = .dictionary(try c.decode([String: AnyCodable].self)) }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case let .bool(b): try c.encode(b)
        case let .number(n): try c.encode(n)
        case let .string(s): try c.encode(s)
        case let .array(a): try c.encode(a)
        case let .dictionary(d): try c.encode(d)
        }
    }

    public var stringValue: String? { if case let .string(s) = self { s } else { nil } }
    public var numberValue: Double? { if case let .number(n) = self { n } else { nil } }
    public var boolValue: Bool? { if case let .bool(b) = self { b } else { nil } }
    public var arrayValue: [AnyCodable]? { if case let .array(a) = self { a } else { nil } }
    public var dictionaryValue: [String: AnyCodable]? { if case let .dictionary(d) = self { d } else { nil } }
    /// The value as text, for a control that shows whatever it was given.
    public var text: String {
        switch self {
        case let .string(s): s
        case let .number(n): n == n.rounded() ? String(Int(n)) : String(n)
        case let .bool(b): String(b)
        default: ""
        }
    }
}

/// A component as the server sent it: its id, its name, its properties (a component's `children` and `child` are ids).
public struct ComponentInstance: Equatable, Sendable {
    public let id: String
    public let type: String
    public var properties: [String: AnyCodable]
}

/// A component with its children resolved: what a view is built from.
public struct ComponentNode: Sendable {
    public let instance: ComponentInstance
    public let children: [ComponentNode]
}

extension ComponentNode {
    var props: [String: AnyCodable] { instance.properties }
    func string(_ key: String) -> String? { props[key]?.stringValue }
    func number(_ key: String) -> Double? { props[key]?.numberValue }
    func bool(_ key: String) -> Bool? { props[key]?.boolValue }
}

/// An action a control raised: the tool it names and the context sent as its arguments.
public struct A2UIAction: Equatable, Sendable {
    public let name: String
    public let sourceComponentId: String
    public let context: [String: AnyCodable]
}

/// A value a control wrote into the data model.
public struct A2UIChange: Equatable, Sendable {
    public let path: String
    public let value: AnyCodable
}

@MainActor @Observable
public final class SurfaceModel {
    public private(set) var components: [String: ComponentInstance] = [:]
    public private(set) var data: [String: AnyCodable] = [:]
    public private(set) var created = false
    /// The server's tool that takes a change; `data_changed` unless `createSurface` names another.
    public private(set) var changeTool = "data_changed"
    /// Paths a control is being edited at: a server write to one waits, so a reply to an older value never overwrites newer typing.
    @ObservationIgnored public var held: Set<String> = []
    @ObservationIgnored public var onAction: ((A2UIAction) -> Void)?
    @ObservationIgnored public var onChange: ((A2UIChange) -> Void)?

    public init() {}

    // MARK: messages

    /// Applies the messages of one document or tool result; returns what it could not apply.
    @discardableResult
    public func process(_ messages: [[String: AnyCodable]]) -> [String] {
        var errors: [String] = []
        for m in messages {
            if let c = m["createSurface"]?.dictionaryValue {
                created = true
                if let t = c["changeTool"]?.stringValue { changeTool = t }
            } else if let u = m["updateComponents"]?.dictionaryValue {
                for raw in u["components"]?.arrayValue ?? [] {
                    guard var d = raw.dictionaryValue, let id = d["id"]?.stringValue, let type = d["component"]?.stringValue else {
                        errors.append("a component with no id or name"); continue
                    }
                    d["id"] = nil; d["component"] = nil
                    components[id] = ComponentInstance(id: id, type: type, properties: d)
                }
            } else if let u = m["updateDataModel"]?.dictionaryValue {
                let path = u["path"]?.stringValue ?? "/"
                if !held.contains(path) { write(path, u["value"] ?? .null) }
            } else if m["deleteSurface"] != nil {
                components = [:]; data = [:]; created = false; changeTool = "data_changed"; held = []
            }
        }
        return errors
    }

    // MARK: components

    /// The root component, resolved down to its leaves.
    public var root: ComponentNode? { node("root") }

    public func node(_ id: String, depth: Int = 0) -> ComponentNode? {
        guard depth < 64, let inst = components[id] else { return nil }
        var ids: [String] = []
        if let c = inst.properties["children"]?.arrayValue { ids += c.compactMap { $0.stringValue } }
        for key in ["child", "trigger", "content"] { if let s = inst.properties[key]?.stringValue { ids.append(s) } }
        let kids = ids.compactMap { node($0, depth: depth + 1) }
        return ComponentNode(instance: ComponentInstance(id: inst.id, type: inst.type, properties: bound(inst.properties)), children: kids)
    }

    // MARK: data

    /// A property whose value is `{"path": "/x"}` reads the data model's value at that path.
    private func bound(_ props: [String: AnyCodable]) -> [String: AnyCodable] {
        var out = props
        for (k, v) in props { if let p = v.dictionaryValue?["path"]?.stringValue, v.dictionaryValue?.count == 1 { out[k] = get(p) ?? .null } }
        return out
    }

    public func get(_ path: String) -> AnyCodable? {
        var cur: AnyCodable? = .dictionary(data)
        for part in path.split(separator: "/") { cur = cur?.dictionaryValue?[String(part)] }
        return cur
    }

    private func write(_ path: String, _ value: AnyCodable) {
        let parts = path.split(separator: "/").map(String.init)
        if parts.isEmpty { data = value.dictionaryValue ?? [:]; return }
        func put(_ node: [String: AnyCodable], _ i: Int) -> [String: AnyCodable] {
            var n = node
            n[parts[i]] = i == parts.count - 1 ? value : .dictionary(put(node[parts[i]]?.dictionaryValue ?? [:], i + 1))
            return n
        }
        data = put(data, 0)
    }

    // MARK: what a control does

    /// A control raising an action (a Button).
    public func dispatchAction(name: String, sourceComponentId: String, context: [String: AnyCodable] = [:]) {
        onAction?(A2UIAction(name: name, sourceComponentId: sourceComponentId, context: context))
    }

    /// A control writing a value: it shows at once, and the server is told.
    public func set(_ path: String, _ value: AnyCodable) {
        guard get(path) != value else { return }
        write(path, value)
        onChange?(A2UIChange(path: path, value: value))
    }
}

/// The path a control writes: the one its `value` is bound to, else `/<id>`.
@MainActor
func a2uiPath(of instance: ComponentInstance, in surface: SurfaceModel) -> String {
    if let p = surface.components[instance.id]?.properties["value"]?.dictionaryValue?["path"]?.stringValue { return p }
    return "/" + instance.id
}
