import WheelKit

/// One node of the tree a pane script's `view` returns: a WheelUI component under its own name
/// (an A2UI basic-catalog component, a SwiftUI control, or `Canvas`; ADR-0031), its properties
/// (`modifiers` among them, ADR-0038), and the nodes in its array part.
public struct PaneNode: Equatable, Sendable {
    public var component: String
    /// Every property but `component` and `children`. A property may hold nodes anywhere inside
    /// it (a Button's `child`, a Table cell's `view`); `a2uiComponents` gives those their own ids.
    public var props: [String: JSONValue]
    public var children: [PaneNode]

    public init(component: String, props: [String: JSONValue] = [:], children: [PaneNode] = []) {
        self.component = component
        self.props = props
        self.children = children
    }

    /// The node `value` describes, or nil when it is not an object with a `component` name, or
    /// one of its children is not.
    public init?(_ value: JSONValue) {
        guard case var .object(fields) = value, case let .string(name)? = fields.removeValue(forKey: "component") else {
            return nil
        }
        var children: [PaneNode] = []
        switch fields.removeValue(forKey: "children") {
        case nil: break
        case let .array(list)?:
            for item in list {
                guard let child = PaneNode(item) else { return nil }
                children.append(child)
            }
        default: return nil
        }
        self.init(component: name, props: fields, children: children)
    }

    /// The tree as the flat component list of an A2UI `updateComponents` message, the form
    /// WheelUI's surface draws: each node gets an id (`root`, then the parent's id and the
    /// child's place, `root.0.2`), `children` becomes a list of ids, and every node inside a
    /// property is replaced by its id (`root.1@child`). The generated `id` and
    /// `component` win over properties of those names.
    public func a2uiComponents(rootID: String = "root") -> [JSONValue] {
        var out: [JSONValue] = []
        flatten(id: rootID, into: &out)
        return out
    }

    private func flatten(id: String, into out: inout [JSONValue]) {
        var fields: [String: JSONValue] = [:]
        let slot = out.count
        out.append(.null)
        for (key, value) in props.sorted(by: { $0.key < $1.key }) {
            fields[key] = Self.lift(value, id: "\(id)@\(key)", into: &out)
        }
        fields["id"] = .string(id)
        fields["component"] = .string(component)
        if !children.isEmpty {
            fields["children"] = .array(children.enumerated().map { i, child in
                let childID = "\(id).\(i)"
                child.flatten(id: childID, into: &out)
                return .string(childID)
            })
        }
        out[slot] = .object(fields)
    }

    /// A property's value with every node in it, at any depth, replaced by its id. The id grows
    /// by `.<i>` for a list item and `@<key>` for a table field: a Button's `child` is
    /// `root.1@child`, a Tabs tab's `child` `root@tabs.0@child`, a Table cell's `view`
    /// `root@sections.0@rows.0@cells.0@view`.
    private static func lift(_ value: JSONValue, id: String, into out: inout [JSONValue]) -> JSONValue {
        if let node = PaneNode(value) {
            node.flatten(id: id, into: &out)
            return .string(id)
        }
        switch value {
        case let .array(list):
            return .array(list.enumerated().map { i, item in lift(item, id: "\(id).\(i)", into: &out) })
        case let .object(fields):
            var lifted = fields
            for (key, field) in fields.sorted(by: { $0.key < $1.key }) {
                lifted[key] = lift(field, id: "\(id)@\(key)", into: &out)
            }
            return .object(lifted)
        default:
            return value
        }
    }
}
