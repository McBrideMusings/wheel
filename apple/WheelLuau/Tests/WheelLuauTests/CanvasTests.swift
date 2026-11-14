import Foundation
import Testing
import WheelKit
@testable import WheelLuau

private func pane(_ draw: String, canvasArgs: String = "") throws -> PaneRuntime {
    try PaneRuntime(source: """
    return {
        view = function()
            return ui.Canvas{ \(canvasArgs) draw = function(ctx, w, h)
                \(draw)
            end }
        end,
    }
    """)
}

private func commands(_ node: PaneNode) -> [JSONValue] {
    if case let .array(list)? = node.props["commands"] { list } else { [] }
}

private func n(_ values: Double...) -> [JSONValue] { values.map(JSONValue.number) }

@Suite struct CanvasTests {
    @Test func everyCallRecordsItsCommand() throws {
        let node = try pane("""
        ctx:beginPath() ctx:moveTo(1, 2) ctx:lineTo(3, 4) ctx:bezierCurveTo(1, 2, 3, 4, 5, 6)
        ctx:quadraticCurveTo(1, 2, 3, 4) ctx:arc(1, 2, 3, 0, 1.5) ctx:arc(1, 2, 3, 0, 1.5, true)
        ctx:rect(1, 2, 3, 4) ctx:roundRect(1, 2, 3, 4, 5) ctx:closePath() ctx:fill() ctx:stroke()
        ctx:save() ctx:translate(5, 6) ctx:scale(2, 3) ctx:rotate(0.5) ctx:restore()
        ctx:fillText('hi', 7, 8)
        ctx.moveTo(ctx, 9, 9)
        """).view()
        #expect(node.component == "Canvas")
        #expect(commands(node) == [
            .array([.string("beginPath")]),
            .array([.string("moveTo")] + n(1, 2)),
            .array([.string("lineTo")] + n(3, 4)),
            .array([.string("bezierCurveTo")] + n(1, 2, 3, 4, 5, 6)),
            .array([.string("quadraticCurveTo")] + n(1, 2, 3, 4)),
            .array([.string("arc")] + n(1, 2, 3, 0, 1.5) + [.bool(false)]),
            .array([.string("arc")] + n(1, 2, 3, 0, 1.5) + [.bool(true)]),
            .array([.string("rect")] + n(1, 2, 3, 4)),
            .array([.string("roundRect")] + n(1, 2, 3, 4, 5)),
            .array([.string("closePath")]),
            .array([.string("fill")]),
            .array([.string("stroke")]),
            .array([.string("save")]),
            .array([.string("translate")] + n(5, 6)),
            .array([.string("scale")] + n(2, 3)),
            .array([.string("rotate")] + n(0.5)),
            .array([.string("restore")]),
            .array([.string("fillText"), .string("hi")] + n(7, 8)),
            .array([.string("moveTo")] + n(9, 9)),
        ])
        #expect(node.props["height"] == .number(240))
        #expect(node.props["canvasKey"] == .string("canvas"))
        #expect(node.props["draw"] == nil)
    }

    @Test func styleFieldsRecordSetCommandsAndRestoreReadsBack() throws {
        let runtime = try PaneRuntime(source: """
        return { view = function()
            local seen = {}
            local node = ui.Canvas{ draw = function(ctx, w, h)
                seen.fill0, seen.font0, seen.alpha0 = ctx.fillStyle, ctx.font, ctx.globalAlpha
                ctx.fillStyle = 'red'
                ctx:save()
                ctx.fillStyle = '#00f'
                ctx.font = 'bold 13px sans-serif'
                ctx.lineWidth = 3
                ctx.globalAlpha = 0.5
                ctx.strokeStyle = 'accent'
                seen.fill1 = ctx.fillStyle
                ctx:restore()
                seen.fill2, seen.font2, seen.width2 = ctx.fillStyle, ctx.font, ctx.lineWidth
            end }
            node.seen = seen
            return node
        end }
        """)
        let node = try runtime.view()
        #expect(node.props["seen"] == .object([
            "fill0": .string("fg"), "font0": .string("10px sans-serif"), "alpha0": .number(1),
            "fill1": .string("#00f"), "fill2": .string("red"), "font2": .string("10px sans-serif"),
            "width2": .number(1),
        ]))
        func set(_ field: String, _ value: JSONValue) -> JSONValue { .array([.string("set"), .string(field), value]) }
        #expect(commands(node) == [
            set("fillStyle", .string("red")), .array([.string("save")]),
            set("fillStyle", .string("#00f")), set("font", .string("bold 13px sans-serif")),
            set("lineWidth", .number(3)), set("globalAlpha", .number(0.5)), set("strokeStyle", .string("accent")),
            .array([.string("restore")]),
        ])
    }

    @Test func measureTextGrowsWithTextAndFontSize() throws {
        let runtime = try PaneRuntime(source: """
        return { view = function()
            local out = {}
            ui.Canvas{ draw = function(ctx)
                out.short = ctx:measureText('ab')
                out.long = ctx:measureText('abcdefgh')
                ctx.font = '30px sans-serif'
                out.big = ctx:measureText('ab')
            end }
            return ui.Text{ text = 'x', out = out }
        end }
        """)
        let node = try runtime.view()
        guard case let .object(out)? = node.props["out"],
              case let .object(short)? = out["short"], case let .object(long)? = out["long"],
              case let .object(big)? = out["big"],
              case let .number(sw)? = short["width"], case let .number(lw)? = long["width"],
              case let .number(bw)? = big["width"],
              case let .number(ascent)? = short["fontBoundingBoxAscent"],
              case let .number(descent)? = short["fontBoundingBoxDescent"] else {
            Issue.record("measureText did not return its table: \(node.props)")
            return
        }
        #expect(sw > 0)
        #expect(lw > sw)
        #expect(bw > sw)
        #expect(ascent > 0 && descent > 0)
    }

    @Test func setCanvasSizeChangesWhatDrawGetsAndReportsChange() throws {
        let runtime = try PaneRuntime(source: """
        return { view = function()
            return ui.Canvas{ key = 'chart', height = 100, draw = function(ctx, w, h) ctx:rect(0, 0, w, h) end }
        end }
        """)
        #expect(commands(try runtime.view()) == [.array([.string("rect")] + n(0, 0, 0, 100))])
        #expect(runtime.setCanvasSize(key: "chart", width: 320, height: 200))
        #expect(commands(try runtime.view()) == [.array([.string("rect")] + n(0, 0, 320, 200))])
        #expect(!runtime.setCanvasSize(key: "chart", width: 320.2, height: 200))
        #expect(runtime.setCanvasSize(key: "chart", width: 321, height: 200))
        #expect(runtime.setCanvasSize(key: "other", width: 5, height: 5))
        try runtime.reload(source: """
        return { view = function()
            return ui.Canvas{ key = 'chart', draw = function(ctx, w, h) ctx:rect(0, 0, w, h) end }
        end }
        """)
        #expect(commands(try runtime.view()) == [.array([.string("rect")] + n(0, 0, 321, 200))])
    }

    @Test func drawImageSendsTheBufferAsBase64() throws {
        let node = try pane("""
        local data = buffer.create(8)
        for i = 0, 7 do buffer.writeu8(data, i, i * 10) end
        ctx:drawImage({ width = 2, height = 1, data = data }, 3, 4)
        ctx:drawImage({ width = 2, height = 1, data = data }, 3, 4, 20, 10)
        """).view()
        let bytes = Data([0, 10, 20, 30, 40, 50, 60, 70]).base64EncodedString()
        #expect(commands(node) == [
            .array([.string("drawImage"), .string(bytes)] + n(2, 1, 3, 4, 2, 1)),
            .array([.string("drawImage"), .string(bytes)] + n(2, 1, 3, 4, 20, 10)),
        ])
    }

    @Test func aMismatchedImageBufferRaisesAnErrorNamingTheSizes() throws {
        let runtime = try pane("ctx:drawImage({ width = 2, height = 2, data = buffer.create(8) }, 0, 0)")
        #expect {
            try runtime.view()
        } throws: { error in
            let message = "\(error)"
            return message.contains("drawImage") && message.contains("2x2") && message.contains("16")
        }
    }

    @Test func badArgumentsRaiseAnErrorNamingTheMethod() throws {
        let runtime = try pane("ctx:lineTo(1, 'x')")
        #expect {
            try runtime.view()
        } throws: { "\($0)".contains("ctx:lineTo expects numbers") }
        let style = try pane("ctx.lineWidth = 'wide'")
        #expect {
            try style.view()
        } throws: { "\($0)".contains("ctx.lineWidth") }
    }

    @Test func aCanvasWithoutDrawPassesUnchanged() throws {
        let runtime = try PaneRuntime(source: """
        return { view = function()
            return ui.Canvas{ ops = { { 'line' } }, hits = {}, key = 'k' }
        end }
        """)
        let node = try runtime.view()
        #expect(node.component == "Canvas")
        #expect(node.props["ops"] == .array([.array([.string("line")])]))
        #expect(node.props["commands"] == nil)
        #expect(node.props["key"] == .string("k"))
    }

    @Test func onPointerBecomesPointerActionAndKeyBecomesCanvasKey() throws {
        let node = try pane("", canvasArgs: "key = 'c1', height = 90, on_pointer = { type = 'pointer' }, modifiers = { 1 },").view()
        #expect(node.props["pointerAction"] == .object(["type": .string("pointer")]))
        #expect(node.props["canvasKey"] == .string("c1"))
        #expect(node.props["height"] == .number(90))
        #expect(node.props["on_pointer"] == nil && node.props["key"] == nil)
        #expect(node.props["modifiers"] == .array([.number(1)]))
        #expect(commands(node) == [])
    }

    @Test func theCanvasFixtureDrawsAndShowsThePointerEvent() throws {
        let path = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("tools/fixtures/plugins/canvas/main.luau")
        let runtime = try PaneRuntime(source: String(contentsOf: path, encoding: .utf8))
        try runtime.send(.object(["type": .string("pointer"), "kind": .string("down"), "x": .number(12.7), "y": .number(40)]))
        _ = runtime.setCanvasSize(key: "main", width: 300, height: 200)
        let tree = try runtime.view()
        #expect(tree.children[0].props["text"] == .string("pointer down 12 40"))
        let canvas = tree.children[1]
        let names = Set(commands(canvas).compactMap { command -> String? in
            if case let .array(items) = command, case let .string(name)? = items.first { name } else { nil }
        })
        for name in ["beginPath", "moveTo", "lineTo", "bezierCurveTo", "quadraticCurveTo", "arc", "rect", "roundRect",
                     "closePath", "fill", "stroke", "save", "restore", "translate", "scale", "rotate", "fillText",
                     "drawImage", "set"] {
            #expect(names.contains(name), "\(name)")
        }
        #expect(canvas.props["pointerAction"] == .object(["type": .string("pointer")]))
    }

    @Test func aLargeDrawRendersAndTooManyCommandsRaiseTheNamedError() throws {
        let big = try pane("for i = 1, 40000 do ctx:lineTo(i, i) end").view()
        #expect(commands(big).count == 40_000)
        let runtime = try pane("for i = 1, 260000 do ctx:lineTo(i, i) end")
        #expect {
            try runtime.view()
        } throws: { "\($0)".contains("250,000") }
    }

    @Test func aMeasureLoopTripsTheStepBudget() throws {
        let runtime = try PaneRuntime(source: """
        return { view = function()
            return ui.Canvas{ draw = function(ctx)
                for i = 1, 100000 do ctx:measureText('t' .. i) end
            end }
        end }
        """, limits: .init(steps: 1_000_000))
        #expect {
            try runtime.view()
        } throws: { error in
            if case PaneRuntime.Failure.stepBudget = error { true } else { false }
        }
    }

    @Test func measureTextRefusesMoreThanTenThousandBytes() throws {
        let runtime = try pane("ctx:measureText(string.rep('a', 10001))")
        #expect {
            try runtime.view()
        } throws: { "\($0)".contains("ctx:measureText takes at most 10,000 bytes of text") }
        _ = try pane("ctx:measureText(string.rep('a', 10000))").view()
    }

    @Test func drawImageCopiesTheBufferAtCallTime() throws {
        let node = try pane("""
        local data = buffer.create(4)
        buffer.writeu8(data, 0, 1)
        ctx:drawImage({ width = 1, height = 1, data = data }, 0, 0)
        buffer.writeu8(data, 0, 200)
        ctx:drawImage({ width = 1, height = 1, data = data }, 0, 0)
        """).view()
        let list = commands(node)
        #expect(list.count == 2)
        guard case let .array(a) = list[0], case let .array(b) = list[1] else { return }
        #expect(a[1] != b[1])
    }

    @Test func drawImageRefusesImagesOver2048() throws {
        let runtime = try pane("ctx:drawImage({ width = 2049, height = 1, data = buffer.create(2049 * 4) }, 0, 0)")
        #expect {
            try runtime.view()
        } throws: { "\($0)".contains("2048") }
    }

    @Test func anUnknownCtxMemberNamesItself() throws {
        let runtime = try pane("ctx:fillRect(0, 0, 1, 1)")
        #expect {
            try runtime.view()
        } throws: { error in
            "\(error)".contains("ctx has no field or method 'fillRect' (see docs/plugin-runtime.md for the Canvas 2D calls ctx has)")
        }
    }

    @Test func aCommandListTheScriptGrewNamesItself() throws {
        // The recorder cannot pass the limit (250,000 commands of at most 8 values); a table appended after ui.Canvas returned can.
        let runtime = try PaneRuntime(source: """
        return { view = function()
            local c = ui.Canvas{ draw = function(ctx) ctx:beginPath() end }
            local big = {}
            for i = 1, 300000 do big[i] = i end
            for _ = 1, 8 do table.insert(c.commands, big) end
            return c
        end }
        """, limits: .init(memory: 512 << 20, steps: 100_000_000))
        #expect {
            try runtime.view()
        } throws: { "\($0)".contains("a Canvas holds more than 2,000,000 values in its commands") }
    }

    @Test func theStepBudgetStopsAnInfiniteLoopInDraw() throws {
        let runtime = try PaneRuntime(source: """
        return { view = function()
            return ui.Canvas{ draw = function(ctx) while true do end end }
        end }
        """, limits: .init(steps: 100_000))
        #expect {
            try runtime.view()
        } throws: { error in
            if case PaneRuntime.Failure.stepBudget = error { true } else { false }
        }
    }
}

@Suite struct CanvasFontTests {
    @Test func parsesTheCSSShorthand() {
        #expect(CanvasFont("bold 13px sans-serif") == CanvasFont(size: 13, weight: 700))
        #expect(CanvasFont("600 11px monospace") == CanvasFont(size: 11, weight: 600, design: .monospaced))
        #expect(CanvasFont("italic 12px serif") == CanvasFont(size: 12, italic: true, design: .serif))
        #expect(CanvasFont("") == CanvasFont(size: 10))
    }
}
