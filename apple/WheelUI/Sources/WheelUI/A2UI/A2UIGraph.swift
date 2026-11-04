import A2UISwiftCore
import A2UISwiftUI
import SwiftUI
#if os(macOS)
import AppKit
#endif

// MARK: Graph — the Beads page's Focus and Graph views.
// The server lays everything out and sends the geometry; this view draws it and keeps the camera.
// Focus: `width`, `height`, `lanes` [{x,y,w,h}], `labels` [{x,y,text,bold}], `edges` [{t,p[8],s}],
// `nodes` [{kind,x,y,w,h,id,title,st,word,opacity,ghost,more{key,count}}], `pop`, `center`.
// Graph: `W`,`H`, `lanes` [{name,y0,h,w,counts,n}], parallel arrays `ids titles x y st lane rank dim`,
// edges `ea`/`eb` (node indexes), `sel` (node index or -1), `marks` (node indexes).
// Raises set_depth, focus_back, graph_cross, graph_select, focus_on, open_bead, graph_more,
// set_width and escape.

private let stGlyph = ["●", "◐", "⊘", "◆", "◌", "○", "✓"]
private let stWord = ["ready", "in progress", "blocked", "needs you", "deferred", "open", "done"]

#if os(macOS)
private let tertiaryColor = Color(nsColor: .tertiaryLabelColor)
private let stageColor = Color(nsColor: .controlBackgroundColor)
private let strongLine = Color(nsColor: .separatorColor)
#else
private let tertiaryColor = Color(uiColor: .tertiaryLabel)
private let stageColor = Color(uiColor: .secondarySystemBackground)
private let strongLine = Color(uiColor: .separator)
#endif

private func stColor(_ c: Int) -> Color {
    switch c {
    case 0: .green
    case 1: .blue
    case 2: .orange
    case 3: .purple
    case 4: tertiaryColor
    default: .secondary
    }
}

private func nums(_ v: AnyCodable?) -> [Double] { (v?.arrayValue ?? []).compactMap { $0.numberValue } }
private func ints(_ v: AnyCodable?) -> [Int] { nums(v).map { Int($0) } }
private func strs(_ v: AnyCodable?) -> [String] { (v?.arrayValue ?? []).compactMap { $0.stringValue } }

private struct GraphCamera {
    var x = 0.0, y = 0.0, s = 1.0
    var auto = true
}

private struct WholeLane {
    var name: String
    var y0: Double, h: Double, w: Double
    var counts: [Int]
    var n: Int
}

private struct WholeModel {
    var W = 0.0, H = 0.0
    var lanes: [WholeLane] = []
    var ids: [String] = [], titles: [String] = []
    var x: [Double] = [], y: [Double] = []
    var st: [Int] = [], lane: [Int] = [], rank: [Int] = []
    var dim: [Bool] = []
    var ea: [Int] = [], eb: [Int] = []
    var sel = -1
    var marked: [Bool] = []
    var filtered = false

    init() {}
    init(_ p: [String: AnyCodable]) {
        W = p["W"]?.numberValue ?? 0; H = p["H"]?.numberValue ?? 0
        lanes = (p["lanes"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.map {
            WholeLane(name: $0["name"]?.stringValue ?? "", y0: $0["y0"]?.numberValue ?? 0, h: $0["h"]?.numberValue ?? 0,
                      w: $0["w"]?.numberValue ?? 0, counts: ints($0["counts"]), n: Int($0["n"]?.numberValue ?? 0))
        }
        ids = strs(p["ids"]); titles = strs(p["titles"])
        x = nums(p["x"]); y = nums(p["y"])
        st = ints(p["st"]); lane = ints(p["lane"]); rank = ints(p["rank"])
        dim = ints(p["dim"]).map { $0 != 0 }
        ea = ints(p["ea"]); eb = ints(p["eb"])
        sel = Int(p["sel"]?.numberValue ?? -1)
        marked = Array(repeating: false, count: ids.count)
        for m in ints(p["marks"]) where m >= 0 && m < marked.count { marked[m] = true }
        filtered = p["filtered"]?.boolValue ?? false
    }
}

private final class GraphCache {
    var rev = ""
    var whole = WholeModel()
    var size = CGSize.zero
    var handled = ""
}

private let CELL_W = 150.0, CELL_H = 30.0, LANE_HEADER = 30.0

private enum GraphMode { case lanes, dots, nodes }
private func modeOf(_ s: Double) -> GraphMode { s < 0.05 ? .lanes : s < 0.75 ? .dots : .nodes }

private enum GraphHit {
    case lane(Int)
    case bead(Int)
}

struct A2UIGraphView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @Environment(\.a2uiViewport) private var viewport
    @State private var cam = GraphCamera()
    @State private var cache = GraphCache()
    @State private var hoverPoint: CGPoint?
    @State private var dragFrom: GraphCamera?
    @State private var dragMoved = false
    @State private var lastMagnify = 1.0
    @State private var scroll = ScrollPosition()
    @State private var stageFrame = CGRect.zero

    private func fire(_ name: String, _ context: [String: AnyCodable] = [:]) {
        surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: context)
    }

    private var props: [String: AnyCodable] { node.instance.properties }
    private var isFocus: Bool { props["mode"]?.stringValue != "graph" }

    private func whole() -> WholeModel {
        let rev = props["rev"]?.stringValue ?? ""
        if cache.rev != rev || cache.whole.ids.isEmpty && !isFocus {
            cache.rev = rev
            cache.whole = isFocus ? WholeModel() : WholeModel(props)
        }
        return cache.whole
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            bar
            Text(captionText)
                .font(.callout)
                .foregroundStyle(props["captionBad"]?.boolValue == true ? Color.red : Color.secondary)
                .frame(maxWidth: .infinity, minHeight: 17, alignment: .leading)
                .padding(.horizontal, 2).padding(.bottom, 8)
            stage
            legend
        }
        .focusable()
        .focusEffectDisabled()
        .onKeyPress { press in keyed(press) }
    }

    // MARK: the bar: Show in graph, Back, hops, zoom

    private var bar: some View {
        HStack(spacing: 8) {
            Button(props["cross"]?.stringValue ?? "") { fire("graph_cross") }
                .disabled(props["crossOff"]?.boolValue == true)
                .frame(minWidth: 132)
            ZStack(alignment: .leading) {
                HStack(spacing: 8) {
                    Button("‹ Back") { fire("focus_back") }
                        .help("Back to the previous centre (Esc)")
                        .disabled(props["canBack"]?.boolValue != true)
                    Picker("", selection: Binding<Int>(
                        get: { Int(props["depth"]?.numberValue ?? 2) },
                        set: { fire("set_depth", ["value": .string(String($0))]) })) {
                        Text("1 hop").tag(1)
                        Text("2 hops").tag(2)
                        Text("3 hops").tag(3)
                    }
                    .pickerStyle(.segmented).labelsHidden().fixedSize()
                }
                .opacity(isFocus ? 1 : 0).allowsHitTesting(isFocus)
                HStack(spacing: 8) {
                    Button("+") { zoomAt(1.4, CGPoint(x: cache.size.width / 2, y: cache.size.height / 2)) }
                    Button("−") { zoomAt(1 / 1.4, CGPoint(x: cache.size.width / 2, y: cache.size.height / 2)) }
                    Button("Fit") { fit() }
                }
                .opacity(isFocus ? 0 : 1).allowsHitTesting(!isFocus)
            }
            Spacer(minLength: 0)
        }
        .buttonStyle(.bordered).controlSize(.small)
        .padding(.top, 10).padding(.bottom, 4)
    }

    private var captionText: String {
        var t = props["caption"]?.stringValue ?? ""
        if !isFocus, props["selected"]?.boolValue != true {
            if cache.size.width > 0, modeOf(effective(cache.size).s) == .lanes { t += " Zoomed out: a lane is one bar; zoom in for dots, then beads." }
        }
        return t
    }

    // MARK: the stage

    private var stage: some View {
        GeometryReader { geo in
            ZStack(alignment: .topLeading) {
                if let empty = props["empty"]?.stringValue, !empty.isEmpty {
                    Text(empty).foregroundStyle(.secondary).padding(12)
                } else if isFocus {
                    focusStage(geo.size)
                } else {
                    graphStage(geo.size)
                }
            }
            .frame(width: geo.size.width, height: geo.size.height, alignment: .topLeading)
            .onAppear { cache.size = geo.size; reportWidth(geo.size.width) }
            .onChange(of: geo.size) { _, new in cache.size = new; reportWidth(new.width) }
        }
        .frame(height: max(380, viewport - 400))
        .background(stageColor)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(strongLine, lineWidth: 1))
        .background(GeometryReader { g in Color.clear.onAppear { stageFrame = g.frame(in: .global) }.onChange(of: g.frame(in: .global)) { _, f in stageFrame = f } })
        #if os(macOS)
        .background(GraphScrollCatcher(frame: stageFrame) { f, p in zoomAt(f, p) })
        #endif
    }

    private func reportWidth(_ w: CGFloat) {
        guard isFocus, props["empty"]?.stringValue == nil else { return }
        let reported = props["cw"]?.numberValue ?? 0
        let q = (w / 10).rounded() * 10
        if abs(q - reported) >= 20 { fire("set_width", ["value": .string(String(Int(q)))]) }
    }

    // MARK: Focus

    private struct FNode: Identifiable {
        let id: Int
        let kind: String
        let x: Double, y: Double, w: Double, h: Double
        let beadId: String, title: String
        let st: Int
        let word: String
        let opacity: Double
        let ghost: String?
        let moreKey: String?
        let moreCount: Int
    }

    private func focusNodes() -> [FNode] {
        (props["nodes"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.enumerated().map { i, d in
            FNode(id: i, kind: d["kind"]?.stringValue ?? "", x: d["x"]?.numberValue ?? 0, y: d["y"]?.numberValue ?? 0,
                  w: d["w"]?.numberValue ?? 0, h: d["h"]?.numberValue ?? 28, beadId: d["id"]?.stringValue ?? "",
                  title: d["title"]?.stringValue ?? "", st: Int(d["st"]?.numberValue ?? 5), word: d["word"]?.stringValue ?? "",
                  opacity: d["opacity"]?.numberValue ?? 1, ghost: d["ghost"]?.stringValue,
                  moreKey: d["more"]?.dictionaryValue?["key"]?.stringValue,
                  moreCount: Int(d["more"]?.dictionaryValue?["count"]?.numberValue ?? 0))
        }
    }

    @ViewBuilder
    private func focusStage(_ size: CGSize) -> some View {
        let W = props["width"]?.numberValue ?? size.width
        let H = props["height"]?.numberValue ?? size.height
        let nodes = focusNodes()
        let center = props["center"]?.dictionaryValue
        ScrollView([.horizontal, .vertical]) {
            ZStack(alignment: .topLeading) {
                Canvas { ctx, _ in drawFocusBack(&ctx) }
                    .frame(width: W, height: H)
                ForEach(nodes) { n in focusNode(n) }
                if let pop = props["pop"]?.dictionaryValue { popover(pop) }
            }
            .frame(width: W, height: H, alignment: .topLeading)
        }
        .scrollPosition($scroll)
        .onAppear { centreScroll(center, size) }
        .onChange(of: center?["id"]?.stringValue) { _, _ in centreScroll(center, size) }
    }

    private func centreScroll(_ c: [String: AnyCodable]?, _ size: CGSize) {
        guard let c else { return }
        let x = (c["x"]?.numberValue ?? 0) + (c["w"]?.numberValue ?? 0) / 2 - size.width / 2
        let y = (c["y"]?.numberValue ?? 0) + (c["h"]?.numberValue ?? 0) / 2 - size.height / 2
        scroll.scrollTo(point: CGPoint(x: max(0, x), y: max(0, y)))
    }

    private func drawFocusBack(_ ctx: inout GraphicsContext) {
        for l in (props["lanes"]?.arrayValue ?? []).compactMap({ $0.dictionaryValue }) {
            let r = CGRect(x: l["x"]?.numberValue ?? 0, y: l["y"]?.numberValue ?? 0, width: l["w"]?.numberValue ?? 0, height: l["h"]?.numberValue ?? 0)
            ctx.fill(Path(roundedRect: r, cornerRadius: 8), with: .color(Color.primary.opacity(0.06)))
        }
        for l in (props["labels"]?.arrayValue ?? []).compactMap({ $0.dictionaryValue }) {
            let bold = l["bold"]?.boolValue == true
            let raw = l["text"]?.stringValue ?? ""
            let t = Text(bold ? raw.uppercased() : raw).font(.system(size: 11, weight: bold ? .semibold : .regular)).tracking(bold ? 0.5 : 0)
                .foregroundStyle(Color.secondary)
            ctx.draw(ctx.resolve(t), at: CGPoint(x: l["x"]?.numberValue ?? 0, y: l["y"]?.numberValue ?? 0), anchor: .leading)
        }
        for e in (props["edges"]?.arrayValue ?? []).compactMap({ $0.dictionaryValue }) {
            let p = nums(e["p"])
            guard p.count == 8 else { continue }
            let t = e["t"]?.stringValue ?? "dep"
            var path = Path()
            path.move(to: CGPoint(x: p[0], y: p[1]))
            path.addCurve(to: CGPoint(x: p[6], y: p[7]), control1: CGPoint(x: p[2], y: p[3]), control2: CGPoint(x: p[4], y: p[5]))
            if t == "par" {
                ctx.stroke(path, with: .color(Color.gray), style: StrokeStyle(lineWidth: 1.3, lineCap: .round, dash: [1.5, 4]))
                continue
            }
            let colour: Color = t == "cyc" ? .red : Color.secondary
            ctx.stroke(path, with: .color(colour.opacity(t == "cyc" ? 1 : 0.75)), lineWidth: 1.3)
            let s = e["s"]?.numberValue ?? 1
            var ah = Path()
            ah.move(to: CGPoint(x: p[6], y: p[7]))
            ah.addLine(to: CGPoint(x: p[6] - 7 * s, y: p[7] - 3.5))
            ah.addLine(to: CGPoint(x: p[6] - 7 * s, y: p[7] + 3.5))
            ah.closeSubpath()
            ctx.fill(ah, with: .color(colour))
        }
    }

    @ViewBuilder
    private func focusNode(_ n: FNode) -> some View {
        let isCenter = n.kind == "center"
        let shape = RoundedRectangle(cornerRadius: 7)
        Group {
            if let g = n.ghost {
                Text("missing: \(g)").font(.system(size: 12)).foregroundStyle(tertiaryColor).lineLimit(1).truncationMode(.tail)
                    .padding(.horizontal, 10)
                    .frame(width: n.w, height: n.h, alignment: .leading)
                    .overlay(shape.strokeBorder(strongLine, style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
                    .help("\(g) is named as a blocker but is not in this repo")
            } else if let key = n.moreKey {
                Button { fire("graph_more", ["key": .string(key)]) } label: {
                    Text("+ \(n.moreCount) more").font(.system(size: 12, weight: .semibold)).foregroundStyle(Color.accentColor)
                        .padding(.horizontal, 10)
                        .frame(width: n.w, height: n.h, alignment: .leading)
                        .background(shape.fill(stageColor))
                        .overlay(shape.strokeBorder(strongLine, lineWidth: 1))
                        .contentShape(shape)
                }
                .buttonStyle(.plain)
                .help("\(n.moreCount) more beads")
            } else {
                HStack(spacing: 6) {
                    Text(stGlyph[min(max(n.st, 0), 6)]).font(.system(size: 12)).foregroundStyle(stColor(n.st))
                    Text(n.title).font(.system(size: 12)).lineLimit(1).truncationMode(.tail)
                }
                .padding(.horizontal, 9)
                .frame(width: n.w, height: n.h, alignment: .leading)
                .background(shape.fill(isCenter ? Color.accentColor.opacity(0.15) : stageColor))
                .overlay(shape.strokeBorder(isCenter ? Color.accentColor : strongLine, lineWidth: isCenter ? 1.5 : 1))
                .contentShape(shape)
                .opacity(n.opacity)
                .onTapGesture { if !isCenter { fire("focus_on", ["id": .string(n.beadId)]) } }
                .simultaneousGesture(TapGesture(count: 2).onEnded { fire("open_bead", ["id": .string(n.beadId)]) })
                .help("\(n.beadId) · \(n.word) · \(n.title)")
                .accessibilityLabel("\(n.beadId), \(n.word), \(n.title)")
                .accessibilityAddTraits(.isButton)
            }
        }
        .position(x: n.x + n.w / 2, y: n.y + n.h / 2)
    }

    private func popover(_ pop: [String: AnyCodable]) -> some View {
        let items = (pop["items"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        return VStack(alignment: .leading, spacing: 0) {
            Text("\(items.count) more").font(.system(size: 11, weight: .semibold)).textCase(.uppercase).foregroundStyle(.secondary)
                .padding(.horizontal, 12).padding(.vertical, 6)
            Divider()
            List(Array(items.enumerated()), id: \.offset) { _, it in
                Button { fire("focus_on", ["id": .string(it["id"]?.stringValue ?? "")]) } label: {
                    HStack(spacing: 6) {
                        Text(stGlyph[min(max(Int(it["st"]?.numberValue ?? 5), 0), 6)]).foregroundStyle(stColor(Int(it["st"]?.numberValue ?? 5)))
                        Text(it["id"]?.stringValue ?? "").font(.system(.callout, design: .monospaced)).foregroundStyle(.secondary)
                        Text(it["title"]?.stringValue ?? "").lineLimit(1)
                    }
                }
                .buttonStyle(.plain)
            }
            .listStyle(.plain)
        }
        .frame(width: 300, height: min(260, CGFloat(items.count) * 28 + 40))
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(strongLine, lineWidth: 1))
        .shadow(radius: 8, y: 4)
        .position(x: (pop["x"]?.numberValue ?? 0) + 150, y: (pop["y"]?.numberValue ?? 0) + min(260, CGFloat(items.count) * 28 + 40) / 2)
    }

    // MARK: Graph

    private func fitted(_ size: CGSize, _ box: CGRect? = nil) -> GraphCamera {
        let m = whole()
        let b = box ?? CGRect(x: 0, y: 0, width: max(m.W, 1), height: max(m.H, 1))
        let s = max(0.002, min(4, min((size.width - 24) / b.width, (size.height - 24) / b.height)))
        var c = GraphCamera()
        c.s = s
        c.x = (size.width - b.width * s) / 2 - b.minX * s
        c.y = min(12, (size.height - b.height * s) / 2) - b.minY * s
        c.auto = box == nil
        return c
    }

    /// The camera as drawn: the fit while no one has panned or zoomed, so a resize refits.
    private func effective(_ size: CGSize) -> GraphCamera { cam.auto ? fitted(size) : cam }

    private func fit() { cam = fitted(cache.size) ; cam.auto = true }

    private func zoomAt(_ f: Double, _ p: CGPoint) {
        var c = effective(cache.size)
        let s = max(0.002, min(4, c.s * f)), k = s / c.s
        c.x = p.x - (p.x - c.x) * k; c.y = p.y - (p.y - c.y) * k; c.s = s; c.auto = false
        cam = c
    }

    private func centreOn(_ i: Int, _ size: CGSize) {
        let m = whole()
        guard i >= 0, i < m.ids.count, size.width > 0 else { return }
        var c = effective(size)
        c.auto = false
        if c.s < 0.9 { c.s = 0.9 }
        c.x = size.width / 2 - (m.x[i] + CELL_W / 2) * c.s
        c.y = size.height / 2 - (m.y[i] + CELL_H / 2) * c.s
        cam = c
    }

    private func hit(_ p: CGPoint, _ c: GraphCamera) -> GraphHit? {
        let m = whole()
        let s = c.s, mode = modeOf(s)
        if mode == .lanes {
            for (li, l) in m.lanes.enumerated() {
                let sy = l.y0 * s + c.y, sh = max(l.h * s, 3)
                if p.y >= sy && p.y <= sy + sh && p.x >= c.x - 6 * s && p.x <= c.x + l.w * s + 6 * s + 10 { return .lane(li) }
            }
            return nil
        }
        let nw = (CELL_W - 14) * s, nh = (CELL_H - 5) * s
        let r = mode == .dots ? max(1.4, min(5.5, CELL_H * s * 0.26)) : 0
        var best: Int?, bd = Double.infinity
        for i in 0..<m.ids.count {
            let x = m.x[i] * s + c.x, y = m.y[i] * s + c.y
            if mode == .nodes {
                if p.x >= x && p.x <= x + nw && p.y >= y && p.y <= y + nh { return .bead(i) }
            } else {
                let dx = p.x - x - r, dy = p.y - y - r, d = dx * dx + dy * dy, lim = (r + 4) * (r + 4)
                if d <= lim && d < bd { bd = d; best = i }
            }
        }
        return best.map { .bead($0) }
    }

    private func graphStage(_ size: CGSize) -> some View {
        let m = whole()
        let c = effective(size)
        let hoverHit = hoverPoint.flatMap { hit($0, c) }
        return ZStack(alignment: .topLeading) {
            Canvas { ctx, sz in drawWhole(&ctx, sz, m, c, hoverHit) }
                .frame(width: size.width, height: size.height)
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
                            switch hit(v.location, start) {
                            case .lane(let li):
                                let l = m.lanes[li]
                                cam = fitted(size, CGRect(x: -6, y: l.y0, width: max(l.w, CELL_W * 3) + 12, height: l.h))
                            case .bead(let i): fire("graph_select", ["id": .string(m.ids[i])])
                            case nil: fire("graph_select", ["id": .string("")])
                            }
                        }
                )
                .simultaneousGesture(SpatialTapGesture(count: 2).onEnded { v in
                    if case .bead(let i) = hit(v.location, c) { fire("open_bead", ["id": .string(m.ids[i])]) }
                })
                .gesture(MagnifyGesture()
                    .onChanged { v in
                        let f = v.magnification / lastMagnify
                        lastMagnify = v.magnification
                        zoomAt(f, v.startLocation)
                    }
                    .onEnded { _ in lastMagnify = 1 })
                .onContinuousHover { phase in
                    switch phase {
                    case .active(let p): hoverPoint = p
                    case .ended: hoverPoint = nil
                    }
                }
            if let h = hoverHit, let p = hoverPoint, dragFrom == nil { tooltip(h, p, size, m) }
        }
        .onAppear {
            if props["centre"]?.boolValue == true, m.sel >= 0, cache.handled != cache.rev { cache.handled = cache.rev; centreOn(m.sel, size) }
        }
    }

    private func tooltip(_ h: GraphHit, _ p: CGPoint, _ size: CGSize, _ m: WholeModel) -> some View {
        let tip = VStack(alignment: .leading, spacing: 2) {
            switch h {
            case .lane(let li):
                let l = m.lanes[li], c = l.counts
                Text(l.name).font(.callout.weight(.semibold))
                Text(laneSummary(l, c)).font(.callout)
                Text("click to zoom to this lane").font(.callout).foregroundStyle(.secondary)
            case .bead(let i):
                HStack(spacing: 6) {
                    Text(m.ids[i]).font(.system(.callout, design: .monospaced)).foregroundStyle(.secondary)
                    Text(stWord[min(max(m.st[i], 0), 6)]).font(.callout.weight(.semibold)).foregroundStyle(stColor(m.st[i]))
                }
                Text(m.titles[i]).font(.callout)
                Text("\(m.lane[i] < m.lanes.count ? m.lanes[m.lane[i]].name : "") · layer \(m.rank[i])").font(.callout).foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 9).padding(.vertical, 5)
        .frame(maxWidth: 320, alignment: .leading)
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(strongLine, lineWidth: 1))
        .shadow(radius: 6, y: 3)
        .fixedSize()
        .allowsHitTesting(false)
        return tip.offset(x: max(4, min(p.x + 14, size.width - 330)), y: p.y + 16 > size.height - 70 ? max(4, p.y - 70) : p.y + 16)
    }

    private func laneSummary(_ l: WholeLane, _ c: [Int]) -> String {
        var parts = ["\(l.n) beads"]
        let named: [(Int, String)] = [(0, "ready"), (1, "in progress"), (2, "blocked"), (3, "need you"), (6, "done")]
        for (code, word) in named where c.count > code && c[code] > 0 { parts.append("\(c[code]) \(word)") }
        return parts.joined(separator: " · ")
    }

    private func shorten(_ s: String, _ px: Double, _ per: Double = 5.6) -> String {
        let n = max(1, Int(px / per))
        return s.count <= n ? s : String(s.prefix(max(0, n - 1))).trimmingCharacters(in: .whitespaces) + "…"
    }

    private func drawWhole(_ ctx: inout GraphicsContext, _ size: CGSize, _ m: WholeModel, _ c: GraphCamera, _ hover: GraphHit?) {
        guard !m.lanes.isEmpty else {
            ctx.draw(ctx.resolve(Text("No beads yet").foregroundStyle(Color.secondary)), at: CGPoint(x: 12, y: 16), anchor: .leading)
            return
        }
        let s = c.s, mode = modeOf(s), tx = c.x, ty = c.y
        let fill = Color.primary.opacity(0.06)
        let hasMarks = m.sel >= 0
        func dim(_ i: Int) -> Bool { (hasMarks && !m.marked[i]) || m.dim[i] }
        func label(_ t: String, _ colour: Color, _ x: Double, _ y: Double) {
            ctx.draw(ctx.resolve(Text(t).font(.system(size: 11)).foregroundStyle(colour)), at: CGPoint(x: x, y: y), anchor: .leading)
        }
        for l in m.lanes {
            let sy = l.y0 * s + ty, sh = l.h * s
            if sy > size.height || sy + sh < 0 { continue }
            let sx = tx - 6 * s, sw = l.w * s + 12 * s
            if sx > size.width || sx + sw < 0 { continue }
            if mode == .lanes {
                let bh = min(26, max(1.5, sh * 0.85)), by = sy + max(0, (sh - bh) / 2), bw = max(20, sw), thin = bh < 10
                if !thin { ctx.fill(Path(roundedRect: CGRect(x: sx, y: by, width: bw, height: bh), cornerRadius: 5), with: .color(fill)) }
                let tot = Double(max(l.n, 1))
                var acc = sx
                for code in [6, 3, 1, 2, 0, 5, 4] {
                    let n = l.counts.count > code ? l.counts[code] : 0
                    if n == 0 { continue }
                    let w = bw * Double(n) / tot
                    let r = thin ? CGRect(x: acc, y: by, width: w, height: bh) : CGRect(x: acc, y: by + bh - 4, width: w, height: 4)
                    ctx.fill(Path(r), with: .color(stColor(code).opacity(code == 6 ? 0.35 : 0.85)))
                    acc += w
                }
                if bh < 12 { continue }
                let x = max(sx + 8, 8)
                let name = shorten(l.name, 220)
                label(name, .primary, x, by + bh / 2 - 2)
                let cn = l.counts
                let sum = [cn[0] > 0 ? "\(cn[0]) ready" : nil, cn[2] > 0 ? "\(cn[2]) blocked" : nil, cn[6] > 0 ? "\(cn[6]) done" : nil].compactMap { $0 }.joined(separator: " · ")
                let tw = Double(name.count) * 5.6
                if bw > tw + 120 { label(sum, .secondary, x + tw + 10, by + bh / 2 - 2) }
            } else {
                ctx.fill(Path(roundedRect: CGRect(x: sx, y: sy, width: sw, height: sh), cornerRadius: min(8, 8 * s + 2)), with: .color(fill))
                if LANE_HEADER * s >= 9 {
                    label(shorten(l.name + " · \(l.n)", max(60, size.width - 40)), .secondary, max(sx + 8, 8), sy + LANE_HEADER * s / 2)
                }
            }
        }
        guard mode != .lanes else { return }
        let nw = (CELL_W - 14) * s, nh = (CELL_H - 5) * s
        let r = mode == .dots ? max(1.4, min(5.5, CELL_H * s * 0.26)) : 0
        var vis: [Int] = []
        for i in 0..<m.ids.count {
            let sx = m.x[i] * s + tx, sy = m.y[i] * s + ty
            if sx + nw < 0 || sx > size.width || sy + nh < 0 || sy > size.height { continue }
            vis.append(i)
        }
        func px(_ i: Int, _ right: Bool) -> Double { m.x[i] * s + tx + (mode == .nodes ? (right ? nw : 0) : r) }
        func py(_ i: Int) -> Double { m.y[i] * s + ty + (mode == .nodes ? nh / 2 : r) }
        func visible(_ k: Int) -> Bool {
            let a = m.ea[k], b = m.eb[k]
            let ax = m.x[a] * s + tx, bx = m.x[b] * s + tx, ay = m.y[a] * s + ty, by = m.y[b] * s + ty
            return !((ax < -nw && bx < -nw) || (ax > size.width && bx > size.width) || (ay < -nh && by < -nh) || (ay > size.height && by > size.height))
        }
        var plain: [Int] = [], hi: [Int] = []
        for k in 0..<m.ea.count {
            if hasMarks { if m.marked[m.ea[k]] && m.marked[m.eb[k]], visible(k) { hi.append(k) }; continue }
            if m.filtered && (m.dim[m.ea[k]] || m.dim[m.eb[k]]) { continue }
            if visible(k) { plain.append(k) }
        }
        func stroke(_ list: [Int], _ colour: Color, _ width: Double, _ alpha: Double) {
            var path = Path()
            for k in list {
                let a = m.ea[k], b = m.eb[k]
                let x1 = px(a, true), y1 = py(a), x2 = px(b, false), y2 = py(b), dx = max(10, abs(x2 - x1) * 0.5)
                path.move(to: CGPoint(x: x1, y: y1))
                path.addCurve(to: CGPoint(x: x2, y: y2), control1: CGPoint(x: x1 + dx, y: y1), control2: CGPoint(x: x2 - dx, y: y2))
            }
            ctx.stroke(path, with: .color(colour.opacity(alpha)), lineWidth: width)
        }
        if plain.count <= 3500 { stroke(plain, .secondary, 1, mode == .dots ? 0.3 : 0.55) }
        if !hi.isEmpty { stroke(hi, .accentColor, mode == .nodes ? 1.6 : 1.3, 1) }
        if mode == .nodes && hi.count < 400 {
            var ah = Path()
            for k in hi {
                let x = px(m.eb[k], false), y = py(m.eb[k])
                ah.move(to: CGPoint(x: x, y: y)); ah.addLine(to: CGPoint(x: x - 6, y: y - 3.5)); ah.addLine(to: CGPoint(x: x - 6, y: y + 3.5)); ah.closeSubpath()
            }
            ctx.fill(ah, with: .color(.accentColor))
        }
        if mode == .dots {
            for pass in 0..<2 {
                for code in 0..<7 {
                    var path = Path()
                    var any = false
                    for i in vis where m.st[i] == code && dim(i) == (pass == 1) {
                        let x = m.x[i] * s + tx + r, y = m.y[i] * s + ty + r
                        path.addEllipse(in: CGRect(x: x - r, y: y - r, width: 2 * r, height: 2 * r)); any = true
                    }
                    if any { ctx.fill(path, with: .color(stColor(code).opacity(pass == 1 ? 0.16 : (code == 6 ? 0.5 : 1)))) }
                }
            }
            if m.sel >= 0 {
                let i = m.sel, x = m.x[i] * s + tx + r, y = m.y[i] * s + ty + r
                ctx.stroke(Path(ellipseIn: CGRect(x: x - r - 3, y: y - r - 3, width: 2 * (r + 3), height: 2 * (r + 3))), with: .color(.accentColor), lineWidth: 2)
            }
        } else {
            var hoverBead: Int?
            if case .bead(let h)? = hover { hoverBead = h }
            for i in vis {
                let x = m.x[i] * s + tx, y = m.y[i] * s + ty
                var cc = ctx
                cc.opacity = dim(i) ? 0.2 : 1
                let selected = i == m.sel
                let rr = Path(roundedRect: CGRect(x: x, y: y, width: nw, height: nh), cornerRadius: min(6, nh / 3))
                cc.fill(rr, with: .color(selected ? Color.accentColor.opacity(0.15) : stageColor))
                cc.stroke(rr, with: .color(selected || hoverBead == i ? Color.accentColor : strongLine), lineWidth: selected ? 2 : 1)
                cc.draw(cc.resolve(Text(stGlyph[min(max(m.st[i], 0), 6)]).font(.system(size: 11)).foregroundStyle(stColor(m.st[i]))), at: CGPoint(x: x + 6, y: y + nh / 2 + 0.5), anchor: .leading)
                cc.draw(cc.resolve(Text(shorten(m.titles[i], nw - 26)).font(.system(size: 11)).foregroundStyle(Color.primary)), at: CGPoint(x: x + 20, y: y + nh / 2 + 0.5), anchor: .leading)
            }
        }
    }

    // MARK: the legend

    private var legend: some View {
        HStack(alignment: .firstTextBaseline, spacing: 16) {
            ForEach(0..<7, id: \.self) { c in
                HStack(spacing: 4) {
                    Text(stGlyph[c]).foregroundStyle(stColor(c))
                    Text(stWord[c]).fontWeight(.semibold).foregroundStyle(stColor(c))
                }
            }
            Spacer(minLength: 8)
            Text(isFocus
                 ? "solid arrow: blocks · dotted line: parent and child" + (props["cyc"]?.boolValue == true ? " · red: dependency cycle" : "")
                 : "one dot per bead · drag to pan, wheel or pinch to zoom")
                .foregroundStyle(.secondary)
        }
        .font(.callout)
        .lineLimit(1)
        .padding(.horizontal, 2).padding(.top, 10)
    }

    // MARK: keys

    private func keyed(_ press: KeyPress) -> KeyPress.Result {
        if press.key == .escape { fire("escape"); return .handled }
        guard !isFocus else { return .ignored }
        let size = cache.size
        let step = 70.0
        var c = effective(size)
        c.auto = false
        switch press.key {
        case .leftArrow: c.x += step
        case .rightArrow: c.x -= step
        case .upArrow: c.y += step
        case .downArrow: c.y -= step
        case .return:
            let m = whole()
            if m.sel >= 0 { fire("open_bead", ["id": .string(m.ids[m.sel])]) }
            return .handled
        default:
            switch press.characters {
            case "+", "=": zoomAt(1.3, CGPoint(x: size.width / 2, y: size.height / 2)); return .handled
            case "-": zoomAt(1 / 1.3, CGPoint(x: size.width / 2, y: size.height / 2)); return .handled
            case "0": fit(); return .handled
            default: return .ignored
            }
        }
        cam = c
        return .handled
    }
}

#if os(macOS)
/// Catches the wheel and trackpad scroll over the graph's stage and turns them into zoom, as the
/// page's wheel does.
private struct GraphScrollCatcher: NSViewRepresentable {
    let frame: CGRect
    let zoom: (Double, CGPoint) -> Void

    func makeNSView(context: Context) -> NSView {
        let v = NSView()
        context.coordinator.install()
        return v
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        context.coordinator.frame = frame
        context.coordinator.zoom = zoom
        context.coordinator.view = nsView
    }

    static func dismantleNSView(_ nsView: NSView, coordinator: Coordinator) { coordinator.remove() }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator {
        var frame = CGRect.zero
        var zoom: (Double, CGPoint) -> Void = { _, _ in }
        weak var view: NSView?
        private var monitor: Any?

        func install() {
            monitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] e in
                guard let self, let win = self.view?.window, e.window === win, let content = win.contentView else { return e }
                let p = CGPoint(x: e.locationInWindow.x, y: content.bounds.height - e.locationInWindow.y)
                guard self.frame.contains(p) else { return e }
                let dy = e.scrollingDeltaY * (e.hasPreciseScrollingDeltas ? 1 : 8)
                self.zoom(exp(dy * 0.0016 * (e.modifierFlags.contains(.control) ? 6 : 1)), CGPoint(x: p.x - self.frame.minX, y: p.y - self.frame.minY))
                return nil
            }
        }

        func remove() { if let m = monitor { NSEvent.removeMonitor(m) }; monitor = nil }
        deinit { remove() }
    }
}
#endif
