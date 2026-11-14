import Foundation
import Testing
import WheelKit
@testable import WheelLuau

/// The wheelhouse plugin's pane scripts (`plugins/wheelhouse/panes/beads.luau`, `diff.luau` and the
/// `lib/` modules they share) run through the pane runtime against fixture feeds, with the
/// messages a client sends. The checks read the flattened tree the way WheelUI draws it.
/// `tools/wheelhouse-panes.sh` runs this suite.
@Suite struct WheelhousePaneTests {
    static let package = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("plugins/wheelhouse")

    // MARK: harness

    struct Failure: Error, CustomStringConvertible {
        var description: String
        init(_ d: String) { description = d }
    }

    static func modules() throws -> [String: String] {
        var out: [String: String] = [:]
        let lib = package.appendingPathComponent("lib")
        for name in try FileManager.default.contentsOfDirectory(atPath: lib.path) where name.hasSuffix(".luau") {
            out["lib/\(name)"] = try String(contentsOf: lib.appendingPathComponent(name), encoding: .utf8)
        }
        return out
    }

    static func json(_ text: String) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
    }

    static func msg(_ type: String, _ fields: [String: JSONValue] = [:]) -> JSONValue {
        var o = fields
        o["type"] = .string(type)
        return .object(o)
    }

    final class Recorder: @unchecked Sendable {
        var actions: [(String, JSONValue)] = []
    }

    /// A pane over `pane` (`beads` or `diff`) answering `wheelhouse.<feed>` with `value` and
    /// `wheel.panes` with `panes` (null: the open panes are not known).
    static func pane(_ name: String, feed: String, value: JSONValue, panes: JSONValue = .null,
                     rec: Recorder = Recorder()) throws -> PaneRuntime {
        let host = PaneRuntime.Host(
            feed: { n, _ in
                switch n {
                case "wheelhouse.\(feed)": return PaneRuntime.FeedRead(value: value)
                case "wheel.panes": return PaneRuntime.FeedRead(value: panes)
                default: return PaneRuntime.FeedRead(value: .null)
                }
            },
            action: { n, a in rec.actions.append((n, a)) })
        let source = try String(contentsOf: package.appendingPathComponent("panes/\(name).luau"), encoding: .utf8)
        return try PaneRuntime(source: source, modules: modules(), host: host)
    }

    static let data: JSONValue = .object(["pane": .string("p1"), "space": .string("s1"), "cwd": .string("/p")])

    /// The tree as JSON: each node an object with its props, its `component` and its `children`.
    static func jsonOf(_ n: PaneNode) -> JSONValue {
        var o = n.props
        o["component"] = .string(n.component)
        o["children"] = .array(n.children.map(jsonOf))
        return .object(o)
    }

    /// Every object below `v` (v included) that names a component, depth first.
    static func nodes(_ v: JSONValue, _ out: inout [[String: JSONValue]]) {
        switch v {
        case let .object(o):
            if case .string? = o["component"] { out.append(o) }
            for key in o.keys.sorted() { nodes(o[key]!, &out) }
        case let .array(a): for x in a { nodes(x, &out) }
        default: break
        }
    }

    static func all(_ tree: PaneNode, _ component: String) -> [[String: JSONValue]] {
        var out: [[String: JSONValue]] = []
        nodes(jsonOf(tree), &out)
        return out.filter { $0["component"] == .string(component) }
    }

    static func str(_ v: JSONValue?) -> String? {
        if case let .string(s)? = v { return s }
        return nil
    }

    static func texts(_ v: JSONValue) -> [String] {
        var out: [JSONValue] = []
        var flat: [[String: JSONValue]] = []
        nodes(v, &flat)
        for o in flat { if let t = o["text"] { out.append(t) } }
        return out.compactMap { str($0) }
    }

    /// The listed table's rows: each row's id and its title cell.
    static func rows(_ tree: PaneNode) throws -> [(id: String, title: String, row: [String: JSONValue])] {
        guard let table = all(tree, "Table").first, case let .array(sections)? = table["sections"],
              case let .object(s)? = sections.first, case let .array(rows)? = s["rows"] else {
            throw Failure("no table")
        }
        return rows.compactMap { r in
            guard case let .object(o) = r, let id = str(o["id"]) else { return nil }
            var title = ""
            if case let .array(cells)? = o["cells"] {
                for case let .object(c) in cells where str(c["key"]) == "title" { title = str(c["text"]) ?? "" }
            }
            return (id, title, o)
        }
    }

    static func ids(_ runtime: PaneRuntime) throws -> [String] {
        try rows(runtime.view(data)).map(\.id)
    }

    struct Btn {
        var text: String
        var action: JSONValue
        var disabled: Bool
        var help: String
    }

    /// Every Button below `tree` (a Modal's footer and a Popover's body included).
    static func buttons(_ tree: PaneNode) -> [Btn] {
        all(tree, "Button").map { b in
            var disabled = false, help = ""
            if case let .array(mods)? = b["modifiers"] {
                for case let .object(m) in mods {
                    if m["disabled"] == .bool(true) { disabled = true }
                    if let h = str(m["help"]) { help = h }
                }
            }
            var text = ""
            if let child = b["child"] { text = texts(child).first ?? "" }
            return Btn(text: text, action: b["action"] ?? .null, disabled: disabled, help: help)
        }
    }

    static func actionType(_ b: Btn) -> String? {
        if case let .object(o) = b.action { return str(o["type"]) }
        return nil
    }

    // MARK: the beads fixture

    static func bead(_ id: String, _ status: String = "open", kind: String = "task", pri: Int? = 2, parent: String? = nil,
                     labels: [String] = [], blocked: [String] = [], ready: Bool = false, epic: Bool = false,
                     title: String? = nil, desc: String = "", updated: String = "2026-11-01T00:00:00Z") -> String {
        func q(_ s: String) -> String { "\"\(s)\"" }
        let p = pri.map(String.init) ?? "null"
        let par = parent.map(q) ?? "null"
        return """
        {"id":\(q(id)),"title":\(q(title ?? "Bead \(id)")),"status":\(q(status)),"kind":\(q(kind)),"priority":\(p),\
        "parent":\(par),"labels":[\(labels.map(q).joined(separator: ","))],"owner":"pierce","updated":\(q(updated)),\
        "blocked_by":[\(blocked.map(q).joined(separator: ","))],"description":\(q(desc)),"ready":\(ready),"epic":\(epic)}
        """
    }

    static func snapshot(_ beads: [String], buttons: String = "{\"task\":null,\"epic\":null,\"closed\":null}",
                         records: String = "{}", pressing: String = "[]", scripts: String = "[]") throws -> JSONValue {
        try json("""
        {"now":1793500000,"name":"wheel","path":"/p","has_repo":true,"error":null,"problems":[],"buttons":\(buttons),\
        "scripts":\(scripts),"records":\(records),"pressing":\(pressing),"beads":[\(beads.joined(separator: ","))]}
        """)
    }

    /// Ten leaves and an epic. By status word: wheel-1.2 in progress; wheel-1.1, -9, -10, -6 ready;
    /// wheel-7, -8, -11 blocked (7 and 8 wait on each other); wheel-2 open; wheel-1.3 done.
    static func small() throws -> JSONValue {
        try snapshot([
            bead("wheel-1", kind: "epic", pri: 1, epic: true),
            bead("wheel-1.1", kind: "task", pri: 1, parent: "wheel-1", labels: ["client"], ready: true),
            bead("wheel-1.2", "in_progress", pri: 2, parent: "wheel-1", labels: ["daemon"]),
            bead("wheel-1.3", "closed", pri: 3, parent: "wheel-1"),
            bead("wheel-9", pri: 1, labels: ["client"], ready: true),
            bead("wheel-10", kind: "bug", pri: 1, labels: ["client", "daemon"], ready: true),
            bead("wheel-2", pri: 2, desc: "flaky socket"),
            bead("wheel-6", pri: 2, ready: true),
            bead("wheel-7", pri: 2, blocked: ["wheel-6", "wheel-8"]),
            bead("wheel-8", pri: 2, blocked: ["wheel-7"]),
            bead("wheel-11", pri: 2, blocked: ["wheel-2"]),
        ])
    }

    static func listPane(_ value: JSONValue? = nil, panes: JSONValue = .null, rec: Recorder = Recorder()) throws -> PaneRuntime {
        try pane("beads", feed: "beads", value: value ?? small(), panes: panes, rec: rec)
    }

    // MARK: list.luau: sort, natural ids, ties

    @Test func theDefaultListIsSortedByStatusWithTiesInFeedOrderAndDoneHidden() throws {
        let runtime = try Self.listPane()
        #expect(try Self.ids(runtime) == ["wheel-1.2", "wheel-1.1", "wheel-9", "wheel-10", "wheel-6", "wheel-7", "wheel-8", "wheel-11", "wheel-2"])
    }

    @Test func aDescendingSortKeepsTiedRowsInFeedOrder() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("list.sort", ["key": .string("status"), "asc": .bool(false)]))
        #expect(try Self.ids(runtime) == ["wheel-2", "wheel-7", "wheel-8", "wheel-11", "wheel-1.1", "wheel-9", "wheel-10", "wheel-6", "wheel-1.2"])
    }

    @Test func idsSortNaturallyNotAsText() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("list.sort", ["key": .string("id"), "asc": .bool(true)]))
        #expect(try Self.ids(runtime) == ["wheel-1.1", "wheel-1.2", "wheel-2", "wheel-6", "wheel-7", "wheel-8", "wheel-9", "wheel-10", "wheel-11"])
        try runtime.send(Self.msg("list.sort", ["key": .string("id"), "asc": .bool(false)]))
        #expect(try Self.ids(runtime).first == "wheel-11")
    }

    @Test func aSortTieIsBrokenByStatusThenFeedOrder() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("list.sort", ["key": .string("pri"), "asc": .bool(true)]))
        // P1: ready rows in feed order; P2: in progress, ready, blocked in feed order, open last.
        #expect(try Self.ids(runtime) == ["wheel-1.1", "wheel-9", "wheel-10", "wheel-1.2", "wheel-6", "wheel-7", "wheel-8", "wheel-11", "wheel-2"])
    }

    @Test func anUnknownSortKeyChangesNothing() throws {
        let runtime = try Self.listPane()
        let before = try Self.ids(runtime)
        try runtime.send(Self.msg("list.sort", ["key": .string("nope"), "asc": .bool(false)]))
        #expect(try Self.ids(runtime) == before)
    }

    // MARK: list.luau: filters, Is not, search, Clear

    @Test func aTickedFilterKeepsRowsWithOneOfItsValues() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("list.toggle", ["key": .string("label"), "item": .string("client"), "value": .bool(true)]))
        #expect(try Self.ids(runtime) == ["wheel-1.1", "wheel-9", "wheel-10"])
        try runtime.send(Self.msg("list.toggle", ["key": .string("label"), "item": .string("daemon"), "value": .bool(true)]))
        #expect(try Self.ids(runtime) == ["wheel-1.2", "wheel-1.1", "wheel-9", "wheel-10"])
        try runtime.send(Self.msg("list.toggle", ["key": .string("type"), "item": .string("bug"), "value": .bool(true)]))
        #expect(try Self.ids(runtime) == ["wheel-10"])
    }

    @Test func isNotExcludesTheTickedValuesAndTheLastUntickDropsIt() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("list.toggle", ["key": .string("label"), "item": .string("client"), "value": .bool(true)]))
        try runtime.send(Self.msg("list.not", ["key": .string("label"), "value": .bool(true)]))
        #expect(try Self.ids(runtime) == ["wheel-1.2", "wheel-6", "wheel-7", "wheel-8", "wheel-11", "wheel-2"])
        // The filter's popover title says it excludes.
        #expect(try Self.texts(Self.jsonOf(runtime.view(Self.data))).contains("Label not: client ⌄"))
        // Unticking the one value matches every row again and forgets the Is not.
        try runtime.send(Self.msg("list.toggle", ["key": .string("label"), "item": .string("client"), "value": .bool(false)]))
        #expect(try Self.ids(runtime).count == 9)
        try runtime.send(Self.msg("list.toggle", ["key": .string("label"), "item": .string("daemon"), "value": .bool(true)]))
        #expect(try Self.ids(runtime) == ["wheel-1.2", "wheel-10"])
    }

    @Test func theDoneStatusShowsOnlyWhileItsFilterIsSet() throws {
        let runtime = try Self.listPane()
        #expect(try !Self.ids(runtime).contains("wheel-1.3"))
        try runtime.send(Self.msg("list.filter", ["key": .string("status"), "value": .string("done")]))
        #expect(try Self.ids(runtime) == ["wheel-1.3"])
        // A priority filter and a status filter together.
        try runtime.send(Self.msg("list.filter", ["key": .string("status"), "value": .string("ready,done")]))
        try runtime.send(Self.msg("list.toggle", ["key": .string("pri"), "item": .string("1"), "value": .bool(true)]))
        #expect(try Self.ids(runtime) == ["wheel-1.1", "wheel-9", "wheel-10"])
    }

    @Test func searchNeedsEveryWordInTheIdTitleKindLabelsDescriptionOrOwner() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("list.query", ["value": .string("daemon bug")]))
        #expect(try Self.ids(runtime) == ["wheel-10"])
        try runtime.send(Self.msg("list.query", ["value": .string("SOCKET")]))
        #expect(try Self.ids(runtime) == ["wheel-2"])
        try runtime.send(Self.msg("list.query", ["value": .string("pierce wheel-9")]))
        #expect(try Self.ids(runtime) == ["wheel-9"])
        try runtime.send(Self.msg("list.query", ["value": .string("zzz")]))
        #expect(try Self.ids(runtime) == [])
    }

    @Test func clearResetsTheSearchFiltersAndSelection() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("list.query", ["value": .string("daemon")]))
        try runtime.send(Self.msg("list.toggle", ["key": .string("type"), "item": .string("bug"), "value": .bool(true)]))
        try runtime.send(Self.msg("list.not", ["key": .string("type"), "value": .bool(true)]))
        let tree = try runtime.view(Self.data)
        let clear = try #require(try Self.buttons(tree).first { $0.text == "Clear" })
        #expect(!clear.disabled)
        try runtime.send(clear.action)
        #expect(try Self.ids(runtime).count == 9)
        // Clear keeps its slot and is disabled when nothing narrows the list.
        let after = try #require(try Self.buttons(runtime.view(Self.data)).first { $0.text == "Clear" })
        #expect(after.disabled)
    }

    @Test func theStatusBoxesCountAndSetOneFilterAndTapAgainClears() throws {
        let runtime = try Self.listPane()
        var tree = try runtime.view(Self.data)
        let boxes = Self.buttons(tree).filter { Self.actionType($0) == "list.filter" }
        #expect(boxes.map(\.text) == ["4", "1", "3", "1"])
        let ready = boxes[0]
        try runtime.send(ready.action)
        #expect(try Self.ids(runtime) == ["wheel-1.1", "wheel-9", "wheel-10", "wheel-6"])
        // The box that is on now sends "" to clear.
        tree = try runtime.view(Self.data)
        let on = try #require(try Self.buttons(tree).first { Self.actionType($0) == "list.filter" })
        try runtime.send(on.action)
        #expect(try Self.ids(runtime).count == 9)
    }

    @Test func aChangeToTheListClearsTheSelectionAndTheActionsCountWhatIsSelected() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("list.select", ["ids": .array([.string("wheel-9"), .string("wheel-6")])]))
        var chips = Self.texts(Self.jsonOf(try runtime.view(Self.data)))
        #expect(chips.contains("2 selected of 9 · Actions ⌄"))
        let copy = try #require(try Self.buttons(runtime.view(Self.data)).first { $0.text == "Copy IDs" })
        #expect(copy.help.hasPrefix("Copy the 2 selected ids"))
        try runtime.send(Self.msg("list.query", ["value": .string("bead")]))
        chips = Self.texts(Self.jsonOf(try runtime.view(Self.data)))
        #expect(chips.contains("9 listed · Actions ⌄"))
    }

    // MARK: the tree views

    @Test func theEpicsViewHangsChildrenUnderTheirEpicAndFoldsThem() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("set_view", ["value": .array([.string("epics")])]))
        // Children by status rank: in progress, then ready; the closed child stays hidden until the done filter is set.
        var rows = try Self.rows(runtime.view(Self.data))
        #expect(rows.map(\.id) == ["wheel-1", "wheel-1.2", "wheel-1.1"])
        #expect(rows.map { $0.row["depth"] } == [.number(0), .number(1), .number(1)])
        #expect(rows[0].row["kids"] == .bool(true))
        try runtime.send(Self.msg("list.filter", ["key": .string("status"), "value": .string("done,ready,in progress")]))
        #expect(try Self.ids(runtime) == ["wheel-1", "wheel-1.2", "wheel-1.1", "wheel-1.3"])
        try runtime.send(Self.msg("list.clear"))
        try runtime.send(Self.msg("toggle_path", ["path": .string(">wheel-1")]))
        rows = try Self.rows(runtime.view(Self.data))
        #expect(rows.map(\.id) == ["wheel-1"])
        #expect(rows[0].row["collapsed"] == .bool(true))
    }

    @Test func theDependsOnViewRootsAtABeadNothingKeptWaitsOn() throws {
        let value = try Self.snapshot([
            Self.bead("wheel-2", pri: 2),
            Self.bead("wheel-11", pri: 2, blocked: ["wheel-2"]),
            Self.bead("wheel-12", pri: 2, blocked: ["wheel-11"]),
        ])
        let runtime = try Self.listPane(value)
        try runtime.send(Self.msg("set_view", ["value": .array([.string("deps")])]))
        let rows = try Self.rows(runtime.view(Self.data))
        // wheel-12 depends on wheel-11 depends on wheel-2: one chain from the bead that waits on the most.
        #expect(rows.map(\.id) == ["wheel-12", "wheel-11", "wheel-2"])
        #expect(rows.map { $0.row["depth"] } == [.number(0), .number(1), .number(2)])
    }

    @Test func theUnlocksViewWalksADependencyCycleOnce() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("set_view", ["value": .array([.string("unlocks")])]))
        let rows = try Self.rows(runtime.view(Self.data))
        // wheel-6 unlocks 7, 7 unlocks 8, 8 unlocks 7 again (skipped: it is on the path); wheel-2 unlocks 11.
        #expect(rows.map(\.id) == ["wheel-6", "wheel-7", "wheel-8", "wheel-2", "wheel-11"])
        #expect(rows.map { $0.row["depth"] } == [.number(0), .number(1), .number(2), .number(0), .number(1)])
    }

    @Test func aTreeViewKeepsAnAncestorOfAMatchAndDropsTheRest() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("set_view", ["value": .array([.string("epics")])]))
        try runtime.send(Self.msg("list.query", ["value": .string("1.1")]))
        #expect(try Self.ids(runtime) == ["wheel-1", "wheel-1.1"])
    }

    @Test func aTreeViewHasNoSortActionAndTheListHasOne() throws {
        let runtime = try Self.listPane()
        var table = try #require(try Self.all(runtime.view(Self.data), "Table").first)
        #expect(table["sortAction"] != nil)
        try runtime.send(Self.msg("set_view", ["value": .array([.string("deps")])]))
        table = try #require(try Self.all(runtime.view(Self.data), "Table").first)
        #expect(table["sortAction"] == nil)
    }

    // MARK: the act decision, against buttons::decide (plugins/wheelhouse/src/beads/buttons.rs)

    static let groups = """
    {"task":{"start":{"label":"Begin","run":"go {id}"},"resume":{"label":null,"run":"again {id}"},"focus":{"label":"Show","run":null}},
     "epic":{"start":{"label":null,"run":null},"resume":{"label":null,"run":null},"focus":{"label":null,"run":null}},
     "closed":"Shipped"}
    """

    /// The footer button of `id`'s detail card: its label, disabled and help.
    static func act(_ value: JSONValue, panes: JSONValue = .null, open id: String) throws -> Btn? {
        let runtime = try listPane(value, panes: panes)
        try runtime.send(msg("select_bead", ["id": .string(id)]))
        return buttons(try runtime.view(data)).first { actionType($0) == "press" }
    }

    @Test func theActFollowsTheRecordAndTheOpenPanesAsDecideDoes() throws {
        func snap(_ rows: [String], records: String = "{}", pressing: String = "[]") throws -> JSONValue {
            try Self.snapshot(rows, buttons: Self.groups, records: records, pressing: pressing)
        }
        // Fresh: no record -> Start, with the project's label and a run.
        var a = try #require(try Self.act(snap([Self.bead("wheel-3", ready: true)]), open: "wheel-3"))
        #expect((a.text, a.disabled, a.help) == ("Begin", false, ""))
        // The epic group's verbs have no run: disabled, naming the missing key.
        a = try #require(try Self.act(snap([Self.bead("wheel-4", kind: "epic", epic: true)]), open: "wheel-4"))
        #expect((a.text, a.disabled, a.help) == ("Start", true, "[beads.epic] has no start.run"))
        // Saved: a record whose pane is not open -> Resume (the verb's label falls back).
        let rec = "{\"wheel-3\":{\"pane\":\"p9\",\"conversation\":\"c\"}}"
        a = try #require(try Self.act(snap([Self.bead("wheel-3")], records: rec), panes: Self.json(#"[{"id":"p1"}]"#), open: "wheel-3"))
        #expect((a.text, a.disabled, a.help) == ("Resume", false, ""))
        // Saved with no resume.run (the epic group has none): disabled, naming the missing key.
        let erec = "{\"wheel-4\":{\"pane\":\"p9\",\"conversation\":\"c\"}}"
        a = try #require(try Self.act(snap([Self.bead("wheel-4", kind: "epic", epic: true)], records: erec), panes: Self.json(#"[{"id":"p1"}]"#), open: "wheel-4"))
        #expect((a.text, a.disabled, a.help) == ("Resume", true, "[beads.epic] has no resume.run"))
        // Live: the record's pane is open -> Focus, enabled although it has no run.
        a = try #require(try Self.act(snap([Self.bead("wheel-3")], records: rec), panes: Self.json(#"[{"id":"p9"}]"#), open: "wheel-3"))
        #expect((a.text, a.disabled, a.help) == ("Show", false, ""))
        // The panes cannot be listed yet: a bead with a record is disabled and says so.
        a = try #require(try Self.act(snap([Self.bead("wheel-3")], records: rec), open: "wheel-3"))
        #expect((a.disabled, a.help) == (true, "The open panes are not known yet"))
        // A press in flight disables the button.
        a = try #require(try Self.act(snap([Self.bead("wheel-3")], pressing: "[\"wheel-3\"]"), open: "wheel-3"))
        #expect((a.disabled, a.help) == (true, "Starting; wait for the pane"))
        // Live with no focus label falls back to "Focus".
        a = try #require(try Self.act(Self.snapshot([Self.bead("wheel-3")], buttons: """
        {"task":{"start":{"label":null,"run":"go"},"resume":{"label":null,"run":"go"},"focus":{"label":null,"run":null}},"epic":null,"closed":null}
        """, records: rec), panes: Self.json(#"[{"id":"p9"}]"#), open: "wheel-3"))
        #expect((a.text, a.disabled) == ("Focus", false))
        // Closed with no group and no closed label: "Closed", disabled.
        a = try #require(try Self.act(Self.snapshot([Self.bead("wheel-3", "closed")]), open: "wheel-3"))
        #expect((a.text, a.disabled, a.help) == ("Closed", true, "This bead is closed"))
        // Closed: the project's closed label, disabled, whatever the record says.
        a = try #require(try Self.act(snap([Self.bead("wheel-3", "closed")], records: rec), panes: Self.json(#"[{"id":"p9"}]"#), open: "wheel-3"))
        #expect((a.text, a.disabled, a.help) == ("Shipped", true, "This bead is closed"))
    }

    @Test func aProjectWithNoGroupForAKindDrawsNoButton() throws {
        let value = try Self.snapshot([Self.bead("wheel-3", ready: true)])
        #expect(try Self.act(value, open: "wheel-3") == nil)
        // And the row's menu offers nothing.
        let runtime = try Self.listPane(value)
        let rows = try Self.rows(runtime.view(Self.data))
        #expect(rows[0].row["menu"] == .array([]))
    }

    @Test func aRowMenuOffersTheActionOnlyWhenItIsEnabledAndPressSendsItToTheBackend() throws {
        let rec = Recorder()
        let value = try Self.snapshot([Self.bead("wheel-3", ready: true)], buttons: Self.groups)
        let runtime = try Self.listPane(value, rec: rec)
        let rows = try Self.rows(runtime.view(Self.data))
        guard case let .array(menu)? = rows[0].row["menu"], case let .object(item)? = menu.first else {
            throw Failure("no menu item: \(String(describing: rows[0].row["menu"]))")
        }
        #expect(Self.str(item["label"]) == "Begin")
        try runtime.send(try #require(item["action"]))
        #expect(rec.actions.count == 1 && rec.actions[0].0 == "wheelhouse.press")
        #expect(rec.actions[0].1 == .object(["dir": .string("/p"), "bead": .string("wheel-3")]))
        // The answer is for the oldest press waiting; the detail card shows it.
        try runtime.send(Self.json(#"{"action":"wheelhouse.press","result":{"said":"Started wheel-3"}}"#))
        try runtime.send(Self.msg("select_bead", ["id": .string("wheel-3")]))
        #expect(try Self.texts(Self.jsonOf(runtime.view(Self.data))).contains("Started wheel-3") == true)
    }

    // MARK: detail card

    @Test func theDetailCardListsFactsAndTheTreesAndBackReturnsToTheLast() throws {
        let runtime = try Self.listPane()
        try runtime.send(Self.msg("select_bead", ["id": .string("wheel-7")]))
        var t = Self.texts(Self.jsonOf(try runtime.view(Self.data)))
        #expect(t.contains("Bead wheel-7") && t.contains("wheel-7") && t.contains("blocked"))
        #expect(t.contains("Depends on"))
        // Its two trees: wheel-7 waits on wheel-6 and wheel-8 and unlocks wheel-8.
        #expect(t.contains("6  Bead wheel-6") && t.contains("8  Bead wheel-8"))
        try runtime.send(Self.msg("open_bead", ["id": .string("wheel-6")]))
        t = Self.texts(Self.jsonOf(try runtime.view(Self.data)))
        #expect(t.contains("Bead wheel-6"))
        try runtime.send(Self.msg("back"))
        t = Self.texts(Self.jsonOf(try runtime.view(Self.data)))
        #expect(t.contains("Bead wheel-7"))
        try runtime.send(Self.msg("close_detail"))
        let modal = try #require(try Self.all(runtime.view(Self.data), "Modal").first)
        #expect(modal["presented"] == .bool(false))
    }

    // MARK: the 2,500-bead fixture

    /// 2,500 beads: 50 epics of 49 children each, a blocked chain through every tenth child.
    static func large() throws -> JSONValue {
        var rows: [String] = []
        var n = 0
        for e in 0..<50 {
            let epic = "wheel-e\(e)"
            rows.append(bead(epic, kind: "epic", pri: 1, epic: true))
            n += 1
            for c in 0..<49 {
                let id = "wheel-\(e)x\(c)"
                let status = c % 7 == 0 ? "closed" : (c % 5 == 0 ? "in_progress" : "open")
                rows.append(bead(id, status, pri: c % 5, parent: epic, labels: ["l\(c % 4)"],
                                 blocked: c % 10 == 3 && c > 3 ? ["wheel-\(e)x\(c - 10)"] : [], ready: c % 2 == 0,
                                 desc: String(repeating: "word ", count: 20)))
                n += 1
            }
        }
        return try snapshot(rows)
    }

    @Test func aLargeFeedStaysUnderTheValueLimitInEveryView() throws {
        let runtime = try Self.listPane(Self.large())
        // List: 2,450 leaves minus the closed ones, cut to the 1,500 sent, with one row saying how many are not.
        var rows = try Self.rows(runtime.view(Self.data))
        #expect(rows.count == 1501)
        #expect(rows.last?.id == "\u{2026}more")
        #expect(rows.last?.title == "600 more not shown; search or filter to narrow")
        for view in ["epics", "deps", "unlocks"] {
            try runtime.send(Self.msg("set_view", ["value": .array([.string(view)])]))
            rows = try Self.rows(runtime.view(Self.data))
            #expect(rows.count > 0 && rows.count <= 1501, "\(view): \(rows.count) rows")
        }
        // Narrowed, the rows fit and no more row ends the table.
        try runtime.send(Self.msg("set_view", ["value": .array([.string("list")])]))
        try runtime.send(Self.msg("list.query", ["value": .string("wheel-3x1")]))
        rows = try Self.rows(runtime.view(Self.data))
        #expect(rows.count > 0 && rows.allSatisfy { $0.id.hasPrefix("wheel-3x1") })
    }

    // MARK: panes/diff.luau's positional ids

    static func diffValue(files: Int) throws -> JSONValue {
        let fs = (0..<files).map { i in
            """
            {"path":"src/f\(i).rs","status":"M","add":2,"del":1,"hunks":[{"header":"-1,2 +1,3 @@","lines":[
              {"kind":" ","old":1,"new":1,"text":"keep"},{"kind":"-","old":2,"text":"gone"},{"kind":"+","new":2,"text":"new"}]}]}
            """
        }.joined(separator: ",")
        return try json("""
        {"error":null,"scopes":[{"key":"uncommitted","label":"Uncommitted","base":"main","files":[\(fs)]}]}
        """)
    }

    /// The node `id` names ("root.2.0.0.1": a child index per dot).
    static func node(_ tree: PaneNode, _ id: String) throws -> PaneNode {
        var n = tree
        for part in id.split(separator: ".").dropFirst() {
            guard let i = Int(part), i < n.children.count else { throw Failure("\(id): no child \(part) under \(n.component)") }
            n = n.children[i]
        }
        return n
    }

    @Test func theDiffPaneScrollsToTheListsItsConstantsNameInBothLayouts() throws {
        let runtime = try Self.pane("diff", feed: "diff", value: Self.diffValue(files: 3))
        var tree = try runtime.view(Self.data)
        // Nothing selected draws the first file; the wide stream and the narrow lines are the Lists the constants name.
        let stream = try Self.node(tree, "root.2.0.0.1")
        #expect(stream.component == "List")
        #expect(stream.children.count == 3 && stream.children.allSatisfy { $0.component == "Section" })
        #expect(stream.props["scrollTo"] == .string("root.2.0.0.1.0"))
        let lines = try Self.node(tree, "root.2.1.2")
        #expect(lines.component == "List")
        #expect(lines.props["scrollTo"] == .string("root.2.1.2.0"))
        // The stream's side is the file table the split holds before it.
        #expect(try Self.all(Self.node(tree, "root.2.0.0.0"), "Table").count == 1)
        // Selecting the third file scrolls the stream to its section, and that section is its header and lines.
        try runtime.send(Self.msg("select_file", ["id": .string("src/f2.rs")]))
        tree = try runtime.view(Self.data)
        let moved = try Self.node(tree, "root.2.0.0.1")
        #expect(moved.props["scrollTo"] == .string("root.2.0.0.1.2"))
        let section = try Self.node(tree, "root.2.0.0.1.2")
        #expect(section.component == "Section")
        #expect(Self.texts(Self.jsonOf(section)).contains("src/f2.rs"))
        // The narrow list draws the open file's lines: its first one is what scrollTo names.
        let narrow = try Self.node(tree, "root.2.1.2")
        #expect(narrow.props["scrollTo"] == .string("root.2.1.2.0"))
        #expect(Self.texts(Self.jsonOf(narrow)).first?.hasPrefix("@@ ") == true)
    }

    @Test func theDiffPaneNamesAFileBeyondTheDrawnLimitAndSaysHowManyAreLeftOut() throws {
        let runtime = try Self.pane("diff", feed: "diff", value: Self.diffValue(files: 205))
        var tree = try runtime.view(Self.data)
        #expect(try Self.node(tree, "root.2.0.0.1").children.count == 200)
        #expect(Self.texts(Self.jsonOf(tree)).contains("5 more files; `wheelhouse diff` lists them all."))
        // A selected file beyond the first 200 is drawn after them and the scroll lands on it.
        try runtime.send(Self.msg("select_file", ["id": .string("src/f204.rs")]))
        tree = try runtime.view(Self.data)
        let stream = try Self.node(tree, "root.2.0.0.1")
        #expect(stream.children.count == 201)
        #expect(stream.props["scrollTo"] == .string("root.2.0.0.1.200"))
    }
}
