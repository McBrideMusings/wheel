import A2UISwiftCore
import A2UISwiftUI
import SwiftUI

// MARK: Canvas — a display list the server draws with, a camera the client moves, and hit regions the server names (ADR-0029)
//
// props:
//   ops     [{op, minZoom?, maxZoom?, fill?, stroke?, width?, dash?, opacity?, px?, ...shapes}] drawn in order, in scene units:
//             rect    r: [[x, y, w, h]...]        radius?
//             circle  c: [[x, y, r]...]
//             path    d: ["M0 0 L10 0 C 10 5 5 10 0 10 Z"...]   (SVG path data: M L C Q Z, absolute)
//             text    t: [[x, y, "label"]...]     size?, weight?, anchor? start|middle|end, mono?
//           A colour is a tone (ok warn bad muted fg faint accent purple line group fill) or "#rrggbb[aa]". `width` and `size` are scene
//           units, or screen points when `px` is true. An op is skipped outside its minZoom...maxZoom.
//   hits    [{id, rect: [x,y,w,h] | circle: [cx,cy,r], action, context?, label?, minZoom?, maxZoom?, slop?}] topmost last
//   bounds  [x, y, w, h] the scene the camera fits to; `focus` {x,y,w,h,key} fits that rect again whenever `key` changes
//   controls true draws Fit, + and −.   emptyAction raised with {} when a tap hits nothing.
// A tap on a hit raises its `action` with its `context` plus {id}. The pointer's hover outlines the hit under it.

struct CanvasCamera: Equatable {
    var x = 0.0, y = 0.0, s = 1.0
    var auto = true
}

struct CanvasStyle {
    var fill: String?, stroke: String?
    var width = 1.0, opacity = 1.0, radius = 0.0, size = 12.0
    var dash: [Double] = []
    var px = false, mono = false
    var weight = "regular", anchor = "start"
    var minZoom = 0.0, maxZoom = Double.infinity
}

struct CanvasOp {
    var kind: String
    var style: CanvasStyle
    var nums: [[Double]] = []
    var paths: [String] = []
    var texts: [(Double, Double, String)] = []
}

struct CanvasHit {
    var id: String, action: String, label: String
    var context: [String: AnyCodable]
    var rect: CGRect?, circle: (Double, Double, Double)?
    var slop = 0.0, minZoom = 0.0, maxZoom = Double.infinity
}

struct CanvasScene {
    var ops: [CanvasOp] = []
    var hits: [CanvasHit] = []
    var bounds = CGRect(x: 0, y: 0, width: 100, height: 100)
    var focus: CGRect?
    var focusKey = ""
    var controls = true
    var emptyAction = ""

    init(_ p: [String: AnyCodable]) {
        func nums(_ v: AnyCodable?) -> [Double] { (v?.arrayValue ?? []).compactMap { $0.numberValue } }
        func rectOf(_ v: AnyCodable?) -> CGRect? { let n = nums(v); return n.count == 4 ? CGRect(x: n[0], y: n[1], width: n[2], height: n[3]) : nil }
        ops = (p["ops"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.map { d in
            var st = CanvasStyle()
            st.fill = d["fill"]?.stringValue; st.stroke = d["stroke"]?.stringValue
            st.width = d["width"]?.numberValue ?? 1; st.opacity = d["opacity"]?.numberValue ?? 1
            st.radius = d["radius"]?.numberValue ?? 0; st.size = d["size"]?.numberValue ?? 12
            st.dash = nums(d["dash"]); st.px = d["px"]?.boolValue ?? false; st.mono = d["mono"]?.boolValue ?? false
            st.weight = d["weight"]?.stringValue ?? "regular"; st.anchor = d["anchor"]?.stringValue ?? "start"
            st.minZoom = d["minZoom"]?.numberValue ?? 0; st.maxZoom = d["maxZoom"]?.numberValue ?? .infinity
            var op = CanvasOp(kind: d["op"]?.stringValue ?? "", style: st)
            switch op.kind {
            case "rect": op.nums = (d["r"]?.arrayValue ?? []).map { nums($0) }
            case "circle": op.nums = (d["c"]?.arrayValue ?? []).map { nums($0) }
            case "path": op.paths = (d["d"]?.arrayValue ?? []).compactMap { $0.stringValue }
            case "text":
                op.texts = (d["t"]?.arrayValue ?? []).compactMap { $0.arrayValue }.compactMap { a in
                    guard a.count == 3, let x = a[0].numberValue, let y = a[1].numberValue, let s = a[2].stringValue else { return nil }
                    return (x, y, s)
                }
            default: break
            }
            return op
        }
        hits = (p["hits"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.map { d in
            var h = CanvasHit(id: d["id"]?.stringValue ?? "", action: d["action"]?.stringValue ?? "", label: d["label"]?.stringValue ?? "",
                              context: d["context"]?.dictionaryValue ?? [:])
            h.rect = rectOf(d["rect"])
            let c = nums(d["circle"]); if c.count == 3 { h.circle = (c[0], c[1], c[2]) }
            h.slop = d["slop"]?.numberValue ?? 0
            h.minZoom = d["minZoom"]?.numberValue ?? 0; h.maxZoom = d["maxZoom"]?.numberValue ?? .infinity
            return h
        }
        if let b = rectOf(p["bounds"]) { bounds = b }
        if let f = p["focus"]?.dictionaryValue {
            focus = rectOf(f["rect"]) ?? CGRect(x: f["x"]?.numberValue ?? 0, y: f["y"]?.numberValue ?? 0,
                                               width: f["w"]?.numberValue ?? 1, height: f["h"]?.numberValue ?? 1)
            focusKey = f["key"]?.stringValue ?? ""
        }
        controls = p["controls"]?.boolValue ?? true
        emptyAction = p["emptyAction"]?.stringValue ?? ""
    }
}

/// SVG path data with absolute M, L, C, Q and Z.
func canvasPath(_ d: String) -> Path {
    var path = Path()
    let scanner = Scanner(string: d)
    scanner.charactersToBeSkipped = CharacterSet(charactersIn: " ,\n\t")
    var cmd: Character = "M"
    func num() -> Double? { scanner.scanDouble() }
    while !scanner.isAtEnd {
        if let c = scanner.scanCharacters(from: CharacterSet(charactersIn: "MLCQZmlcqz")), let last = c.last { cmd = last }
        switch cmd {
        case "M": if let x = num(), let y = num() { path.move(to: CGPoint(x: x, y: y)); cmd = "L" } else { return path }
        case "L": if let x = num(), let y = num() { path.addLine(to: CGPoint(x: x, y: y)) } else { return path }
        case "C":
            if let a = num(), let b = num(), let c = num(), let e = num(), let x = num(), let y = num() {
                path.addCurve(to: CGPoint(x: x, y: y), control1: CGPoint(x: a, y: b), control2: CGPoint(x: c, y: e))
            } else { return path }
        case "Q":
            if let a = num(), let b = num(), let x = num(), let y = num() {
                path.addQuadCurve(to: CGPoint(x: x, y: y), control: CGPoint(x: a, y: b))
            } else { return path }
        case "Z", "z": path.closeSubpath()
        default: return path
        }
    }
    return path
}

func canvasColor(_ name: String?) -> Color? {
    guard let name, !name.isEmpty else { return nil }
    let t = WebTheme.shared
    switch name {
    case "ok": return t.green
    case "warn": return t.orange
    case "bad": return t.red
    case "muted": return t.secondary
    case "fg": return t.label
    case "faint": return t.tertiary
    case "accent": return t.blue
    case "purple": return t.purple
    case "line": return t.separatorStrong
    case "group": return t.group
    case "fill": return t.fill
    default:
        guard name.hasPrefix("#") else { return nil }
        let hex = String(name.dropFirst())
        guard hex.count == 6 || hex.count == 8, let v = UInt64(hex, radix: 16) else { return nil }
        let a = hex.count == 8 ? Double(v & 0xff) / 255 : 1
        let rgb = hex.count == 8 ? v >> 8 : v
        return Color(.sRGB, red: Double((rgb >> 16) & 0xff) / 255, green: Double((rgb >> 8) & 0xff) / 255, blue: Double(rgb & 0xff) / 255, opacity: a)
    }
}

struct A2UICanvasView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        CanvasStage(scene: CanvasScene(p), height: p["height"]?.numberValue ?? 480) { name, context in
            surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: context)
        }
    }
}

private struct CanvasStage: View {
    let scene: CanvasScene
    let height: Double
    let fire: (String, [String: AnyCodable]) -> Void
    @State private var cam = CanvasCamera()
    @State private var dragFrom: CanvasCamera?
    @State private var dragMoved = false
    @State private var hover: CGPoint?
    @State private var size = CGSize.zero
    @State private var lastFocus = ""

    private func fitted(_ rect: CGRect, in size: CGSize) -> CanvasCamera {
        guard size.width > 0, size.height > 0, rect.width > 0, rect.height > 0 else { return CanvasCamera() }
        let s = min(size.width / rect.width, size.height / rect.height) * 0.96
        return CanvasCamera(x: (size.width - rect.width * s) / 2 - rect.minX * s,
                            y: (size.height - rect.height * s) / 2 - rect.minY * s, s: s, auto: false)
    }

    private func effective(_ size: CGSize) -> CanvasCamera {
        if cam.auto { var c = fitted(scene.bounds, in: size); c.auto = true; return c }
        return cam
    }

    private func zoom(_ f: Double, about p: CGPoint) {
        var c = effective(size)
        let s = max(0.002, min(8, c.s * f)), k = s / c.s
        c.x = p.x - (p.x - c.x) * k; c.y = p.y - (p.y - c.y) * k; c.s = s; c.auto = false
        cam = c
    }

    private func hit(at p: CGPoint, _ c: CanvasCamera) -> CanvasHit? {
        for h in scene.hits.reversed() where c.s >= h.minZoom && c.s <= h.maxZoom {
            if let r = h.rect {
                let sr = CGRect(x: r.minX * c.s + c.x, y: r.minY * c.s + c.y, width: r.width * c.s, height: r.height * c.s).insetBy(dx: -h.slop, dy: -h.slop)
                if sr.contains(p) { return h }
            } else if let ci = h.circle {
                let dx = p.x - (ci.0 * c.s + c.x), dy = p.y - (ci.1 * c.s + c.y), r = ci.2 * c.s + h.slop
                if dx * dx + dy * dy <= r * r { return h }
            }
        }
        return nil
    }

    private func draw(_ ctx: inout GraphicsContext, _ c: CanvasCamera) {
        let t = WebTheme.shared
        for op in scene.ops where c.s >= op.style.minZoom && c.s <= op.style.maxZoom {
            let st = op.style
            let fill = canvasColor(st.fill)?.opacity(st.opacity), stroke = canvasColor(st.stroke)?.opacity(st.opacity)
            let lw = st.px ? st.width : st.width * c.s
            let style = StrokeStyle(lineWidth: lw, lineCap: .round, lineJoin: .round, dash: st.dash.map { st.px ? $0 : $0 * c.s })
            func paint(_ path: Path) {
                if let fill { ctx.fill(path, with: .color(fill)) }
                if let stroke { ctx.stroke(path, with: .color(stroke), style: style) }
            }
            switch op.kind {
            case "rect":
                for r in op.nums where r.count == 4 {
                    let rect = CGRect(x: r[0] * c.s + c.x, y: r[1] * c.s + c.y, width: r[2] * c.s, height: r[3] * c.s)
                    paint(Path(roundedRect: rect, cornerRadius: st.radius * c.s))
                }
            case "circle":
                for r in op.nums where r.count == 3 {
                    let rr = r[2] * c.s
                    paint(Path(ellipseIn: CGRect(x: r[0] * c.s + c.x - rr, y: r[1] * c.s + c.y - rr, width: rr * 2, height: rr * 2)))
                }
            case "path":
                for d in op.paths {
                    paint(canvasPath(d).applying(CGAffineTransform(a: c.s, b: 0, c: 0, d: c.s, tx: c.x, ty: c.y)))
                }
            case "text":
                let size = st.px ? st.size : st.size * c.s
                let weight: Font.Weight = st.weight == "bold" ? .bold : st.weight == "semibold" ? .semibold : st.weight == "medium" ? .medium : .regular
                let color = canvasColor(st.fill) ?? t.label
                let anchor: UnitPoint = st.anchor == "middle" ? .center : st.anchor == "end" ? .trailing : .leading
                for (x, y, s) in op.texts {
                    let text = Text(s).font(.system(size: size, weight: weight, design: st.mono ? .monospaced : .default)).foregroundColor(color.opacity(st.opacity))
                    ctx.draw(text, at: CGPoint(x: x * c.s + c.x, y: y * c.s + c.y), anchor: anchor)
                }
            default: break
            }
        }
        if let p = hover, let h = hit(at: p, c) {
            var path = Path()
            if let r = h.rect {
                path = Path(roundedRect: CGRect(x: r.minX * c.s + c.x, y: r.minY * c.s + c.y, width: r.width * c.s, height: r.height * c.s).insetBy(dx: -h.slop, dy: -h.slop), cornerRadius: 3)
            } else if let ci = h.circle {
                let rr = ci.2 * c.s + h.slop
                path = Path(ellipseIn: CGRect(x: ci.0 * c.s + c.x - rr, y: ci.1 * c.s + c.y - rr, width: rr * 2, height: rr * 2))
            }
            ctx.stroke(path, with: .color(t.blue), lineWidth: 1.5)
        }
    }

    var body: some View {
        let t = WebTheme.shared
        GeometryReader { geo in
            let c = effective(geo.size)
            ZStack(alignment: .topTrailing) {
                Canvas { ctx, _ in draw(&ctx, c) }
                    .frame(width: geo.size.width, height: geo.size.height)
                    .contentShape(Rectangle())
                    .gesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { v in
                                if dragFrom == nil { dragFrom = c; dragMoved = false }
                                if abs(v.translation.width) + abs(v.translation.height) > 4 { dragMoved = true }
                                if dragMoved, var d = dragFrom {
                                    d.x += v.translation.width; d.y += v.translation.height; d.auto = false
                                    cam = d
                                }
                            }
                            .onEnded { v in
                                defer { dragFrom = nil }
                                guard !dragMoved, let start = dragFrom else { return }
                                if let h = hit(at: v.location, start) {
                                    var ctxt = h.context
                                    ctxt["id"] = .string(h.id)
                                    fire(h.action, ctxt)
                                } else if !scene.emptyAction.isEmpty {
                                    fire(scene.emptyAction, [:])
                                }
                            }
                    )
                    .simultaneousGesture(
                        MagnifyGesture().onChanged { v in zoom(v.magnification > 1 ? 1.03 : 1 / 1.03, about: v.startLocation) }
                    )
                    #if os(macOS)
                    .onContinuousHover { phase in
                        switch phase {
                        case let .active(p): hover = p
                        case .ended: hover = nil
                        }
                    }
                    #endif
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(scene.hits.first?.label ?? "Canvas")
                if scene.controls {
                    HStack(spacing: 4) {
                        control("Fit") { cam = CanvasCamera() }
                        control("+") { zoom(1.4, about: CGPoint(x: geo.size.width / 2, y: geo.size.height / 2)) }
                        control("−") { zoom(1 / 1.4, about: CGPoint(x: geo.size.width / 2, y: geo.size.height / 2)) }
                    }
                    .padding(8)
                }
            }
            .background(t.fill.opacity(0.5))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(t.separatorStrong, lineWidth: 1))
            .onAppear { size = geo.size; applyFocus(geo.size) }
            .onChange(of: geo.size) { _, new in size = new }
            .onChange(of: scene.focusKey) { _, _ in applyFocus(geo.size) }
        }
        .frame(height: height)
    }

    private func applyFocus(_ size: CGSize) {
        guard let f = scene.focus, scene.focusKey != lastFocus else { return }
        lastFocus = scene.focusKey
        cam = fitted(f, in: size)
    }

    private func control(_ title: String, _ action: @escaping () -> Void) -> some View {
        let t = WebTheme.shared
        return Button(action: action) {
            Text(title).font(.system(size: WebTheme.callout)).foregroundStyle(t.label)
                .padding(.horizontal, 8).frame(height: 24)
                .background(t.group, in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(t.separatorStrong, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }
}
