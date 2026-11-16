#if os(macOS)
import AppKit
import Foundation
import SwiftUI
import Testing
@testable import WheelUI

/// `StructuredText{markdown, style?}` (ADR-0046) drawn in a real hosting view.
@MainActor
struct A2UIStructuredTextTests {
    private func height(_ props: String) throws -> CGFloat {
        let comps = #"[{"id":"root","component":"StructuredText","# + props + "}]"
        let json = #"[{"createSurface":{"surfaceId":"s"}},{"updateComponents":{"components":"# + comps + "}}]"
        let model = SurfaceModel()
        #expect(model.process(try JSONDecoder().decode([[String: AnyCodable]].self, from: Data(json.utf8))).isEmpty)
        let host = NSHostingView(rootView: A2UISurfaceView(surface: model).frame(width: 480))
        host.layoutSubtreeIfNeeded()
        return host.fittingSize.height
    }

    private func encoded(_ s: String) throws -> String {
        String(decoding: try JSONEncoder().encode(s), as: UTF8.self)
    }

    @Test func blocksDrawAsBlocksInEveryStyle() throws {
        let one = try height(#""markdown":"one line""#)
        let doc = try encoded("# Title\n\n- a\n- b\n- c\n\n> quote\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n```\ncode\n```")
        for style in ["default", "gitHub", "chat"] {
            let tall = try height(#""style":"\#(style)","markdown":"# + doc)
            #expect(tall > one * 4, "\(style): \(tall) pt against \(one) pt for one line")
        }
    }
}
#endif
