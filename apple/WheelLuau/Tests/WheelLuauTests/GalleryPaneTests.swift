import Foundation
import Testing
import WheelKit
@testable import WheelLuau

/// The component gallery (`plugins/gallery/panes/gallery.luau`) run through the pane runtime under
/// its default limits and flattened the way a client draws it.
@Suite struct GalleryPaneTests {
    static let script = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("plugins/gallery/panes/gallery.luau")

    static func components() throws -> [String: [String: JSONValue]] {
        let source = try String(contentsOf: script, encoding: .utf8)
        let pane = try PaneRuntime(source: source)
        var out: [String: [String: JSONValue]] = [:]
        for case let .object(fields) in try pane.view().a2uiComponents() {
            if case let .string(id)? = fields["id"] { out[id] = fields }
        }
        return out
    }

    @Test func drawsEveryClass() throws {
        let all = try Self.components()
        let texts = Set(all.values.compactMap { f -> String? in if case let .string(t)? = f["text"] { t } else { nil } })
        for heading in ["A2UI component gallery", "Content", "Buttons", "Selection and input", "Containers and layout",
                        "Navigation and presentation", "SwiftUI controls", "Table", "Modifiers", "Wheel's own component"] {
            #expect(texts.contains(heading), "no \(heading) heading")
        }
    }

    /// A vertical List or a Table with no frame makes the surface fill the pane (SurfaceModel.fillsPane),
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

    @Test func everyIDAPropertyNamesExists() throws {
        let all = try Self.components()
        let tabs = try #require(all.values.first { $0["component"] == .string("Tabs") })
        guard case let .array(list)? = tabs["tabs"] else { Issue.record("Tabs has no tabs"); return }
        for case let .object(tab) in list {
            guard case let .string(child)? = tab["child"] else { Issue.record("a tab has no child id: \(tab)"); continue }
            #expect(all[child] != nil, "tab child \(child) is not a component")
        }
        let pinned = try #require(all.values.first { $0["scrollTo"] != nil })
        guard case let .string(target)? = pinned["scrollTo"] else { Issue.record("scrollTo is not an id"); return }
        #expect(all[target]?["component"] == .string("Section"), "scrollTo names \(target), not a Section")
    }
}
