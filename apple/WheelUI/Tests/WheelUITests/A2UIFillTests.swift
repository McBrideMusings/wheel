import Foundation
import Testing
@testable import WheelUI

@MainActor
struct A2UIFillTests {
    private func surface(_ components: String) throws -> SurfaceModel {
        let json = #"[{"createSurface":{"surfaceId":"s"}},{"updateComponents":{"components":"# + components + "}}]"
        let model = SurfaceModel()
        #expect(model.process(try JSONDecoder().decode([[String: AnyCodable]].self, from: Data(json.utf8))).isEmpty)
        return model
    }

    @Test func aFramelessVerticalListFillsThePane() throws {
        let m = try surface(#"[{"id":"root","component":"Column","children":["l","c"]},{"id":"l","component":"List","children":["a"]},{"id":"a","component":"Text","text":"a"},{"id":"c","component":"Text","text":"composer"}]"#)
        #expect(m.fillsPane)
    }

    @Test func aListWithAFrameOrAHorizontalListDoesNotFill() throws {
        let framed = try surface(#"[{"id":"root","component":"List","children":["a"],"modifiers":[{"frame":{"height":80}}]},{"id":"a","component":"Text","text":"a"}]"#)
        #expect(!framed.fillsPane)
        let across = try surface(#"[{"id":"root","component":"List","direction":"horizontal","children":["a"]},{"id":"a","component":"Text","text":"a"}]"#)
        #expect(!across.fillsPane)
        let plain = try surface(#"[{"id":"root","component":"Text","text":"a"}]"#)
        #expect(!plain.fillsPane)
    }

    @Test func aFramelessTableFillsThePane() throws {
        let m = try surface(#"[{"id":"root","component":"Table","columns":[],"sections":[]}]"#)
        #expect(m.fillsPane)
    }
}
