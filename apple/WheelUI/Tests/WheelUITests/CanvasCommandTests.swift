import Foundation
import SwiftUI
import Testing
@testable import WheelUI

private func cmds(_ json: String) throws -> [AnyCodable] {
    try JSONDecoder().decode([AnyCodable].self, from: Data(json.utf8))
}

private func close(_ a: (Double, Double, Double, Double)?, _ b: (Double, Double, Double, Double)) -> Bool {
    guard let a else { return false }
    return abs(a.0 - b.0) < 0.002 && abs(a.1 - b.1) < 0.002 && abs(a.2 - b.2) < 0.002 && abs(a.3 - b.3) < 0.002
}

@Suite struct CanvasCommandParsingTests {
    @Test func coloursParse() {
        #expect(canvasParseColor("fg") != nil)
        #expect(canvasParseColor("accent") != nil)
        #expect(close(canvasRGBA("#f00"), (1, 0, 0, 1)))
        #expect(close(canvasRGBA("#f008"), (1, 0, 0, 136.0 / 255)))
        #expect(close(canvasRGBA("#336699"), (0.2, 0.4, 0.6, 1)))
        #expect(close(canvasRGBA("#33669980"), (0.2, 0.4, 0.6, 128.0 / 255)))
        #expect(close(canvasRGBA("rgb(255, 0, 51)"), (1, 0, 0.2, 1)))
        #expect(close(canvasRGBA("rgba(255, 0, 51, 0.5)"), (1, 0, 0.2, 0.5)))
        #expect(close(canvasRGBA("white"), (1, 1, 1, 1)))
        #expect(close(canvasRGBA("transparent"), (0, 0, 0, 0)))
        #expect(canvasParseColor("red") != nil)
        for junk in ["", "nonsense", "#12", "#12345", "#ggg", "rgb(1,2)", "rgb(a,b,c)"] {
            #expect(canvasRGBA(junk) == nil, "\(junk)")
        }
        #expect(canvasParseColor("nonsense") == nil)
    }

    @Test func everyCommandParses() throws {
        let list = try cmds(#"""
        [["set","fillStyle","red"],["set","strokeStyle","#fff"],["set","lineWidth",3],["set","font","12px serif"],["set","globalAlpha",0.5],
         ["beginPath"],["moveTo",1,2],["lineTo",3,4],["bezierCurveTo",1,2,3,4,5,6],["quadraticCurveTo",1,2,3,4],
         ["arc",1,2,3,0,6.28,false],["rect",1,2,3,4],["roundRect",1,2,3,4,5],["closePath"],["fill"],["stroke"],["save"],["restore"],
         ["translate",1,2],["scale",1,2],["rotate",0.5],["fillText","hi",1,2],["drawImage","AAAAAA==",1,1,0,0,1,1]]
        """#)
        #expect(canvasCommands(list) == [
            .fillStyle("red"), .strokeStyle("#fff"), .lineWidth(3), .font("12px serif"), .globalAlpha(0.5),
            .beginPath, .moveTo(1, 2), .lineTo(3, 4), .bezierCurveTo(1, 2, 3, 4, 5, 6), .quadraticCurveTo(1, 2, 3, 4),
            .arc(1, 2, 3, 0, 6.28, false), .rect(1, 2, 3, 4), .roundRect(1, 2, 3, 4, 5), .closePath, .fill, .stroke, .save, .restore,
            .translate(1, 2), .scale(1, 2), .rotate(0.5), .fillText("hi", 1, 2), .drawImage("AAAAAA==", 1, 1, 0, 0, 1, 1),
        ])
    }

    @Test func badCommandsAreDropped() throws {
        let list = try cmds(#"""
        [["moveTo",1],["moveTo",1,2,3],["moveTo","a",2],["lineTo"],["rect",1,2,3],["arc",1,2,3,0,1],["arc",1,2,3,0,1,"no"],
         ["fillText",1,2,3],["fillText","t","x",2],["fillStyle","red"],["set","fillStyle",3],["set","lineWidth","3"],["set","bogus",1],
         ["set","fillStyle"],["beginPath",1],["save",0],["drawImage","AA==",0,1,0,0,1,1],["drawImage","AA==",1,1,0,0],["nope"],[],"text",
         ["fill"]]
        """#)
        #expect(canvasCommands(list) == [.fill])
    }
}

@Suite struct CanvasPlayerTests {
    func play(_ json: String) throws -> CanvasPlayer {
        CanvasPlayer(canvasCommands(try cmds(json)), fg: .primary)
    }

    @Test func transformsMapPathPoints() throws {
        let p = try play(#"[["translate",10,20],["scale",2,2],["beginPath"],["rect",0,0,5,5]]"#)
        #expect(p.path.boundingRect == CGRect(x: 10, y: 20, width: 10, height: 10))
    }

    @Test func anArcContinuesTheSubpath() throws {
        // A pie wedge: centre, a quarter arc from 0 to pi/2 (clockwise on screen, y down), closed back to the centre. One subpath
        // holding the centre, so the fill contains a point near the centre inside the wedge and none outside it.
        let p = try play(#"[["beginPath"],["moveTo",50,50],["arc",50,50,40,0,1.5707963,false],["closePath"]]"#)
        var moves = 0
        p.path.forEach { if case .move = $0 { moves += 1 } }
        #expect(moves == 1)
        #expect(p.path.contains(CGPoint(x: 55, y: 55)))
        #expect(!p.path.contains(CGPoint(x: 45, y: 55)))
        #expect(!p.path.contains(CGPoint(x: 55, y: 45)))
    }

    @Test func aRingSectorIsOneFilledShape() throws {
        // Outer arc clockwise, inner arc back counter-clockwise, closed: the band between them fills, the hole does not.
        let p = try play(#"[["beginPath"],["arc",50,50,40,0,3.14159,false],["arc",50,50,20,3.14159,0,true],["closePath"]]"#)
        #expect(p.path.contains(CGPoint(x: 50, y: 80)))
        #expect(!p.path.contains(CGPoint(x: 50, y: 60)))
        #expect(!p.path.contains(CGPoint(x: 50, y: 30)))
    }

    @Test func restoreUndoesTheTransform() throws {
        let p = try play(#"[["save"],["translate",10,20],["restore"],["beginPath"],["rect",0,0,5,5]]"#)
        #expect(p.path.boundingRect == CGRect(x: 0, y: 0, width: 5, height: 5))
    }

    @Test func fillsAndStrokesArePaints() throws {
        let p = try play(#"[["beginPath"],["rect",0,0,5,5],["fill"],["stroke"],["stroke"]]"#)
        var fills = 0, strokes = 0
        for paint in p.paints {
            switch paint {
            case .fill: fills += 1
            case .stroke: strokes += 1
            default: break
            }
        }
        #expect(fills == 1)
        #expect(strokes == 2)
    }

    @Test func strokeWidthScalesWithTheTransform() throws {
        let p = try play(#"[["set","lineWidth",3],["scale",2,2],["beginPath"],["moveTo",0,0],["lineTo",5,0],["stroke"]]"#)
        guard case let .stroke(_, _, width)? = p.paints.first else { Issue.record("no stroke"); return }
        #expect(abs(width - 6) < 1e-9)
    }

    @Test func drawImageNeedsAMatchingPayload() throws {
        let pixel = Data([255, 0, 0, 255]).base64EncodedString()
        let good = try play(#"[["drawImage","\#(pixel)",1,1,2,3,4,5]]"#)
        guard case let .image(_, rect, _, _)? = good.paints.first else { Issue.record("no image paint"); return }
        #expect(rect == CGRect(x: 2, y: 3, width: 4, height: 5))
        let wrong = Data([255, 0, 0]).base64EncodedString()
        #expect(try play(#"[["drawImage","\#(wrong)",1,1,0,0,1,1]]"#).paints.isEmpty)
    }
}

@MainActor @Suite struct CanvasPluginPaneTests {
    static let sizeScript = """
    return {
        init = function(p) return p or { got = false } end,
        update = function(state, msg) state.got = true end,
        view = function(state, data)
            return ui.Canvas{ key = 'c', draw = function(ctx, w, h) ctx:fillText(tostring(w), 0, 10) end }
        end,
    }
    """

    static let pointerScript = """
    return {
        init = function(p) return p or { log = {} } end,
        update = function(state, msg)
            table.insert(state.log, msg.type .. ' ' .. tostring(msg.kind) .. ' ' .. tostring(msg.x) .. ' ' .. tostring(msg.y))
        end,
        view = function(state, data)
            return ui.Canvas{ key = 'p', on_pointer = { type = 'pointer' }, draw = function(ctx, w, h) ctx:rect(0, 0, w, h) end }
        end,
    }
    """

    func canvas(_ pane: PluginPane) throws -> ComponentNode {
        func find(_ n: ComponentNode) -> ComponentNode? {
            if n.instance.type == "Canvas" { return n }
            for c in n.children { if let f = find(c) { return f } }
            return nil
        }
        return try #require(pane.surface.root.flatMap(find))
    }

    func fillTexts(_ pane: PluginPane) throws -> [String] {
        (try canvas(pane).props["commands"]?.arrayValue ?? []).compactMap { c in
            guard let a = c.arrayValue, a.first?.stringValue == "fillText" else { return nil }
            return a[1].stringValue
        }
    }

    @Test func aSizeReportRedrawsWithoutReachingUpdate() throws {
        let pane = PluginPane()
        pane.load(source: Self.sizeScript, chunk: "size.luau")
        #expect(pane.error == nil)
        #expect(try fillTexts(pane) == ["0"])
        let node = try canvas(pane)
        pane.surface.dispatchAction(name: "canvas-size:c", sourceComponentId: node.instance.id,
                                    context: ["w": .number(300), "h": .number(240)])
        #expect(try fillTexts(pane) == ["300"])
        #expect(pane.stateJSON == #"{"got":false}"#)
    }

    @Test func aPointerActionReachesUpdateWithTheEventsFields() throws {
        let pane = PluginPane()
        pane.load(source: Self.pointerScript, chunk: "pointer.luau")
        #expect(pane.error == nil)
        let node = try canvas(pane)
        #expect(node.props["pointerAction"] != nil)
        #expect(pane.raise(component: node.instance.id, key: "pointerAction",
                           context: ["kind": "down", "x": 3, "y": 4, "button": 0, "dx": 0, "dy": 0]))
        #expect(pane.stateJSON == #"{"log":["pointer down 3 4"]}"#)
    }
}
