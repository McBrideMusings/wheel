import SwiftUI
#if os(macOS)
import AppKit
#endif

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
// Screen-space sizes: `rpx` [min, max] on a circle op or a circle hit draws the scene radius clamped to that many screen points;
//   `hpx` / `wpx` [min, max] on a rect op clamp its height (kept centred on the rect) or width (kept at its left edge);
//   `sizepx` [min, max] on a text op clamps `size`. `maxw` on a text op cuts the label with "…" to fit that many scene units; `minx` keeps its left edge at least that many screen points
//   from the view's left (a label that stays in view while its row scrolls past).
// A shape outside the view is not drawn. A hit may also carry `tip` (lines shown beside the pointer; a line is a string or
// [{t, tone?, mono?, bold?}] runs), `doubleAction` / `doubleContext` (a double tap), and `fit` [x,y,w,h] (a tap moves the camera
// to that rect instead of raising an action). A hit with no `action` only hovers, labels and shows its tip.
// `focus` {rect, key, zoom?} moves the camera whenever `key` changes: it fits the rect, or with `zoom` centres the rect at that
// zoom. `fitKey` resets the camera to fit `bounds` whenever it changes. `sizeAction` raises {value: "<width in points>"} (rounded
// to 10, when it moved 20 or more from `sizeKnown`). `escapeAction`, and `enterAction` with `enterContext`, answer those keys.
// `wheel` is "zoom" (default) or "pan". `height`, or `minHeight` with `reserve` (the pane's height less `reserve`), sizes the stage.
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
    var rpx: [Double]?, hpx: [Double]?, wpx: [Double]?, sizepx: [Double]?
    var maxw: Double?, minx: Double?
}

struct CanvasOp {
    var kind: String
    var style: CanvasStyle
    var nums: [[Double]] = []
    var paths: [(path: Path, bounds: CGRect)] = []
    var texts: [(Double, Double, String)] = []
}

struct CanvasTipRun {
    var text: String, tone: String?, mono: Bool, bold: Bool
}

struct CanvasHit {
    var id: String, action: String, label: String
    var context: [String: AnyCodable]
    var rect: CGRect?, circle: (Double, Double, Double)?
    var slop = 0.0, minZoom = 0.0, maxZoom = Double.infinity
    var rpx: [Double]?
    var tip: [[CanvasTipRun]] = []
    var doubleAction = "", doubleContext: [String: AnyCodable] = [:]
    var fit: CGRect?
}

struct CanvasScene {
    var ops: [CanvasOp] = []
    var hits: [CanvasHit] = []
    var bounds = CGRect(x: 0, y: 0, width: 100, height: 100)
    var focus: CGRect?
    var focusKey = "", focusZoom: Double?
    var fitKey = ""
    var controls = true
    var emptyAction = "", emptyContext: [String: AnyCodable] = [:]
    var sizeAction = "", sizeKnown = 0.0
    var escapeAction = "", enterAction = "", enterContext: [String: AnyCodable] = [:]
    var wheelPans = false

    init(_ p: [String: AnyCodable]) {
        func nums(_ v: AnyCodable?) -> [Double] { (v?.arrayValue ?? []).compactMap { $0.numberValue } }
        func range(_ v: AnyCodable?) -> [Double]? { let n = nums(v); return n.count == 2 ? n : nil }
        func rectOf(_ v: AnyCodable?) -> CGRect? { let n = nums(v); return n.count == 4 ? CGRect(x: n[0], y: n[1], width: n[2], height: n[3]) : nil }
        ops = (p["ops"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.map { d in
            var st = CanvasStyle()
            st.fill = d["fill"]?.stringValue; st.stroke = d["stroke"]?.stringValue
            st.width = d["width"]?.numberValue ?? 1; st.opacity = d["opacity"]?.numberValue ?? 1
            st.radius = d["radius"]?.numberValue ?? 0; st.size = d["size"]?.numberValue ?? 12
            st.dash = nums(d["dash"]); st.px = d["px"]?.boolValue ?? false; st.mono = d["mono"]?.boolValue ?? false
            st.weight = d["weight"]?.stringValue ?? "regular"; st.anchor = d["anchor"]?.stringValue ?? "start"
            st.minZoom = d["minZoom"]?.numberValue ?? 0; st.maxZoom = d["maxZoom"]?.numberValue ?? .infinity
            st.rpx = range(d["rpx"]); st.hpx = range(d["hpx"]); st.wpx = range(d["wpx"]); st.sizepx = range(d["sizepx"])
            st.maxw = d["maxw"]?.numberValue; st.minx = d["minx"]?.numberValue
            var op = CanvasOp(kind: d["op"]?.stringValue ?? "", style: st)
            switch op.kind {
            case "rect": op.nums = (d["r"]?.arrayValue ?? []).map { nums($0) }
            case "circle": op.nums = (d["c"]?.arrayValue ?? []).map { nums($0) }
            case "path":
                op.paths = (d["d"]?.arrayValue ?? []).compactMap { $0.stringValue }.map { let path = canvasPath($0); return (path, path.boundingRect) }
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
            h.rpx = range(d["rpx"])
            h.doubleAction = d["doubleAction"]?.stringValue ?? ""; h.doubleContext = d["doubleContext"]?.dictionaryValue ?? [:]
            h.fit = rectOf(d["fit"])
            h.tip = (d["tip"]?.arrayValue ?? []).map { line in
                if let s = line.stringValue { return [CanvasTipRun(text: s, tone: nil, mono: false, bold: false)] }
                return (line.arrayValue ?? []).compactMap { $0.dictionaryValue }.map {
                    CanvasTipRun(text: $0["t"]?.stringValue ?? "", tone: $0["tone"]?.stringValue, mono: $0["mono"]?.boolValue ?? false, bold: $0["bold"]?.boolValue ?? false)
                }
            }
            return h
        }
        if let b = rectOf(p["bounds"]) { bounds = b }
        if let f = p["focus"]?.dictionaryValue {
            focus = rectOf(f["rect"]) ?? CGRect(x: f["x"]?.numberValue ?? 0, y: f["y"]?.numberValue ?? 0,
                                               width: f["w"]?.numberValue ?? 1, height: f["h"]?.numberValue ?? 1)
            focusKey = f["key"]?.stringValue ?? ""
            focusZoom = f["zoom"]?.numberValue
        }
        fitKey = p["fitKey"]?.stringValue ?? ""
        controls = p["controls"]?.boolValue ?? true
        emptyAction = p["emptyAction"]?.stringValue ?? ""; emptyContext = p["emptyContext"]?.dictionaryValue ?? [:]
        sizeAction = p["sizeAction"]?.stringValue ?? ""; sizeKnown = p["sizeKnown"]?.numberValue ?? 0
        escapeAction = p["escapeAction"]?.stringValue ?? ""
        enterAction = p["enterAction"]?.stringValue ?? ""; enterContext = p["enterContext"]?.dictionaryValue ?? [:]
        wheelPans = p["wheel"]?.stringValue == "pan"
    }
}

/// The camera at `zoom` with `rect` centred in `size`, never showing more empty space than `bounds` has: a scene smaller than
/// the view starts at its own edge, a larger one stops at its edges.
func canvasFocusCamera(_ rect: CGRect, zoom z: Double, bounds: CGRect, in size: CGSize) -> CanvasCamera {
    func axis(_ mid: Double, _ view: Double, _ lo: Double, _ hi: Double) -> Double {
        if (hi - lo) * z <= view { return -lo * z }
        return min(-lo * z, max(view - hi * z, view / 2 - mid * z))
    }
    return CanvasCamera(x: axis(rect.midX, size.width, bounds.minX, bounds.maxX), y: axis(rect.midY, size.height, bounds.minY, bounds.maxY), s: z, auto: false)
}

/// Cut labels by (text, limit, size, weight): a pan changes no label's limit, so a frame re-measures nothing it has cut before.
final class CanvasCutCache {
    private var store: [String: String] = [:]
    func cut(_ text: String, limit: Double, size: Double, weight: String, mono: Bool, _ make: (Double) -> String) -> String {
        let key = "\(text)|\(Int(limit.rounded()))|\(size)|\(weight)|\(mono)"
        if let hit = store[key] { return hit }
        if store.count > 20_000 { store.removeAll() }
        let out = make(limit)
        store[key] = out
        return out
    }
}

/// A screen size clamped to [min, max] points, or itself with no range.
func canvasClamp(_ v: Double, _ range: [Double]?) -> Double {
    guard let range, range.count == 2 else { return v }
    return max(range[0], min(range[1], v))
}

/// The longest prefix of `text` that, with a trailing "…", `measure` says fits `limit`; the text itself when it fits whole.
func canvasCut(_ text: String, limit: Double, measure: (String) -> Double) -> String {
    if measure(text) <= limit { return text }
    var lo = 0, hi = text.count
    while lo < hi {
        let mid = (lo + hi + 1) / 2
        if measure(String(text.prefix(mid)).trimmingCharacters(in: .whitespaces) + "…") <= limit { lo = mid } else { hi = mid - 1 }
    }
    return lo == 0 ? "…" : String(text.prefix(lo)).trimmingCharacters(in: .whitespaces) + "…"
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
    case "accentFill": return t.blue.opacity(0.15)
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
    @Environment(\.a2uiViewport) private var viewport
    var body: some View {
        let p = node.instance.properties
        let height = p["height"]?.numberValue ?? max(p["minHeight"]?.numberValue ?? 480, viewport - (p["reserve"]?.numberValue ?? 0))
        CanvasStage(scene: CanvasScene(p), height: height) { name, context in
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
    @State private var stageFrame = CGRect.zero
    @State private var lastFocus = ""
    @State private var lastFit = ""
    @State private var lastMagnify = 1.0
    @State private var reported: Double?
    @State private var lastTap: CanvasHit?
    @State private var cuts = CanvasCutCache()

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

    private func pan(_ dx: Double, _ dy: Double) {
        var c = effective(size)
        c.x += dx; c.y += dy; c.auto = false
        cam = c
    }

    private func radius(_ r: Double, _ range: [Double]?, _ s: Double) -> Double { canvasClamp(r * s, range) }

    private func hit(at p: CGPoint, _ c: CanvasCamera) -> CanvasHit? {
        for h in scene.hits.reversed() where c.s >= h.minZoom && c.s <= h.maxZoom {
            if let r = h.rect {
                let sr = CGRect(x: r.minX * c.s + c.x, y: r.minY * c.s + c.y, width: r.width * c.s, height: r.height * c.s).insetBy(dx: -h.slop, dy: -h.slop)
                if sr.contains(p) { return h }
            } else if let ci = h.circle {
                let dx = p.x - (ci.0 * c.s + c.x), dy = p.y - (ci.1 * c.s + c.y), r = radius(ci.2, h.rpx, c.s) + h.slop
                if dx * dx + dy * dy <= r * r { return h }
            }
        }
        return nil
    }

    private func draw(_ ctx: inout GraphicsContext, _ c: CanvasCamera, _ view: CGSize) {
        let t = WebTheme.shared
        let m = 24 / c.s
        let seen = CGRect(x: -c.x / c.s, y: -c.y / c.s, width: view.width / c.s, height: view.height / c.s).insetBy(dx: -m, dy: -m)
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
                    guard CGRect(x: r[0], y: r[1], width: r[2], height: r[3]).insetBy(dx: -m, dy: -m).intersects(seen) else { continue }
                    let w = st.wpx == nil ? r[2] * c.s : canvasClamp(r[2] * c.s, st.wpx), h = st.hpx == nil ? r[3] * c.s : canvasClamp(r[3] * c.s, st.hpx)
                    let rect = CGRect(x: r[0] * c.s + c.x, y: (r[1] + r[3] / 2) * c.s + c.y - h / 2, width: w, height: h)
                    paint(Path(roundedRect: rect, cornerRadius: st.radius * c.s))
                }
            case "circle":
                for r in op.nums where r.count == 3 {
                    let rr = radius(r[2], st.rpx, c.s)
                    guard CGRect(x: r[0] - rr / c.s, y: r[1] - rr / c.s, width: 2 * rr / c.s, height: 2 * rr / c.s).intersects(seen) else { continue }
                    paint(Path(ellipseIn: CGRect(x: r[0] * c.s + c.x - rr, y: r[1] * c.s + c.y - rr, width: rr * 2, height: rr * 2)))
                }
            case "path":
                let tf = CGAffineTransform(a: c.s, b: 0, c: 0, d: c.s, tx: c.x, ty: c.y)
                for p in op.paths where p.bounds.insetBy(dx: -m, dy: -m).intersects(seen) { paint(p.path.applying(tf)) }
            case "text":
                let size = st.sizepx != nil ? canvasClamp(st.size * c.s, st.sizepx) : st.px ? st.size : st.size * c.s
                let weight: Font.Weight = st.weight == "bold" ? .bold : st.weight == "semibold" ? .semibold : st.weight == "medium" ? .medium : .regular
                let color = canvasColor(st.fill) ?? t.label
                let anchor: UnitPoint = st.anchor == "middle" ? .center : st.anchor == "end" ? .trailing : .leading
                let reach = (st.maxw ?? 400) + 8
                for (x, y, s) in op.texts {
                    guard CGRect(x: st.minx == nil ? x - reach : seen.minX, y: y - 20 / c.s, width: st.minx == nil ? reach * 2 : seen.width, height: 40 / c.s).intersects(seen) else { continue }
                    func text(_ s: String) -> Text { Text(s).font(.system(size: size, weight: weight, design: st.mono ? .monospaced : .default)).foregroundColor(color.opacity(st.opacity)) }
                    var shown = s
                    if let maxw = st.maxw {
                        shown = cuts.cut(s, limit: maxw * c.s, size: size, weight: st.weight, mono: st.mono) { limit in canvasCut(s, limit: limit) { ctx.resolve(text($0)).measure(in: CGSize(width: 10_000, height: 100)).width } }
                    }
                    ctx.draw(text(shown), at: CGPoint(x: max(x * c.s + c.x, st.minx ?? -.infinity), y: y * c.s + c.y), anchor: anchor)
                }
            default: break
            }
        }
        if let p = hover, let h = hit(at: p, c) {
            var path = Path()
            if let r = h.rect {
                path = Path(roundedRect: CGRect(x: r.minX * c.s + c.x, y: r.minY * c.s + c.y, width: r.width * c.s, height: r.height * c.s).insetBy(dx: -h.slop, dy: -h.slop), cornerRadius: 3)
            } else if let ci = h.circle {
                let rr = radius(ci.2, h.rpx, c.s) + h.slop
                path = Path(ellipseIn: CGRect(x: ci.0 * c.s + c.x - rr, y: ci.1 * c.s + c.y - rr, width: rr * 2, height: rr * 2))
            }
            ctx.stroke(path, with: .color(t.blue), lineWidth: 1.5)
        }
    }

    /// The labelled hits in view, for VoiceOver: a Canvas is one element, so each hit with a label is offered as a button.
    private func labelled(_ c: CanvasCamera) -> [CanvasHit] {
        var out: [CanvasHit] = []
        for h in scene.hits where !h.label.isEmpty && c.s >= h.minZoom && c.s <= h.maxZoom {
            guard let r = h.rect else { continue }
            let sr = CGRect(x: r.minX * c.s + c.x, y: r.minY * c.s + c.y, width: r.width * c.s, height: r.height * c.s)
            if sr.intersects(CGRect(origin: .zero, size: size)) { out.append(h) }
            if out.count >= 80 { break }
        }
        return out
    }

    private func activate(_ h: CanvasHit, in size: CGSize) {
        if let f = h.fit { cam = fitted(f, in: size); return }
        guard !h.action.isEmpty else { return }
        var ctxt = h.context
        ctxt["id"] = .string(h.id)
        fire(h.action, ctxt)
    }

    var body: some View {
        let t = WebTheme.shared
        GeometryReader { geo in
            let c = effective(geo.size)
            ZStack(alignment: .topLeading) {
                Canvas { ctx, sz in draw(&ctx, c, sz) }
                    .frame(width: geo.size.width, height: geo.size.height)
                    .contentShape(Rectangle())
                    .gesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { v in
                                // macOS sends no hover events while a button is held, so a drag that pans the camera leaves the
                                // hover point where the press began and the outline stays on whatever passes under that old point.
                                hover = v.location
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
                                    activate(h, in: geo.size); lastTap = h
                                } else if !scene.emptyAction.isEmpty {
                                    fire(scene.emptyAction, scene.emptyContext)
                                }
                            }
                    )
                    .simultaneousGesture(SpatialTapGesture(count: 2).onEnded { v in
                        if let h = lastTap, !h.doubleAction.isEmpty {
                            var ctxt = h.doubleContext
                            ctxt["id"] = .string(h.id)
                            fire(h.doubleAction, ctxt)
                        }
                    })
                    .simultaneousGesture(
                        MagnifyGesture()
                            .onChanged { v in
                                let f = v.magnification / lastMagnify
                                lastMagnify = v.magnification
                                hover = v.startLocation
                                zoom(f, about: v.startLocation)
                            }
                            .onEnded { _ in lastMagnify = 1 }
                    )
                    #if os(macOS)
                    .onContinuousHover { phase in
                        switch phase {
                        case let .active(p): hover = p
                        case .ended: hover = nil
                        }
                    }
                    #endif
                    .accessibilityElement(children: .contain)
                    .accessibilityLabel("Canvas")
                    .accessibilityChildren {
                        ForEach(Array(labelled(c).enumerated()), id: \.offset) { _, h in
                            Rectangle().accessibilityLabel(h.label).accessibilityAddTraits(.isButton)
                                .accessibilityAction { activate(h, in: geo.size) }
                        }
                    }
                if let p = hover, dragFrom == nil, let h = hit(at: p, c), !h.tip.isEmpty { tooltip(h, p, geo.size) }
                if scene.controls {
                    HStack(spacing: 4) {
                        control("Fit") { cam = CanvasCamera() }
                        control("+") { zoom(1.4, about: CGPoint(x: geo.size.width / 2, y: geo.size.height / 2)) }
                        control("−") { zoom(1 / 1.4, about: CGPoint(x: geo.size.width / 2, y: geo.size.height / 2)) }
                    }
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .topTrailing)
                }
            }
            .background(t.fill.opacity(0.5))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(t.separatorStrong, lineWidth: 1))
            .background(GeometryReader { g in Color.clear.onAppear { stageFrame = g.frame(in: .global) }.onChange(of: g.frame(in: .global)) { _, f in stageFrame = f } })
            #if os(macOS)
            .background(CanvasScrollCatcher(frame: stageFrame) { dx, dy, p in
                if scene.wheelPans { pan(dx, dy) } else { zoom(exp(dy * 0.0016), about: p) }
                // A scroll moves the camera under a still pointer and macOS sends no hover event for it, so the outline and tip
                // would stay on the hit under the pointer's old content. The scroll event carries the pointer's place: use it.
                hover = p
            })
            #endif
            .onAppear { size = geo.size; reported = scene.sizeKnown; applyFocus(geo.size); report(geo.size.width) }
            .onChange(of: geo.size) { _, new in size = new; report(new.width) }
            .onChange(of: scene.focusKey) { _, _ in applyFocus(geo.size) }
            .onChange(of: scene.fitKey) { _, _ in applyFocus(geo.size) }
        }
        .frame(height: height)
        .focusable()
        .focusEffectDisabled()
        .onKeyPress { press in keyed(press) }
    }

    private func report(_ width: CGFloat) {
        guard !scene.sizeAction.isEmpty else { return }
        let q = (width / 10).rounded() * 10
        if abs(q - (reported ?? scene.sizeKnown)) >= 20 {
            reported = q
            fire(scene.sizeAction, ["value": .string(String(Int(q)))])
        }
    }

    private func applyFocus(_ size: CGSize) {
        if scene.fitKey != lastFit { lastFit = scene.fitKey; cam = CanvasCamera() }
        guard let f = scene.focus, scene.focusKey != lastFocus else { return }
        lastFocus = scene.focusKey
        if let z = scene.focusZoom {
            cam = canvasFocusCamera(f, zoom: z, bounds: scene.bounds, in: size)
        } else {
            cam = fitted(f, in: size)
        }
    }

    private func keyed(_ press: KeyPress) -> KeyPress.Result {
        let mid = CGPoint(x: size.width / 2, y: size.height / 2)
        switch press.key {
        case .escape: if scene.escapeAction.isEmpty { return .ignored }; fire(scene.escapeAction, [:]); return .handled
        case .return: if scene.enterAction.isEmpty { return .ignored }; fire(scene.enterAction, scene.enterContext); return .handled
        case .leftArrow: pan(70, 0); return .handled
        case .rightArrow: pan(-70, 0); return .handled
        case .upArrow: pan(0, 70); return .handled
        case .downArrow: pan(0, -70); return .handled
        default:
            switch press.characters {
            case "+", "=": zoom(1.3, about: mid); return .handled
            case "-": zoom(1 / 1.3, about: mid); return .handled
            case "0": cam = CanvasCamera(); return .handled
            default: return .ignored
            }
        }
    }

    private func tooltip(_ h: CanvasHit, _ p: CGPoint, _ size: CGSize) -> some View {
        let t = WebTheme.shared
        let tip = VStack(alignment: .leading, spacing: 2) {
            ForEach(Array(h.tip.enumerated()), id: \.offset) { _, line in
                line.reduce(Text("")) { acc, r in
                    acc + Text(r.text).font(.system(size: WebTheme.callout, weight: r.bold ? .semibold : .regular, design: r.mono ? .monospaced : .default))
                        .foregroundColor(canvasColor(r.tone) ?? t.label)
                }
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(.horizontal, 9).padding(.vertical, 5)
        .frame(maxWidth: 320, alignment: .leading)
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(t.separatorStrong, lineWidth: 1))
        .shadow(radius: 6, y: 3)
        .fixedSize()
        .allowsHitTesting(false)
        return tip.offset(x: max(4, min(p.x + 14, size.width - 330)), y: p.y + 16 > size.height - 70 ? max(4, p.y - 70) : p.y + 16)
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

#if os(macOS)
/// Catches the wheel and trackpad scroll over the canvas's stage: `(dx, dy, point in the stage)`.
private struct CanvasScrollCatcher: NSViewRepresentable {
    let frame: CGRect
    let scroll: (Double, Double, CGPoint) -> Void

    func makeNSView(context: Context) -> NSView {
        context.coordinator.install()
        return NSView()
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        context.coordinator.frame = frame
        context.coordinator.scroll = scroll
        context.coordinator.view = nsView
    }

    static func dismantleNSView(_ nsView: NSView, coordinator: Coordinator) { coordinator.remove() }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator {
        var frame = CGRect.zero
        var scroll: (Double, Double, CGPoint) -> Void = { _, _, _ in }
        weak var view: NSView?
        private var monitor: Any?

        func install() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] e in
                guard let self, let v = self.view, let win = v.window, e.window === win else { return e }
                // The point in the stage's own coordinates, converted by the view that fills the stage: the window's content
                // coordinates differ from the window's event coordinates by the title bar, so a point computed from the window is
                // that far from the pointer.
                let l = v.convert(e.locationInWindow, from: nil)
                let p = CGPoint(x: l.x, y: v.isFlipped ? l.y : v.bounds.height - l.y)
                guard v.bounds.contains(l) else { return e }
                let k = e.hasPreciseScrollingDeltas ? 1.0 : 8.0
                let boost = e.modifierFlags.contains(.control) ? 6.0 : 1.0
                self.scroll(e.scrollingDeltaX * k, e.scrollingDeltaY * k * boost, p)
                return nil
            }
        }

        func remove() { if let m = monitor { NSEvent.removeMonitor(m) }; monitor = nil }
        deinit { remove() }
    }
}
#endif
