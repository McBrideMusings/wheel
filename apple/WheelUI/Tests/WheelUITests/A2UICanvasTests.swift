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

    @Test func aScreenSizeIsClampedToItsRange() {
        #expect(canvasClamp(0.5, [1.4, 5.5]) == 1.4)
        #expect(canvasClamp(3, [1.4, 5.5]) == 3)
        #expect(canvasClamp(40, [1.4, 5.5]) == 5.5)
        #expect(canvasClamp(40, nil) == 40)
    }

    @Test func aLabelIsCutWithAnEllipsisToFitItsWidth() {
        let width: (String) -> Double = { Double($0.count) * 5 }
        #expect(canvasCut("short", limit: 100, measure: width) == "short")
        let cut = canvasCut("a much longer title than fits", limit: 50, measure: width)
        #expect(cut.hasSuffix("…") && width(cut) <= 50)
        #expect(canvasCut("anything", limit: 3, measure: width) == "…")
    }

    @Test func aSceneReadsScreenSizesTipsAndKeys() throws {
        let json = #"{"ops":[{"op":"circle","c":[[3,3,8]],"rpx":[1.4,5.5]},{"op":"rect","r":[[0,0,9,9]],"hpx":[1.5,26],"wpx":[20,1000]},{"op":"text","t":[[1,1,"hi"]],"sizepx":[8,14],"maxw":90,"minx":8},{"op":"path","d":["M0 0 L10 5"]}],"hits":[{"id":"a","rect":[0,0,5,5],"action":"pick","tip":["one",[{"t":"x","tone":"ok","mono":true}]],"doubleAction":"open","fit":[0,0,9,9],"label":"A"}],"focus":{"rect":[1,2,3,4],"key":"k","zoom":1},"fitKey":"f","sizeAction":"w","sizeKnown":900,"escapeAction":"esc","enterAction":"go","wheel":"pan","emptyContext":{"id":""}}"#
        let scene = CanvasScene(try JSONDecoder().decode([String: AnyCodable].self, from: Data(json.utf8)))
        #expect(scene.ops[0].style.rpx == [1.4, 5.5])
        #expect(scene.ops[1].style.hpx == [1.5, 26] && scene.ops[1].style.wpx == [20, 1000])
        #expect(scene.ops[2].style.sizepx == [8, 14] && scene.ops[2].style.maxw == 90 && scene.ops[2].style.minx == 8)
        #expect(scene.ops[3].paths.first?.bounds == CGRect(x: 0, y: 0, width: 10, height: 5))
        #expect(scene.hits[0].tip.count == 2 && scene.hits[0].tip[1][0].mono && scene.hits[0].tip[1][0].tone == "ok")
        #expect(scene.hits[0].doubleAction == "open" && scene.hits[0].fit == CGRect(x: 0, y: 0, width: 9, height: 9) && scene.hits[0].label == "A")
        #expect(scene.focusZoom == 1 && scene.fitKey == "f" && scene.sizeAction == "w" && scene.sizeKnown == 900)
        #expect(scene.escapeAction == "esc" && scene.enterAction == "go" && scene.wheelPans && scene.emptyContext["id"]?.stringValue == "")
    }

    @Test func aFocusAtZoomCentresTheRectButStopsAtTheScenesEdges() {
        let bounds = CGRect(x: 0, y: 0, width: 1000, height: 400)
        let view = CGSize(width: 600, height: 800)
        // a scene shorter than the view starts at the top; wider than the view, the rect is centred
        var c = canvasFocusCamera(CGRect(x: 480, y: 190, width: 40, height: 20), zoom: 1, bounds: bounds, in: view)
        #expect(c.s == 1 && c.y == 0 && c.x == -200)
        // near the left edge the camera stops at the edge instead of showing blank space
        c = canvasFocusCamera(CGRect(x: 10, y: 0, width: 40, height: 20), zoom: 1, bounds: bounds, in: view)
        #expect(c.x == 0)
        c = canvasFocusCamera(CGRect(x: 990, y: 0, width: 10, height: 20), zoom: 1, bounds: bounds, in: view)
        #expect(c.x == -400)
    }
}
