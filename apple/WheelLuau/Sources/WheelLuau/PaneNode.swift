import WheelKit

/// One node of the tree a pane script's `view` returns: a WheelUI component under its own name
/// (an A2UI basic-catalog component, a SwiftUI control, or `Canvas`; ADR-0031), its properties
/// (`modifiers` among them, ADR-0038), and the nodes in its array part.
public struct PaneNode: Equatable, Sendable {
    public var component: String
    /// Every property but `component` and `children`. A property may hold nodes anywhere inside
    /// it (a Button's `child`, a Table cell's `view`); WheelUI's `ViewTree` gives those their own ids.
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
}
