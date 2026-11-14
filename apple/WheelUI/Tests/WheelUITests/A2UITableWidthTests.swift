#if os(macOS)
import AppKit
import Foundation
import SwiftUI
import Testing
@testable import WheelUI

@MainActor
struct A2UITableWidthTests {
    private func tableColumns(width: CGFloat) async throws -> [CGFloat] {
        let comps = ##"[{"id":"root","component":"Table","columns":[{"key":"id","label":"ID","width":80},{"key":"title","label":"Title","width":null},{"key":"labels","label":"Labels","width":120},{"key":"who","label":"Author","width":100}],"sections":[{"title":"","count":1,"rows":[{"id":"1","path":"1","cells":[{"key":"id","text":"#1"},{"key":"title","text":"Wrong icon in sidebar"},{"key":"labels","text":"bug"},{"key":"who","text":"pierce"}]}]}]}]"##
        let json = #"[{"createSurface":{"surfaceId":"s"}},{"updateComponents":{"components":"# + comps + "}}]"
        let model = SurfaceModel()
        #expect(model.process(try JSONDecoder().decode([[String: AnyCodable]].self, from: Data(json.utf8))).isEmpty)
        let view = A2UISurfaceView(surface: model).environment(\.a2uiPane, CGSize(width: width, height: 400))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: width, height: 400), styleMask: [.titled], backing: .buffered, defer: false)
        let host = NSHostingView(rootView: view)
        window.contentView = host
        host.frame = NSRect(x: 0, y: 0, width: width, height: 400)
        func find(_ v: NSView) -> NSTableView? {
            if let t = v as? NSTableView { return t }
            for s in v.subviews { if let t = find(s) { return t } }
            return nil
        }
        var last: [CGFloat] = []
        var steady = 0
        for _ in 0..<100 where steady < 5 {
            host.layoutSubtreeIfNeeded()
            try await Task.sleep(nanoseconds: 50_000_000)
            let now = find(host)?.tableColumns.map(\.width) ?? []
            steady = (!now.isEmpty && now == last) ? steady + 1 : 0
            last = now
        }
        let table = try #require(find(host))
        return table.tableColumns.map(\.width)
    }

    @Test func aColumnWithNoWidthTakesTheLeftoverAndTheOthersKeepTheirs() async throws {
        let w = try await tableColumns(width: 960)
        #expect(w.count == 4)
        #expect(abs(w[0] - 80) < 1 && abs(w[2] - 120) < 1 && abs(w[3] - 100) < 1)
        #expect(w[1] > 500)
    }
}
#endif
