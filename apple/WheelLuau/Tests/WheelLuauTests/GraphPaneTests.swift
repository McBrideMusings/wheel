import Foundation
import Testing
import WheelKit
@testable import WheelLuau

/// The Graph pane (`plugins/wheelhouse/panes/graph.luau`) run through the pane runtime against a fake
/// `wheelhouse.beads` feed, with the pointer events the Canvas stage raises sent to `update`.
@Suite struct GraphPaneTests {
    static let package = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("plugins/wheelhouse")

    static func modules() throws -> [String: String] {
        var out: [String: String] = [:]
        let lib = package.appendingPathComponent("lib")
        for name in try FileManager.default.contentsOfDirectory(atPath: lib.path) where name.hasSuffix(".luau") {
            out["lib/\(name)"] = try String(contentsOf: lib.appendingPathComponent(name), encoding: .utf8)
        }
        return out
    }

    static let data: JSONValue = .object(["pane": .string("p1"), "space": .string("s1"), "cwd": .string("/p")])

    static func bead(_ id: String, status: String = "open", blockedBy: [String] = [], ready: Bool = false) -> JSONValue {
        .object([
            "id": .string(id), "title": .string("Title of \(id)"), "status": .string(status), "kind": .string("task"),
            "priority": .number(2), "blocked_by": .array(blockedBy.map(JSONValue.string)), "description": .string("About \(id)"),
            "ready": .bool(ready),
        ])
    }

    static func pane(_ beads: [JSONValue]) throws -> PaneRuntime {
        let feed: JSONValue = .object(["beads": .array(beads)])
        let host = PaneRuntime.Host(feed: { _, _ in PaneRuntime.FeedRead(value: feed) }, action: { _, _ in })
        let source = try String(contentsOf: package.appendingPathComponent("panes/graph.luau"), encoding: .utf8)
        return try PaneRuntime(source: source, modules: modules(), host: host)
    }

    /// Open beads `a` ← `b` ← `c` (b waits on a, c on b) and one closed bead that stays out of the drawing.
    static func chain() throws -> PaneRuntime {
        try pane([bead("a", ready: true), bead("b", blockedBy: ["a"]), bead("c", blockedBy: ["b"]), bead("z", status: "closed")])
    }

    static func canvas(_ node: PaneNode) -> PaneNode? {
        if node.component == "Canvas" { return node }
        for child in node.children { if let c = canvas(child) { return c } }
        return nil
    }

    static func texts(_ node: PaneNode) -> [String] {
        var out: [String] = []
        if case let .string(t)? = node.props["text"] { out.append(t) }
        for child in node.children { out += texts(child) }
        return out
    }

    static func commands(_ node: PaneNode) -> [[JSONValue]] {
        guard let c = canvas(node), case let .array(list)? = c.props["commands"] else { return [] }
        return list.compactMap { if case let .array(a) = $0 { a } else { nil } }
    }

    static func st(_ runtime: PaneRuntime) -> JSONValue { (try? runtime.state) ?? .null }

    static func field(_ state: JSONValue, _ key: String) -> JSONValue? {
        if case let .object(o) = state { o[key] } else { nil }
    }

    static func number(_ state: JSONValue, _ key: String) -> Double? {
        if case let .number(n)? = field(state, key) { n } else { nil }
    }

    /// The top-left of node `id` in the drawing, from the state the pane shows a check.
    static func position(_ runtime: PaneRuntime, _ id: String) throws -> (x: Double, y: Double) {
        guard case let .object(layout)? = Self.field(Self.st(runtime), "layout"), case let .object(at)? = layout[id],
              case let .number(x)? = at["x"], case let .number(y)? = at["y"] else { throw Failure("no position for \(id)") }
        return (x, y)
    }

    struct Failure: Error, CustomStringConvertible {
        var description: String
        init(_ d: String) { description = d }
    }

    static func pointer(_ runtime: PaneRuntime, _ kind: String, _ x: Double, _ y: Double, dx: Double = 0, dy: Double = 0) throws {
        try runtime.send(.object(["type": .string("pointer"), "kind": .string(kind), "x": .number(x), "y": .number(y), "button": .number(0), "dx": .number(dx), "dy": .number(dy)]))
    }

    @Test func drawsOpenBeadsAndTheirEdgesLeftToRightByDependency() throws {
        let runtime = try Self.chain()
        let tree = try runtime.view(Self.data)
        #expect(Self.texts(tree).contains("3 open issues · 2 dependencies"))
        let curves = Self.commands(tree).filter { $0.first == .string("bezierCurveTo") }
        #expect(curves.count == 2)
        let (a, b, c) = (try Self.position(runtime, "a"), try Self.position(runtime, "b"), try Self.position(runtime, "c"))
        #expect(a.x < b.x && b.x < c.x, "a blocker sits left of what it blocks: \(a) \(b) \(c)")
        #expect(Self.field(Self.st(runtime), "nodes") == .number(3))
        if case let .object(layout)? = Self.field(Self.st(runtime), "layout") { #expect(layout["z"] == nil) }
    }

    @Test func hoverHighlightsTheNodeAndItsEdges() throws {
        let runtime = try Self.chain()
        let before = Self.commands(try runtime.view(Self.data))
        let a = try Self.position(runtime, "a")
        try Self.pointer(runtime, "move", a.x + 20, a.y + 15)
        let after = Self.commands(try runtime.view(Self.data))
        #expect(Self.field(Self.st(runtime), "hover") == .string("a"))
        #expect(before != after)
        let lit = after.filter { $0.count == 3 && $0[1] == .string("strokeStyle") && $0[2] == .string("accent") }
        #expect(lit.count >= 1, "the hovered node's edge is stroked in the accent tone")
        // Off every node clears it.
        try Self.pointer(runtime, "move", 900, 480)
        _ = try runtime.view(Self.data)
        #expect(Self.field(Self.st(runtime), "hover") == .null || Self.field(Self.st(runtime), "hover") == nil)
    }

    @Test func aClickOpensTheBeadAndCloseShutsIt() throws {
        let runtime = try Self.chain()
        _ = try runtime.view(Self.data)
        let b = try Self.position(runtime, "b")
        try Self.pointer(runtime, "down", b.x + 20, b.y + 15)
        try Self.pointer(runtime, "up", b.x + 20, b.y + 15)
        let tree = try runtime.view(Self.data)
        #expect(Self.field(Self.st(runtime), "open") == .string("b"))
        #expect(Self.texts(tree).contains("Blocked by a · Blocks c"))
        try runtime.send(.object(["type": .string("close")]))
        _ = try runtime.view(Self.data)
        #expect(Self.field(Self.st(runtime), "open") == .null || Self.field(Self.st(runtime), "open") == nil)
    }

    @Test func aDragPansAndOpensNothing() throws {
        let runtime = try Self.chain()
        _ = try runtime.view(Self.data)
        let a = try Self.position(runtime, "a")
        // A drag that starts on a node pans too, and its release is not a click.
        try Self.pointer(runtime, "down", a.x + 20, a.y + 15)
        try Self.pointer(runtime, "drag", a.x + 60, a.y + 40, dx: 40, dy: 25)
        try Self.pointer(runtime, "up", a.x + 60, a.y + 40)
        let tree = try runtime.view(Self.data)
        #expect(Self.number(Self.st(runtime), "ox") == 40 && Self.number(Self.st(runtime), "oy") == 25)
        #expect(Self.field(Self.st(runtime), "open") == .null || Self.field(Self.st(runtime), "open") == nil)
        // The drawing is translated by the pan.
        #expect(Self.commands(tree).contains { $0.first == .string("translate") && $0.count == 3 && $0[1] == .number(40) && $0[2] == .number(25) })
        // A tap inside the slop is a click, not a pan.
        try Self.pointer(runtime, "down", a.x + 60, a.y + 40)
        try Self.pointer(runtime, "drag", a.x + 62, a.y + 41, dx: 2, dy: 1)
        try Self.pointer(runtime, "up", a.x + 62, a.y + 41)
        _ = try runtime.view(Self.data)
        #expect(Self.number(Self.st(runtime), "ox") == 40 && Self.field(Self.st(runtime), "open") == .string("a"))
    }

    @Test func theFirstDragPastTheSlopPansByEverythingSinceThePress() throws {
        let runtime = try Self.chain()
        _ = try runtime.view(Self.data)
        try Self.pointer(runtime, "down", 700, 400)
        try Self.pointer(runtime, "drag", 702, 400, dx: 2, dy: 0)
        #expect(Self.number(Self.st(runtime), "ox") == 0)
        try Self.pointer(runtime, "drag", 705, 400, dx: 3, dy: 0)
        #expect(Self.number(Self.st(runtime), "ox") == 5)
        try Self.pointer(runtime, "drag", 707, 401, dx: 2, dy: 1)
        #expect(Self.number(Self.st(runtime), "ox") == 7 && Self.number(Self.st(runtime), "oy") == 1)
    }

    @Test func aCycleStillLaysOut() throws {
        let runtime = try Self.pane([Self.bead("a", blockedBy: ["b"]), Self.bead("b", blockedBy: ["a"])])
        let tree = try runtime.view(Self.data)
        #expect(Self.texts(tree).contains("2 open issues · 2 dependencies"))
    }

    @Test func noOpenBeadsSaysSoAndDrawsNoCanvas() throws {
        let runtime = try Self.pane([Self.bead("z", status: "closed")])
        let tree = try runtime.view(Self.data)
        #expect(Self.texts(tree).contains("No open issues"))
        #expect(Self.canvas(tree) == nil)
    }
}
