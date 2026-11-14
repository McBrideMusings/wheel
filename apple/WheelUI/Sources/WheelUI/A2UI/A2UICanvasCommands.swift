import SwiftUI
internal import WheelLuau
#if os(macOS)
import AppKit
#endif

// MARK: Canvas `commands` — a Canvas 2D command list a pane script recorded (docs/plugin-runtime.md), replayed as written
//
// Each command is an array whose first element names it: ["set", field, value] (fillStyle strokeStyle lineWidth font globalAlpha),
// ["beginPath"], ["moveTo", x, y], ["lineTo", x, y], ["bezierCurveTo", c1x, c1y, c2x, c2y, x, y], ["quadraticCurveTo", cx, cy, x, y],
// ["arc", x, y, r, a0, a1, ccw], ["rect", x, y, w, h], ["roundRect", x, y, w, h, r], ["closePath"], ["fill"], ["stroke"],
// ["save"], ["restore"], ["translate", x, y], ["scale", x, y], ["rotate", a], ["fillText", text, x, y],
// ["drawImage", base64 RGBA, iw, ih, x, y, w, h]. A command with the wrong arguments is dropped.

enum CanvasCommand: Equatable {
    case fillStyle(String), strokeStyle(String), lineWidth(Double), font(String), globalAlpha(Double)
    case beginPath, closePath, fill, stroke, save, restore
    case moveTo(Double, Double), lineTo(Double, Double)
    case bezierCurveTo(Double, Double, Double, Double, Double, Double)
    case quadraticCurveTo(Double, Double, Double, Double)
    case arc(Double, Double, Double, Double, Double, Bool)
    case rect(Double, Double, Double, Double)
    case roundRect(Double, Double, Double, Double, Double)
    case translate(Double, Double), scale(Double, Double), rotate(Double)
    case fillText(String, Double, Double)
    case drawImage(String, Int, Int, Double, Double, Double, Double)
}

/// The commands in `list` that parse, in order.
func canvasCommands(_ list: [AnyCodable]) -> [CanvasCommand] {
    list.compactMap { item -> CanvasCommand? in
        guard let a = item.arrayValue, let name = a.first?.stringValue else { return nil }
        let rest = Array(a.dropFirst())
        func n(_ k: Int) -> [Double]? {
            guard rest.count == k else { return nil }
            let v = rest.compactMap { $0.numberValue }
            return v.count == k ? v : nil
        }
        switch name {
        case "set":
            guard rest.count == 2, let field = rest[0].stringValue else { return nil }
            switch field {
            case "fillStyle": return rest[1].stringValue.map(CanvasCommand.fillStyle)
            case "strokeStyle": return rest[1].stringValue.map(CanvasCommand.strokeStyle)
            case "font": return rest[1].stringValue.map(CanvasCommand.font)
            case "lineWidth": return rest[1].numberValue.map(CanvasCommand.lineWidth)
            case "globalAlpha": return rest[1].numberValue.map(CanvasCommand.globalAlpha)
            default: return nil
            }
        case "beginPath": return rest.isEmpty ? .beginPath : nil
        case "closePath": return rest.isEmpty ? .closePath : nil
        case "fill": return rest.isEmpty ? .fill : nil
        case "stroke": return rest.isEmpty ? .stroke : nil
        case "save": return rest.isEmpty ? .save : nil
        case "restore": return rest.isEmpty ? .restore : nil
        case "moveTo": return n(2).map { .moveTo($0[0], $0[1]) }
        case "lineTo": return n(2).map { .lineTo($0[0], $0[1]) }
        case "bezierCurveTo": return n(6).map { .bezierCurveTo($0[0], $0[1], $0[2], $0[3], $0[4], $0[5]) }
        case "quadraticCurveTo": return n(4).map { .quadraticCurveTo($0[0], $0[1], $0[2], $0[3]) }
        case "arc":
            guard rest.count == 6, let ccw = rest[5].boolValue else { return nil }
            let v = rest.prefix(5).compactMap { $0.numberValue }
            return v.count == 5 ? .arc(v[0], v[1], v[2], v[3], v[4], ccw) : nil
        case "rect": return n(4).map { .rect($0[0], $0[1], $0[2], $0[3]) }
        case "roundRect": return n(5).map { .roundRect($0[0], $0[1], $0[2], $0[3], $0[4]) }
        case "translate": return n(2).map { .translate($0[0], $0[1]) }
        case "scale": return n(2).map { .scale($0[0], $0[1]) }
        case "rotate": return n(1).map { .rotate($0[0]) }
        case "fillText":
            guard rest.count == 3, let t = rest[0].stringValue, let x = rest[1].numberValue, let y = rest[2].numberValue else { return nil }
            return .fillText(t, x, y)
        case "drawImage":
            guard rest.count == 7, let data = rest[0].stringValue else { return nil }
            let v = rest.dropFirst().compactMap { $0.numberValue }
            // The sizes the Luau side allows: whole numbers 1...2048, so the pixel count can neither trap nor overflow.
            guard v.count == 6, (1...2048).contains(v[0]), (1...2048).contains(v[1]), v[0] == v[0].rounded(), v[1] == v[1].rounded() else { return nil }
            return .drawImage(data, Int(v[0]), Int(v[1]), v[2], v[3], v[4], v[5])
        default: return nil
        }
    }
}

/// RGBA in 0...1 for a CSS colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(r, g, b)`, `rgba(r, g, b, a)` and a few names.
func canvasRGBA(_ s: String) -> (Double, Double, Double, Double)? {
    let t = s.trimmingCharacters(in: .whitespaces).lowercased()
    switch t {
    case "black": return (0, 0, 0, 1)
    case "white": return (1, 1, 1, 1)
    case "red": return (1, 0, 0, 1)
    case "green": return (0, 128.0 / 255, 0, 1)
    case "blue": return (0, 0, 1, 1)
    case "transparent": return (0, 0, 0, 0)
    default: break
    }
    if t.hasPrefix("#") {
        let hex = Array(t.dropFirst())
        guard hex.allSatisfy({ $0.isHexDigit }) else { return nil }
        func h(_ c: [Character]) -> Double { Double(Int(String(c), radix: 16) ?? 0) / 255 }
        switch hex.count {
        case 3, 4:
            let v = hex.map { h([$0, $0]) }
            return (v[0], v[1], v[2], hex.count == 4 ? v[3] : 1)
        case 6, 8:
            let v = stride(from: 0, to: hex.count, by: 2).map { h([hex[$0], hex[$0 + 1]]) }
            return (v[0], v[1], v[2], hex.count == 8 ? v[3] : 1)
        default: return nil
        }
    }
    for prefix in ["rgba(", "rgb("] where t.hasPrefix(prefix) && t.hasSuffix(")") {
        let inner = t.dropFirst(prefix.count).dropLast()
        let parts = inner.split(separator: ",").map { Double($0.trimmingCharacters(in: .whitespaces)) }
        guard parts.allSatisfy({ $0 != nil }) else { return nil }
        let v = parts.map { $0! }
        if v.count == 3 { return (v[0] / 255, v[1] / 255, v[2] / 255, 1) }
        if v.count == 4 { return (v[0] / 255, v[1] / 255, v[2] / 255, max(0, min(1, v[3]))) }
        return nil
    }
    return nil
}

/// A fillStyle or strokeStyle as a colour: a tone `canvasColor` knows, else a CSS colour; nil when it is neither.
func canvasParseColor(_ s: String) -> Color? {
    if !s.hasPrefix("#"), let tone = canvasColor(s) { return tone }
    guard let c = canvasRGBA(s) else { return nil }
    return Color(.sRGB, red: c.0, green: c.1, blue: c.2, opacity: c.3)
}

extension CanvasFont {
    /// The SwiftUI font that draws the face `measure` measured.
    var swiftUIFont: Font {
        let weight: Font.Weight = switch weightName {
        case "ultraLight": .ultraLight
        case "thin": .thin
        case "light": .light
        case "medium": .medium
        case "semibold": .semibold
        case "bold": .bold
        case "heavy": .heavy
        case "black": .black
        default: .regular
        }
        let design: Font.Design = switch self.design {
        case .monospaced: .monospaced
        case .serif: .serif
        case .rounded: .rounded
        case .default: .default
        }
        let font = Font.system(size: size, weight: weight, design: design)
        return italic ? font.italic() : font
    }
}

/// Each font's ascent, found once.
final class CanvasAscents: @unchecked Sendable {
    private let lock = NSLock()
    private var store: [String: Double] = [:]

    func of(_ font: CanvasFont) -> Double {
        let key = "\(font.size)|\(font.weight)|\(font.italic)|\(font.design)"
        lock.lock(); defer { lock.unlock() }
        if let hit = store[key] { return hit }
        let value = Double(CTFontGetAscent(font.ctFont))
        if store.count > 256 { store.removeAll() }
        store[key] = value
        return value
    }
}

/// One paint the replay produced, in device space.
enum CanvasPaint {
    case fill(Path, Color)
    case stroke(Path, Color, Double)
    case text(String, CanvasFont, Color, CGAffineTransform, CGPoint)
    case image(CGImage, CGRect, CGAffineTransform, Double)
}

/// Decoded drawImage pixels by their base64 text, so a redraw decodes nothing it has decoded before; it holds at most 64 MB of
/// pixels, dropping the oldest first.
final class CanvasImageCache: @unchecked Sendable {
    static let shared = CanvasImageCache()
    private static let maxBytes = 64 << 20
    private let lock = NSLock()
    private var store: [String: CGImage] = [:]
    private var order: [String] = []
    private var bytes = 0

    func image(_ base64: String, width: Int, height: Int) -> CGImage? {
        let key = "\(width)x\(height):\(base64)"
        lock.lock(); defer { lock.unlock() }
        if let hit = store[key] { return hit }
        guard let data = Data(base64Encoded: base64), data.count == width * height * 4,
              let provider = CGDataProvider(data: data as CFData),
              let image = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                                  provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
        else { return nil }
        store[key] = image
        order.append(key)
        bytes += data.count
        while bytes > Self.maxBytes, order.count > 1, let old = store.removeValue(forKey: order.removeFirst()) {
            bytes -= old.bytesPerRow * old.height
        }
        return image
    }
}

/// The commands run once with Canvas 2D semantics into the paints they make: the path is kept in device space (each point mapped by
/// the transform current when it was added), and save/restore push and pop the transform and the styles.
struct CanvasPlayer {
    private struct Style {
        var transform = CGAffineTransform.identity
        var fill: Color, stroke: Color
        var lineWidth = 1.0, alpha = 1.0
        var font = CanvasFont()
    }

    private(set) var paints: [CanvasPaint] = []
    /// The path as it stands after the last command, for a check.
    private(set) var path = Path()

    init(_ commands: [CanvasCommand], fg: Color) {
        var st = Style(fill: fg, stroke: fg)
        var stack: [Style] = []
        func p(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: x, y: y).applying(st.transform) }
        for c in commands {
            switch c {
            case let .fillStyle(s): if let col = canvasParseColor(s) { st.fill = col }
            case let .strokeStyle(s): if let col = canvasParseColor(s) { st.stroke = col }
            case let .lineWidth(w): if w > 0, w.isFinite { st.lineWidth = w }
            case let .font(f): st.font = CanvasFont(f)
            case let .globalAlpha(a): if (0...1).contains(a) { st.alpha = a }
            case .beginPath: path = Path()
            case .closePath: path.closeSubpath()
            case .fill: paints.append(.fill(path, st.fill.opacity(st.alpha)))
            case .stroke:
                let t = st.transform
                let k = sqrt(abs(t.a * t.d - t.b * t.c))
                paints.append(.stroke(path, st.stroke.opacity(st.alpha), st.lineWidth * k))
            case .save: stack.append(st)
            case .restore: if let s = stack.popLast() { st = s }
            case let .moveTo(x, y): path.move(to: p(x, y))
            case let .lineTo(x, y):
                if path.isEmpty { path.move(to: p(x, y)) } else { path.addLine(to: p(x, y)) }
            case let .bezierCurveTo(a, b, cc, d, x, y):
                if path.isEmpty { path.move(to: p(a, b)) }
                path.addCurve(to: p(x, y), control1: p(a, b), control2: p(cc, d))
            case let .quadraticCurveTo(a, b, x, y):
                if path.isEmpty { path.move(to: p(a, b)) }
                path.addQuadCurve(to: p(x, y), control: p(a, b))
            case let .arc(x, y, r, a0, a1, ccw):
                guard r >= 0 else { continue }
                // The arc joins the current subpath with a line to its start, as Canvas 2D does: its elements are appended one by
                // one, mapped by the transform, and the move a fresh Path starts with is dropped.
                var arc = Path()
                arc.addArc(center: CGPoint(x: x, y: y), radius: r, startAngle: .radians(a0), endAngle: .radians(a1), clockwise: ccw)
                let first = CGPoint(x: x + r * cos(a0), y: y + r * sin(a0))
                if path.isEmpty { path.move(to: p(first.x, first.y)) } else { path.addLine(to: p(first.x, first.y)) }
                let t = st.transform
                arc.forEach { element in
                    switch element {
                    case .move: break
                    case let .line(to): path.addLine(to: to.applying(t))
                    case let .quadCurve(to, c): path.addQuadCurve(to: to.applying(t), control: c.applying(t))
                    case let .curve(to, c1, c2): path.addCurve(to: to.applying(t), control1: c1.applying(t), control2: c2.applying(t))
                    case .closeSubpath: break
                    }
                }
            case let .rect(x, y, w, h):
                path.move(to: p(x, y)); path.addLine(to: p(x + w, y)); path.addLine(to: p(x + w, y + h)); path.addLine(to: p(x, y + h))
                path.closeSubpath()
                path.move(to: p(x, y))
            case let .roundRect(x, y, w, h, r):
                let rr = max(0, min(r, abs(w) / 2, abs(h) / 2))
                let shape = Path(roundedRect: CGRect(x: x, y: y, width: w, height: h).standardized, cornerRadius: rr)
                path.addPath(shape, transform: st.transform)
                path.move(to: p(x, y))
            case let .translate(x, y): st.transform = CGAffineTransform(translationX: x, y: y).concatenating(st.transform)
            case let .scale(x, y): st.transform = CGAffineTransform(scaleX: x, y: y).concatenating(st.transform)
            case let .rotate(a): st.transform = CGAffineTransform(rotationAngle: a).concatenating(st.transform)
            case let .fillText(text, x, y): paints.append(.text(text, st.font, st.fill.opacity(st.alpha), st.transform, CGPoint(x: x, y: y)))
            case let .drawImage(data, iw, ih, x, y, w, h):
                if let img = CanvasImageCache.shared.image(data, width: iw, height: ih) {
                    paints.append(.image(img, CGRect(x: x, y: y, width: w, height: h), st.transform, st.alpha))
                }
            }
        }
    }

    /// A font's ascent, read once per font: a frame with many labels builds no text line to find it.
    private static let ascents = CanvasAscents()
    private static func ascent(_ font: CanvasFont) -> Double { ascents.of(font) }

    func draw(_ ctx: inout GraphicsContext) {
        for paint in paints {
            switch paint {
            case let .fill(path, color): ctx.fill(path, with: .color(color))
            case let .stroke(path, color, width): ctx.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: width, lineCap: .butt, lineJoin: .miter))
            case let .text(text, font, color, transform, at):
                var c = ctx
                c.concatenate(transform)
                let resolved = c.resolve(Text(text).font(font.swiftUIFont).foregroundColor(color))
                // Canvas 2D puts (x, y) on the alphabetic baseline at the text's left end.
                let ascent = Self.ascent(font)
                c.draw(resolved, at: CGPoint(x: at.x, y: at.y - ascent), anchor: .topLeading)
            case let .image(image, rect, transform, alpha):
                var c = ctx
                c.concatenate(transform)
                c.opacity = alpha
                c.draw(Image(decorative: image, scale: 1), in: rect)
            }
        }
    }
}

/// The action name a command Canvas reports its size under, followed by its `canvasKey`.
let canvasSizePrefix = "canvas-size:"

extension EnvironmentValues {
    /// Whether the surface takes a command Canvas's size report: a plugin pane does (its runtime hands the size to `draw`); a page
    /// whose server would get an action it never asked for does not.
    @Entry var a2uiCanvasSizes = false
}

/// A Canvas drawn from a recorded command list: `height` tall, as wide as it is offered, clipped to its rect, with no camera,
/// controls, background or border. It reports its size as `canvas-size:<key>` and, with a `pointerAction`, raises it for pointer
/// events with `{kind, x, y, button, dx, dy}`.
struct CanvasCommandStage: View {
    let player: CanvasPlayer
    let height: Double
    let key: String
    let pointerAction: String?
    let fire: (String, [String: AnyCodable]) -> Void
    @State private var reportedSize: CGSize?
    @State private var last: CGPoint?
    @State private var pressed = false
    @State private var stageFrame = CGRect.zero
    @Environment(\.a2uiCanvasSizes) private var reportsSize

    init(commands: [AnyCodable], height: Double, key: String, pointerAction: String?, fire: @escaping (String, [String: AnyCodable]) -> Void) {
        self.player = CanvasPlayer(canvasCommands(commands), fg: WebTheme.shared.label)
        self.height = height
        self.key = key
        self.pointerAction = (pointerAction?.isEmpty ?? true) ? nil : pointerAction
        self.fire = fire
    }

    private func pointer(_ kind: String, _ p: CGPoint, dx: Double = 0, dy: Double = 0) {
        guard let pointerAction else { return }
        fire(pointerAction, ["kind": .string(kind), "x": .number(p.x), "y": .number(p.y), "button": .number(0), "dx": .number(dx), "dy": .number(dy)])
    }

    private func report(_ size: CGSize) {
        guard reportsSize, size != reportedSize else { return }
        reportedSize = size
        fire(canvasSizePrefix + key, ["w": .number(size.width), "h": .number(size.height)])
    }

    var body: some View {
        GeometryReader { geo in
            Canvas { ctx, _ in player.draw(&ctx) }
                .frame(width: geo.size.width, height: geo.size.height)
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { v in
                            if !pressed {
                                pressed = true
                                last = v.startLocation
                                pointer("down", v.startLocation)
                            }
                            let from = last ?? v.startLocation
                            if v.location != from {
                                pointer("drag", v.location, dx: v.location.x - from.x, dy: v.location.y - from.y)
                                last = v.location
                            }
                        }
                        .onEnded { v in
                            pressed = false
                            last = nil
                            pointer("up", v.location)
                        },
                    including: pointerAction == nil ? .subviews : .all
                )
                #if os(macOS)
                .onContinuousHover { phase in
                    guard pointerAction != nil, !pressed, case let .active(p) = phase else { return }
                    pointer("move", p)
                }
                #endif
                .clipped()
                .onAppear { report(geo.size) }
                .onChange(of: geo.size) { _, new in report(new) }
                .background(GeometryReader { g in Color.clear.onAppear { stageFrame = g.frame(in: .global) }.onChange(of: g.frame(in: .global)) { _, f in stageFrame = f } })
                #if os(macOS)
                .background {
                    if pointerAction != nil {
                        CanvasScrollCatcher(frame: stageFrame) { dx, dy, p in pointer("wheel", p, dx: dx, dy: dy) }
                    }
                }
                #endif
        }
        .frame(height: height)
    }
}
