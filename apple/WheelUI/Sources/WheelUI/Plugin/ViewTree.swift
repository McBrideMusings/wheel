import Foundation
import Observation
import WheelKit
internal import WheelLuau

// A plugin pane's view as WheelUI holds it: one `ViewNode` per component the script's `view` returned, kept by identity across
// renders. `ViewTree.reconcile` takes each new `view` and writes only what changed, so SwiftUI redraws only the components whose
// node changed (each component view observes its own node).
//
// Identity: the root is `root`; a child is `<parent>/<key>` when it has a `key`, else `<parent>/<Component><n>` with n its
// place among siblings of the same component; a component inside a property is `<parent>@<property path>` (`root@child`,
// `root/Table0/b-12@cells.2.view`); a Table's row is `<table>/<row path, else row id, else section.row>`. A node whose
// component changes is built again.

/// One component of the view, observed by the view that draws it.
@MainActor @Observable
public final class ViewNode: Identifiable {
    public let id: String
    public let component: String
    /// The component's properties as the script wrote them, with every component inside a property replaced by its id
    /// (`.string(<id>)`), a Table's `sections[].rows` by its row ids, and a bound control's `value` taken out (`draft`).
    public internal(set) var props: [String: JSONValue] = [:]
    public internal(set) var children: [ViewNode] = []
    /// The components inside properties, by property path (`child`, `trigger`, `content`, `header`, `actions.0`, `cells.2.view`).
    public internal(set) var embedded: [String: ViewNode] = [:]
    /// A Table's rows, in order, each a `TableRow` node whose props are the row the script wrote.
    public internal(set) var rows: [ViewNode] = []
    /// What a control that holds a value shows: the last value the person or the script gave it.
    public internal(set) var draft: JSONValue?
    /// The script's `value` at the last render, for a bound control: a new one replaces the draft, the same one leaves it.
    @ObservationIgnored var given: JSONValue?
    /// A Table row's own `id` field, read when a selection names rows.
    @ObservationIgnored public internal(set) var rowID = ""

    init(id: String, component: String) {
        self.id = id
        self.component = component
    }

    /// The nodes drawn as this component's content: its children, then its `child`, `trigger` and `content`.
    public var parts: [ViewNode] {
        children + ["child", "trigger", "content"].compactMap { embedded[$0] }
    }

    /// What a control shows: its draft, else the `value` the script gave.
    public var value: JSONValue? { draft ?? props["value"] }

    func string(_ key: String) -> String? { props[key]?.stringValue }
    func number(_ key: String) -> Double? { props[key]?.numberValue }
    func bool(_ key: String) -> Bool? { props[key]?.boolValue }

    /// `key` when the property holds a table the component raises, else "".
    func action(_ key: String) -> String {
        if case .object = props[key] { key } else { "" }
    }
}

@MainActor @Observable
public final class ViewTree {
    public private(set) var root: ViewNode?
    /// The Modals with `presented` set, drawn over the pane.
    public private(set) var modals: [ViewNode] = []
    /// Whether the view takes the pane's height itself (a frame-less Table or vertical List), so the pane has no ScrollView.
    public private(set) var fillsPane = false
    /// Every node the last render reached, by id.
    @ObservationIgnored private var index: [String: ViewNode] = [:]
    /// A table a control raised, with its event fields added: the pane sends it to the script's `update`.
    @ObservationIgnored public var onRaise: ((JSONValue) -> Void)?
    /// A command Canvas measured itself: its `canvasKey`, width and height.
    @ObservationIgnored public var onCanvasSize: ((String, Double, Double) -> Void)?

    /// Nodes the last reconcile or `set` created or wrote.
    @ObservationIgnored public private(set) var written = 0
    /// The first duplicate `key` the last reconcile met, as "<parent> <key>".
    @ObservationIgnored public private(set) var duplicate: String?
    @ObservationIgnored private var touched: Set<String> = []
    @ObservationIgnored private var reached: [String: ViewNode] = [:]

    public init() {}

    public func node(_ id: String) -> ViewNode? { index[id] }

    /// Every node the last render reached, in no order.
    var all: [ViewNode] { Array(index.values) }

    // MARK: reconcile

    /// Makes the tree match `tree`: a node whose identity survives keeps its object and its draft; a property, child list or
    /// embedded map is assigned only when it differs; a node no longer reached is dropped.
    func reconcile(_ tree: PaneNode) {
        touched = []
        reached = [:]
        duplicate = nil
        let r = place(tree, id: "root", old: root)
        if root !== r { root = r }
        index = reached
        reached = [:]
        let presented = index.values.filter { $0.component == "Modal" && $0.props["presented"]?.boolValue == true }.sorted { $0.id < $1.id }
        if presented.map(\.id) != modals.map(\.id) || zip(presented, modals).contains(where: { $0 !== $1 }) { modals = presented }
        let fills = root.map(Self.fills) ?? false
        if fills != fillsPane { fillsPane = fills }
        written = touched.count
    }

    /// The node for `pane` at `id`: `old` when it is the same component, else a new one; then its contents are brought up to date.
    private func place(_ pane: PaneNode, id: String, old: ViewNode?) -> ViewNode {
        let node: ViewNode
        if let old, old.id == id, old.component == pane.component { node = old } else {
            node = ViewNode(id: id, component: pane.component)
            touched.insert(id)
        }
        reached[id] = node
        var embedded: [String: ViewNode] = [:]
        var props: [String: JSONValue] = [:]
        // A Table's sections are its rows' own: each row node lifts the components in its cells.
        for (key, value) in pane.props {
            props[key] = pane.component == "Table" && key == "sections" ? value : lift(value, host: node, path: key, into: &embedded)
        }
        var rows: [ViewNode] = []
        if pane.component == "Table" { props["sections"] = tableRows(props["sections"], table: node, into: &rows) }
        if pane.component == "TextField" || props["on_change"] != nil {
            let value = props.removeValue(forKey: "value")
            if let value, value != node.given { write(node, draft: value) }
            else if value == nil, node.given == nil, node.draft == nil, pane.component == "TextField" { write(node, draft: .string("")) }
            node.given = value
        }
        if node.props != props { node.props = props; touched.insert(id) }
        if !Self.same(node.rows, rows) { node.rows = rows; touched.insert(id) }
        if node.embedded.count != embedded.count || embedded.contains(where: { node.embedded[$0.key] !== $0.value }) {
            node.embedded = embedded
            touched.insert(id)
        }
        let children = placeChildren(pane.children, parent: node)
        if !Self.same(node.children, children) { node.children = children; touched.insert(id) }
        return node
    }

    private func placeChildren(_ panes: [PaneNode], parent: ViewNode) -> [ViewNode] {
        var old: [String: ViewNode] = [:]
        for c in parent.children { old[c.id] = c }
        var counts: [String: Int] = [:]
        var keys: Set<String> = []
        var out: [ViewNode] = []
        for pane in panes {
            let id: String
            if let key = pane.props["key"].map(Self.keyText) {
                guard keys.insert(key).inserted else {
                    if duplicate == nil { duplicate = "\(parent.id) \(key)" }
                    UILog.info("viewtree duplicate key \(parent.id) \(key)")
                    continue
                }
                id = "\(parent.id)/\(key)"
            } else {
                let n = counts[pane.component, default: 0]
                counts[pane.component] = n + 1
                id = "\(parent.id)/\(pane.component)\(n)"
            }
            out.append(place(pane, id: id, old: old[id]))
        }
        return out
    }

    /// A property's value with every component in it, at any depth, placed as an embedded node and replaced by its id.
    private func lift(_ value: JSONValue, host: ViewNode, path: String, into embedded: inout [String: ViewNode]) -> JSONValue {
        if let pane = PaneNode(value) {
            let id = "\(host.id)@\(path)"
            embedded[path] = place(pane, id: id, old: host.embedded[path])
            return .string(id)
        }
        switch value {
        case let .array(items):
            return .array(items.enumerated().map { lift($1, host: host, path: "\(path).\($0)", into: &embedded) })
        case let .object(fields):
            var out = fields
            for (k, v) in fields { out[k] = lift(v, host: host, path: "\(path).\(k)", into: &embedded) }
            return .object(out)
        default:
            return value
        }
    }

    /// A Table's sections with each row replaced by its row node's id; the rows are placed in `rows`, in order.
    private func tableRows(_ sections: JSONValue?, table: ViewNode, into rows: inout [ViewNode]) -> JSONValue? {
        guard case let .array(list)? = sections else { return sections }
        var old: [String: ViewNode] = [:]
        for r in table.rows { old[r.id] = r }
        var seen: Set<String> = []
        return .array(list.enumerated().map { si, section in
            guard case var .object(sec) = section, case let .array(items)? = sec["rows"] else { return section }
            sec["rows"] = .array(items.enumerated().map { ri, item in
                guard case let .object(row) = item else { return item }
                var key = row["path"]?.stringValue ?? row["id"]?.stringValue ?? "\(si).\(ri)"
                if !seen.insert(key).inserted {
                    UILog.info("viewtree duplicate row \(table.id) \(key)")
                    key = "\(key)#\(si).\(ri)"
                    seen.insert(key)
                }
                let id = "\(table.id)/\(key)"
                let node = place(PaneNode(component: "TableRow", props: row), id: id, old: old[id])
                node.rowID = row["id"]?.stringValue ?? ""
                rows.append(node)
                return .string(id)
            })
            return .object(sec)
        })
    }

    private func write(_ node: ViewNode, draft: JSONValue) {
        guard node.draft != draft else { return }
        node.draft = draft
        touched.insert(node.id)
    }

    private static func keyText(_ v: JSONValue) -> String {
        switch v {
        case let .string(s): s
        case let .number(n): n == n.rounded() ? String(Int(n)) : String(n)
        default: v.text
        }
    }

    private static func same(_ a: [ViewNode], _ b: [ViewNode]) -> Bool {
        a.count == b.count && zip(a, b).allSatisfy { $0 === $1 }
    }

    /// A Table or a vertical List with no `frame` modifier fills the pane, so the page around it stays put and that component
    /// scrolls itself.
    private static func fills(_ n: ViewNode) -> Bool {
        let scrolls = n.component == "Table" || (n.component == "List" && n.props["direction"]?.stringValue != "horizontal")
        if scrolls, !(n.props["modifiers"]?.arrayValue ?? []).contains(where: { $0.dictionaryValue?["frame"] != nil }) { return true }
        return n.parts.contains(where: fills)
    }

    /// The script restarted: its next render's values all count as new, so each replaces what its control shows.
    func forgetGiven() {
        for node in index.values { node.given = nil }
    }

    // MARK: writes from outside a render

    /// Writes one property of one node, as a fine-grained script does when a value it bound changes; nothing when it is equal.
    /// A bound control's `value` follows the render rule: a value the script did not give last replaces the draft.
    public func set(node: ViewNode, prop: String, value: JSONValue?) {
        touched = []
        if prop == "value", node.component == "TextField" || node.props["on_change"] != nil {
            if let value, value != node.given { write(node, draft: value) }
            node.given = value
        } else if node.props[prop] != value {
            node.props[prop] = value
            touched.insert(node.id)
        }
        written = touched.count
    }

    // MARK: what a control does

    /// Sends the table at `keyPath` of `node`'s props (`action`, `rowAction`, `hits.3.doubleAction`, `menu.1.action`) to the
    /// script, with each of `fields` the table lacks added. False, with a log line, when the node has left the view or holds no
    /// table there.
    @discardableResult
    public func raise(_ node: ViewNode, _ keyPath: String, _ fields: [String: JSONValue] = [:]) -> Bool {
        guard !keyPath.isEmpty else { return false }
        guard index[node.id] === node, case var .object(msg)? = Self.lookup(node.props, keyPath) else {
            UILog.info("viewtree raise missing \(node.id) \(keyPath)")
            return false
        }
        for (k, v) in fields where msg[k] == nil { msg[k] = v }
        onRaise?(.object(msg))
        return true
    }

    /// Does what pressing `node` does: its `action` `{copy = {text}}` goes to the pasteboard (true); any other `action` table is
    /// raised (false).
    @discardableResult
    public func activate(_ node: ViewNode) -> Bool {
        if case let .object(action)? = node.props["action"], let text = action["copy"]?.dictionaryValue?["text"]?.stringValue {
            copyToPasteboard(text)
            return true
        }
        if !node.action("action").isEmpty { raise(node, "action") }
        return false
    }

    /// A control writing a value: it shows at once, and its `on_change` table goes to the script with `value` set.
    public func change(_ node: ViewNode, _ value: JSONValue) {
        guard node.value != value else { return }
        node.draft = value
        if case var .object(msg)? = node.props["on_change"] {
            msg["value"] = value
            onRaise?(.object(msg))
        }
    }

    static func lookup(_ props: [String: JSONValue], _ keyPath: String) -> JSONValue? {
        var cur: JSONValue? = .object(props)
        for part in keyPath.split(separator: ".") {
            switch cur {
            case let .object(o)?: cur = o[String(part)]
            case let .array(a)?: cur = Int(part).flatMap { a.indices.contains($0) ? a[$0] : nil }
            default: return nil
            }
        }
        return cur
    }
}

public extension JSONValue {
    var stringValue: String? { if case let .string(s) = self { s } else { nil } }
    var numberValue: Double? { if case let .number(n) = self { n } else { nil } }
    var boolValue: Bool? { if case let .bool(b) = self { b } else { nil } }
    var arrayValue: [JSONValue]? { if case let .array(a) = self { a } else { nil } }
    var dictionaryValue: [String: JSONValue]? { if case let .object(o) = self { o } else { nil } }
    /// The value as text, for a control that shows whatever it was given.
    var text: String {
        switch self {
        case let .string(s): s
        case let .number(n): n == n.rounded() ? String(Int(n)) : String(n)
        case let .bool(b): String(b)
        default: ""
        }
    }
}
