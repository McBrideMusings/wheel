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
    /// The number a mounted view (one that binds a `ui.state` or `ui.computed`) gave the node, which its later `ViewOp`s name.
    public var handle: Int?
    /// The node is drawn already and nothing in it changed: its props and children are left out.
    public var keep: Bool

    public init(component: String, props: [String: JSONValue] = [:], children: [PaneNode] = [], handle: Int? = nil, keep: Bool = false) {
        self.component = component
        self.props = props
        self.children = children
        self.handle = handle
        self.keep = keep
    }

    /// The node `value` describes, or nil when it is not an object with a `component` name, or
    /// one of its children is not. `__node` and `__keep` are a mounted view's handle and keep mark.
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
        var handle: Int?
        if case let .number(n)? = fields.removeValue(forKey: "__node") { handle = Int(n) }
        let keep = fields.removeValue(forKey: "__keep") == .bool(true)
        self.init(component: name, props: fields, children: children, handle: handle, keep: keep)
    }
}

/// A write a mounted view makes after a state it bound changed, applied to WheelUI's `ViewTree` by handle.
public enum ViewOp: Equatable, Sendable {
    /// One property of one node; `.null` takes the property away.
    case set(node: Int, prop: String, value: JSONValue)
    /// A node's children, in order: a `keep` node is one drawn already, any other is new and complete.
    case children(node: Int, children: [PaneNode])
}
