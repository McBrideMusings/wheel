import Foundation
import Testing
import WheelKit
@testable import WheelLuau

/// The component gallery (`plugins/gallery/panes/gallery.luau`) run through the pane runtime under
/// its default limits, every node of its tree (children and nodes inside properties) listed by its path.
@Suite struct GalleryPaneTests {
    static let script = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("plugins/gallery/panes/gallery.luau")

    static func nodes() throws -> [String: PaneNode] {
        let source = try String(contentsOf: script, encoding: .utf8)
        var out: [String: PaneNode] = [:]
        func walk(_ n: PaneNode, _ id: String) {
            out[id] = n
            for (i, c) in n.children.enumerated() { walk(c, "\(id).\(i)") }
            for (k, v) in n.props { lift(v, "\(id)@\(k)") }
        }
        func lift(_ v: JSONValue, _ id: String) {
            if let n = PaneNode(v) { walk(n, id); return }
            switch v {
            case let .array(a): for (i, x) in a.enumerated() { lift(x, "\(id).\(i)") }
            case let .object(o): for (k, x) in o { lift(x, "\(id).\(k)") }
            default: break
            }
        }
        walk(try PaneRuntime(source: source).view(), "root")
        return out
    }

    static func components() throws -> [String: [String: JSONValue]] {
        try nodes().mapValues { n in n.props.merging(["component": .string(n.component)]) { a, _ in a } }
    }

    @Test func drawsEveryClass() throws {
        let all = try Self.components()
        let texts = Set(all.values.compactMap { f -> String? in if case let .string(t)? = f["text"] { t } else { nil } })
        for heading in ["A2UI component gallery", "Content", "Buttons", "Selection and input", "Containers and layout",
                        "Navigation and presentation", "SwiftUI controls", "Table", "Modifiers", "Wheel's own component"] {
            #expect(texts.contains(heading), "no \(heading) heading")
        }
    }

    /// A vertical List or a Table with no frame makes the view fill the pane (ViewTree.fillsPane),
    /// and a page that fills never scrolls past its first screen.
    @Test func theGalleryScrollsAsAWhole() throws {
        for (id, f) in try Self.components() {
            let scrollsItself = f["component"] == .string("Table")
                || (f["component"] == .string("List") && f["direction"] != .string("horizontal"))
            guard scrollsItself else { continue }
            guard case let .array(mods)? = f["modifiers"] else { Issue.record("\(id) has no modifiers"); continue }
            #expect(mods.contains { if case let .object(m) = $0 { m["frame"] != nil } else { false } }, "\(id) has no frame")
        }
    }

    @Test func everyNodeAPropertyNamesExists() throws {
        let all = try Self.nodes()
        let tabs = try #require(all.values.first { $0.component == "Tabs" })
        guard case let .array(list)? = tabs.props["tabs"] else { Issue.record("Tabs has no tabs"); return }
        for case let .object(tab) in list {
            guard let child = tab["child"] else { Issue.record("a tab has no child: \(tab)"); continue }
            #expect(PaneNode(child) != nil, "tab child \(child) is not a component")
        }
        let pinned = try #require(all.values.first { $0.props["scrollTo"] != nil })
        guard case let .number(place)? = pinned.props["scrollTo"] else { Issue.record("scrollTo is not a place"); return }
        let target = Int(place) - 1
        #expect(pinned.children.indices.contains(target) && pinned.children[target].component == "Section",
                "scrollTo names place \(Int(place)), not a Section")
    }
}
