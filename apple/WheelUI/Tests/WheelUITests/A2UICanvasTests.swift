import A2UISwiftCore
import Foundation
import Testing
@testable import WheelUI

@Suite struct A2UICanvasTests {
    @Test func pathDataBecomesAPathWithItsCurvesAndClose() {
        let path = canvasPath("M0 0 L10 0 C 10 5 5 10 0 10 Z")
        #expect(path.boundingRect == CGRect(x: 0, y: 0, width: 10, height: 10))
        #expect(canvasPath("M1 2 Q 5 9 8 2").boundingRect.maxX == 8)
    }

    @Test func aColourIsAToneOrHex() {
        #expect(canvasColor("ok") != nil)
        #expect(canvasColor("#336699") != nil)
        #expect(canvasColor("#33669980") != nil)
        #expect(canvasColor("nonsense") == nil)
        #expect(canvasColor(nil) == nil)
    }

    @Test func aSceneReadsItsOpsHitsAndBounds() throws {
        let json = #"{"ops":[{"op":"rect","r":[[0,0,10,5]],"fill":"fill","radius":2,"minZoom":0.5},{"op":"circle","c":[[3,3,2],[8,8,1]],"fill":"ok"},{"op":"text","t":[[1,1,"hi"]],"size":11,"anchor":"middle"}],"hits":[{"id":"a","circle":[3,3,2],"action":"pick","context":{"k":"v"},"slop":4}],"bounds":[0,0,20,10],"focus":{"x":1,"y":2,"w":3,"h":4,"key":"k1"},"emptyAction":"none"}"#
        let props = try JSONDecoder().decode([String: AnyCodable].self, from: Data(json.utf8))
        let scene = CanvasScene(props)
        #expect(scene.ops.map(\.kind) == ["rect", "circle", "text"])
        #expect(scene.ops[0].style.minZoom == 0.5)
        #expect(scene.ops[1].nums.count == 2)
        #expect(scene.ops[2].texts.first?.2 == "hi")
        #expect(scene.hits.first?.circle?.2 == 2)
        #expect(scene.hits.first?.slop == 4)
        #expect(scene.bounds == CGRect(x: 0, y: 0, width: 20, height: 10))
        #expect(scene.focus == CGRect(x: 1, y: 2, width: 3, height: 4))
        #expect(scene.focusKey == "k1")
        #expect(scene.emptyAction == "none")
    }
}
