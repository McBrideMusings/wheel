import A2UISwiftCore
import A2UISwiftUI
import SwiftUI

/// The names Wheel adds to the basic catalog.
let wheelA2UIExtensions: Set<String> = ["ProgressBar", "Disclosure", "Label", "Box", "SearchField", "Segmented", "Select", "Check", "Sheet", "BeadTable", "Graph", "Stat", "Pill", "Unsupported"]

// The palette is the system's own semantic colours, so a page follows light and dark and the
// person's accent colour like any native window.
struct WheelPalette {
    let fg, muted, bg, card, soft, line, ok, warn, bad, todo, focus: Color
    init(dark _: Bool) {
        #if os(macOS)
        bg = Color(nsColor: .windowBackgroundColor); card = Color(nsColor: .controlBackgroundColor)
        line = Color(nsColor: .separatorColor)
        #else
        bg = Color(uiColor: .systemBackground); card = Color(uiColor: .secondarySystemBackground)
        line = Color(uiColor: .separator)
        #endif
        fg = .primary; muted = .secondary; soft = Color.secondary.opacity(0.12); todo = Color.secondary.opacity(0.25)
        ok = .green; warn = .orange; bad = .red; focus = .accentColor
    }
    func tone(_ n: String?) -> Color? {
        switch n {
        case "fg": return fg; case "muted": return muted; case "ok": return ok; case "warn": return warn
        case "bad": return bad; case "line": return line; case "card": return card; case "soft": return soft
        case "bg": return bg; case "todo": return todo; case "focus": return focus; default: return nil
        }
    }
}

func weightOf(_ s: String?) -> Font.Weight {
    switch s { case "bold": return .bold; case "semibold": return .semibold; case "medium": return .medium; default: return .regular }
}
func insets(_ v: AnyCodable?) -> EdgeInsets {
    guard let a = v?.arrayValue?.compactMap({ $0.numberValue }) else { return EdgeInsets() }
    if a.count == 2 { return EdgeInsets(top: a[0], leading: a[1], bottom: a[0], trailing: a[1]) }
    if a.count == 4 { return EdgeInsets(top: a[0], leading: a[3], bottom: a[2], trailing: a[1]) }
    return EdgeInsets()
}

// MARK: Label — one styled-text component: tone colour, size, weight, mono, chip border, status dot, inline runs
struct A2UILabelView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @Environment(\.colorScheme) private var cs
    func fmt(_ p: [String: AnyCodable], _ pal: WheelPalette, base: [String: AnyCodable]) -> (Font, Color) {
        let size = p["size"]?.numberValue ?? base["size"]?.numberValue ?? 13
        let mono = p["mono"]?.boolValue ?? base["mono"]?.boolValue ?? false
        let w = weightOf(p["weight"]?.stringValue ?? base["weight"]?.stringValue)
        let c = pal.tone(p["tone"]?.stringValue ?? base["tone"]?.stringValue) ?? pal.fg
        return (.system(size: size, weight: w, design: mono ? .monospaced : .default), c)
    }
    var body: some View {
        let pal = WheelPalette(dark: cs == .dark)
        let p = node.instance.properties
        let dc = DataContext(surface: surface, path: node.dataContextPath)
        let tracking = p["tracking"]?.numberValue ?? 0
        let content: Text = {
            if let runs = p["runs"]?.arrayValue {
                var t = Text("")
                for r in runs {
                    guard let d = r.dictionaryValue else { continue }
                    let (f, c) = fmt(d, pal, base: p)
                    t = t + Text(d["text"]?.stringValue ?? "").font(f).foregroundColor(c)
                }
                return t
            }
            let s = (try? node.typedProperties(TextProperties.self)).map { dc.resolve($0.text) } ?? ""
            let (f, c) = fmt(p, pal, base: [:])
            return Text(s).font(f).fontWeight(weightOf(p["weight"]?.stringValue)).foregroundColor(c).tracking(tracking)
        }()
        let tone = pal.tone(p["tone"]?.stringValue) ?? pal.fg
        let align: Alignment = p["align"]?.stringValue == "end" ? .trailing : .leading
        HStack(alignment: .firstTextBaseline, spacing: 5) {
            if let dot = p["dot"]?.stringValue {
                Group {
                    switch dot {
                    case "ring": Circle().strokeBorder(tone, lineWidth: 2)
                    case "hollow": Circle().strokeBorder(tone, lineWidth: 1)
                    default: Circle().fill(tone)
                    }
                }.frame(width: 8, height: 8)
            }
            content
                .multilineTextAlignment(p["align"]?.stringValue == "end" ? .trailing : .leading)
                .textCase(p["upper"]?.boolValue == true ? .uppercase : nil)
        }
        .padding(.horizontal, p["chip"]?.boolValue == true ? 4 : 0)
        .overlay {
            if p["chip"]?.boolValue == true { RoundedRectangle(cornerRadius: 4).stroke(pal.line, lineWidth: 1) }
        }
        .frame(maxWidth: p["align"]?.stringValue == "end" ? .infinity : nil, alignment: align)
        .fixedSize(horizontal: false, vertical: true)
    }
}

// MARK: Box — one generic container: axis, spacing, widths (number | "flex" | null), padding, border, fill, radius, edge, controlSize
struct A2UIBoxView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @Environment(\.colorScheme) private var cs
    var body: some View {
        let pal = WheelPalette(dark: cs == .dark)
        let p = node.instance.properties
        let horizontal = p["axis"]?.stringValue == "h"
        let spacing = p["spacing"]?.numberValue ?? 0
        let widths = p["widths"]?.arrayValue ?? []
        let trailing = Int(p["trailing"]?.numberValue ?? 0)
        let radius = p["radius"]?.numberValue ?? 0
        let alignName = p["align"]?.stringValue ?? "start"
        let kids = Array(node.children.enumerated())
        let shape = RoundedRectangle(cornerRadius: radius, style: .continuous)
        let fill: Color? = p["fill"]?.stringValue == "ready" ? pal.ok.opacity(0.07) : pal.tone(p["fill"]?.stringValue)
        let size: ControlSize = p["controlSize"]?.stringValue == "small" ? .small : .regular

        Group {
            if horizontal {
                let va: VerticalAlignment = alignName == "firstTextBaseline" ? .firstTextBaseline : alignName == "top" ? .top : .center
                HStack(alignment: va, spacing: spacing) {
                    ForEach(kids, id: \.offset) { i, k in
                        if trailing > 0 && i == kids.count - trailing { Spacer(minLength: 0) }
                        let w: AnyCodable? = i < widths.count ? widths[i] : nil
                        if let n = w?.numberValue {
                            A2UIChildView(node: k, surface: surface).frame(width: n, alignment: .leading)
                        } else if w?.stringValue == "flex" {
                            A2UIChildView(node: k, surface: surface).frame(maxWidth: .infinity, alignment: .leading)
                        } else {
                            A2UIChildView(node: k, surface: surface)
                        }
                    }
                }
            } else {
                let ha: HorizontalAlignment = alignName == "end" ? .trailing : alignName == "center" ? .center : .leading
                VStack(alignment: ha, spacing: spacing) {
                    ForEach(kids, id: \.offset) { _, k in
                        A2UIChildView(node: k, surface: surface)
                            .frame(maxWidth: alignName == "start" ? .infinity : nil, alignment: .leading)
                    }
                }
            }
        }
        .modifier(A2UIMonoIf(on: p["mono"]?.boolValue == true))
        .controlSize(size)
        .padding(insets(p["padding"]))
        .frame(maxWidth: horizontal ? nil : (alignName == "start" ? .infinity : nil), alignment: .leading)
        .background { if let fill { shape.fill(fill) } }
        .overlay(alignment: p["edge"]?.stringValue == "top" ? .top : .bottom) {
            if p["edge"] != nil {
                Path { q in q.move(to: .zero); q.addLine(to: CGPoint(x: 4000, y: 0)) }
                    .stroke(pal.line, style: StrokeStyle(lineWidth: 1, dash: p["dash"]?.boolValue == true ? [3, 3] : []))
                    .frame(height: 1)
            }
        }
        .overlay { if let b = pal.tone(p["border"]?.stringValue) { shape.strokeBorder(b, lineWidth: 1) } }
        .clipShape(shape)
        .padding(insets(p["margin"]))
    }
}
struct A2UIMonoIf: ViewModifier {
    let on: Bool
    func body(content: Content) -> some View {
        if on { content.font(.system(size: 13, design: .monospaced)) } else { content }
    }
}

// MARK: ProgressBar — three segments, coloured from the palette
struct A2UIProgressBarView: View {
    let node: ComponentNode
    @Environment(\.colorScheme) private var cs
    var body: some View {
        let pal = WheelPalette(dark: cs == .dark)
        let p = node.instance.properties
        let closed = p["closed"]?.numberValue ?? 0, wip = p["wip"]?.numberValue ?? 0, total = max(p["total"]?.numberValue ?? 1, 1)
        GeometryReader { g in
            HStack(spacing: 0) {
                Rectangle().fill(pal.ok).frame(width: g.size.width * closed / total)
                Rectangle().fill(pal.warn).frame(width: g.size.width * wip / total)
                Rectangle().fill(pal.todo)
            }.clipShape(RoundedRectangle(cornerRadius: 4))
        }.frame(height: 8).padding(.vertical, 4)
    }
}

// MARK: Disclosure — the system's DisclosureGroup: children[0] is the label, children[1] the content
struct A2UIDisclosureView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @State private var open: Bool
    init(node: ComponentNode, surface: SurfaceModel) {
        self.node = node; self.surface = surface
        _open = State(initialValue: node.instance.properties["expanded"]?.boolValue ?? false)
    }
    var body: some View {
        DisclosureGroup(isExpanded: $open) {
            if node.children.count > 1 { A2UIChildView(node: node.children[1], surface: surface) }
        } label: {
            if let h = node.children.first { A2UIChildView(node: h, surface: surface) }
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
    }
}

// MARK: SearchField — the system's rounded text field; it raises its action, text as `q`, as the person types
struct A2UISearchFieldView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @State private var text: String
    @State private var pending: Task<Void, Never>?
    init(node: ComponentNode, surface: SurfaceModel) {
        self.node = node; self.surface = surface
        _text = State(initialValue: node.instance.properties["value"]?.stringValue ?? "")
    }
    var body: some View {
        let p = node.instance.properties
        TextField(p["placeholder"]?.stringValue ?? "", text: $text)
            .textFieldStyle(.roundedBorder)
            .autocorrectionDisabled()
            .accessibilityIdentifier("a2ui.search")
            .onChange(of: text) { _, new in
                pending?.cancel()
                guard let name = p["action"]?.stringValue else { return }
                pending = Task { @MainActor in
                    try? await Task.sleep(nanoseconds: 200_000_000)
                    if Task.isCancelled { return }
                    surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: ["q": .string(new)])
                }
            }
    }
}

// MARK: Segmented — the system's segmented control: `options` [{value, text}], `selected`, `action` raised with the value as `value`
struct A2UISegmentedView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @State private var selection: String
    init(node: ComponentNode, surface: SurfaceModel) {
        self.node = node; self.surface = surface
        _selection = State(initialValue: node.instance.properties["selected"]?.stringValue ?? "")
    }
    var body: some View {
        let p = node.instance.properties
        let options = (p["options"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        Picker("", selection: $selection) {
            ForEach(Array(options.enumerated()), id: \.offset) { _, o in
                Text(o["text"]?.stringValue ?? "").tag(o["value"]?.stringValue ?? "")
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
        .accessibilityIdentifier("a2ui.segmented")
        .onChange(of: selection) { _, new in
            guard let name = p["action"]?.stringValue else { return }
            surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: ["value": .string(new)])
        }
    }
}

struct A2UIPillStyle: ViewModifier {
    let link: Bool
    func body(content: Content) -> some View {
        if link {
            #if os(macOS)
            content.buttonStyle(.link)
            #else
            content.buttonStyle(.borderless)
            #endif
        } else {
            content.buttonStyle(.bordered).controlSize(.small)
        }
    }
}

// MARK: Pill — the system's small bordered button raising its action; `src` is a small tag after the text
struct A2UIPillView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        Button {
            guard let name = p["name"]?.stringValue else { return }
            surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: p["context"]?.dictionaryValue ?? [:])
        } label: {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(p["text"]?.stringValue ?? "")
                if let src = p["src"]?.stringValue {
                    Text(src).font(.caption2).foregroundStyle(.secondary)
                }
            }
        }
        .modifier(A2UIPillStyle(link: p["style"]?.stringValue == "link"))
        .disabled(p["disabled"]?.boolValue == true)
        .accessibilityIdentifier("a2ui.pill.\(node.instance.id)")
    }
}

// MARK: Select — the system's pop-up menu: `options` [{value, text}], `selected`; raises its action with `key` and `value`
struct A2UISelectView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        let options = (p["options"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let binding = Binding<String>(
            get: { p["selected"]?.stringValue ?? "" },
            set: { new in
                guard let name = p["action"]?.stringValue else { return }
                surface.dispatchAction(name: name, sourceComponentId: node.instance.id,
                                       context: ["key": .string(p["key"]?.stringValue ?? ""), "value": .string(new)])
            })
        Picker("", selection: binding) {
            ForEach(Array(options.enumerated()), id: \.offset) { _, o in
                Text(o["text"]?.stringValue ?? "").tag(o["value"]?.stringValue ?? "")
            }
        }
        .labelsHidden()
        .pickerStyle(.menu)
        .fixedSize()
        .disabled(p["disabled"]?.boolValue == true)
        .accessibilityIdentifier("a2ui.select.\(p["key"]?.stringValue ?? "")")
    }
}

// MARK: Check — the system's checkbox: `text`, `value`; raises its action with `value` "true" or "false"
struct A2UICheckView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        let binding = Binding<Bool>(
            get: { p["value"]?.boolValue ?? false },
            set: { new in
                guard let name = p["action"]?.stringValue else { return }
                surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: ["value": .string(new ? "true" : "false")])
            })
        Toggle(p["text"]?.stringValue ?? "", isOn: binding)
            #if os(macOS)
            .toggleStyle(.checkbox)
            #endif
            .accessibilityIdentifier("a2ui.check")
    }
}

// MARK: Sheet — the system's sheet over the page while `presented`; its one child is the content; closing raises `action`
struct A2UISheetView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        let binding = Binding<Bool>(
            get: { p["presented"]?.boolValue ?? false },
            set: { open in
                if !open, let name = p["action"]?.stringValue {
                    surface.dispatchAction(name: name, sourceComponentId: node.instance.id)
                }
            })
        Color.clear.frame(width: 0, height: 0)
            .sheet(isPresented: binding) {
                VStack(spacing: 0) {
                    HStack {
                        Spacer()
                        Button("Done") { binding.wrappedValue = false }.keyboardShortcut(.cancelAction)
                    }.padding(10)
                    Divider()
                    ScrollView {
                        if let c = node.children.first { A2UIChildView(node: c, surface: surface).padding(16) }
                    }
                }
                .frame(minWidth: 520, idealWidth: 600, minHeight: 520, idealHeight: 640)
            }
    }
}

// MARK: BeadTable — the system's List: sections of rows under a sortable header, selection drawn by the system
// column {key, label, width|null, tree, sortable}; row {id, path, depth, kids, collapsed, dim, sel, cells [{text, tone, dot, mono}],
// buttons [{label, script}]}. Raises sort_by {key}, select_bead {id}, toggle_path {path}, run_script {bead, script}.
private struct A2UIViewportKey: EnvironmentKey { static let defaultValue: CGFloat = 640 }
extension EnvironmentValues {
    var a2uiViewport: CGFloat {
        get { self[A2UIViewportKey.self] }
        set { self[A2UIViewportKey.self] = newValue }
    }
}

struct A2UIBeadTableView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @Environment(\.a2uiViewport) private var viewport

    private func fire(_ name: String, _ context: [String: AnyCodable]) {
        surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: context)
    }

    var body: some View {
        let p = node.instance.properties
        let cols = (p["columns"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let sort = p["sort"]?.dictionaryValue
        let sections = (p["sections"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let selected = Binding<String?>(
            get: {
                for sec in sections { for r in (sec["rows"]?.arrayValue ?? []).compactMap({ $0.dictionaryValue }) where r["sel"]?.boolValue == true { return r["path"]?.stringValue } }
                return nil
            },
            set: { path in
                guard let path else { return }
                for sec in sections { for r in (sec["rows"]?.arrayValue ?? []).compactMap({ $0.dictionaryValue }) where r["path"]?.stringValue == path {
                    fire("select_bead", ["id": .string(r["id"]?.stringValue ?? "")])
                } }
            })
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                ForEach(Array(cols.enumerated()), id: \.offset) { _, c in
                    let key = c["key"]?.stringValue ?? ""
                    let active = sort?["key"]?.stringValue == key
                    Button {
                        fire("sort_by", ["key": .string(key)])
                    } label: {
                        HStack(spacing: 3) {
                            Text(c["label"]?.stringValue ?? "")
                            if active {
                                Image(systemName: sort?["asc"]?.boolValue == true ? "chevron.up" : "chevron.down").font(.caption2.weight(.bold))
                            }
                        }
                        .font(.subheadline.weight(active ? .semibold : .regular))
                        .foregroundStyle(active ? .primary : .secondary)
                    }
                    .buttonStyle(.plain)
                    .disabled(c["sortable"]?.boolValue != true)
                    .modifier(A2UIColWidth(width: c["width"]?.numberValue))
                }
            }
            .padding(.horizontal, 20).padding(.vertical, 6)
            Divider()
            List(selection: selected) {
                ForEach(Array(sections.enumerated()), id: \.offset) { _, sec in
                    let rows = (sec["rows"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
                    let title = sec["title"]?.stringValue ?? ""
                    if title.isEmpty {
                        ForEach(Array(rows.enumerated()), id: \.offset) { _, r in rowView(r, cols).tag(r["path"]?.stringValue) }
                    } else {
                        Section {
                            ForEach(Array(rows.enumerated()), id: \.offset) { _, r in rowView(r, cols).tag(r["path"]?.stringValue) }
                        } header: {
                            Text("\(title)  \(Int(sec["count"]?.numberValue ?? 0))")
                        }
                    }
                }
            }
            #if os(macOS)
            .listStyle(.inset(alternatesRowBackgrounds: true))
            #else
            .listStyle(.plain)
            #endif
            .overlay {
                if sections.allSatisfy({ ($0["rows"]?.arrayValue ?? []).isEmpty }) {
                    Text(p["empty"]?.stringValue ?? "Nothing matches the search and filters.").foregroundStyle(.secondary)
                }
            }
        }
        .frame(height: max(320, viewport - 250))
    }

    @ViewBuilder
    private func rowView(_ row: [String: AnyCodable], _ cols: [[String: AnyCodable]]) -> some View {
        let cells = (row["cells"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let depth = row["depth"]?.numberValue ?? 0
        let buttons = (row["buttons"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let id = row["id"]?.stringValue ?? ""
        let dim = row["dim"]?.boolValue == true
        HStack(spacing: 8) {
            ForEach(Array(cols.enumerated()), id: \.offset) { i, c in
                let cell = i < cells.count ? cells[i] : [:]
                HStack(spacing: 4) {
                    if c["tree"]?.boolValue == true {
                        Color.clear.frame(width: CGFloat(depth) * 16, height: 1)
                        if row["kids"]?.boolValue == true {
                            Button {
                                fire("toggle_path", ["path": .string(row["path"]?.stringValue ?? "")])
                            } label: {
                                Image(systemName: "chevron.right").font(.caption2.weight(.semibold))
                                    .rotationEffect(.degrees(row["collapsed"]?.boolValue == true ? 0 : 90))
                                    .foregroundStyle(.secondary).frame(width: 12)
                            }.buttonStyle(.plain)
                        } else {
                            Color.clear.frame(width: 12, height: 1)
                        }
                    }
                    cellView(cell, dim: dim)
                }
                .modifier(A2UIColWidth(width: c["width"]?.numberValue))
            }
            if !buttons.isEmpty {
                HStack(spacing: 4) {
                    ForEach(Array(buttons.enumerated()), id: \.offset) { _, b in
                        Button(b["label"]?.stringValue ?? "") {
                            fire("run_script", ["bead": .string(id), "script": .string(b["script"]?.stringValue ?? "")])
                        }.buttonStyle(.bordered).controlSize(.mini)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func cellView(_ cell: [String: AnyCodable], dim: Bool) -> some View {
        let text = cell["text"]?.stringValue ?? ""
        let tone = nativeTone(cell["tone"]?.stringValue)
        HStack(spacing: 5) {
            if cell["dot"]?.boolValue == true { Circle().fill(tone ?? .secondary).frame(width: 7, height: 7) }
            Text(text)
                .font(cell["mono"]?.boolValue == true ? .system(.callout, design: .monospaced) : .body)
                .foregroundStyle(tone ?? (dim ? .secondary : .primary))
                .lineLimit(1).truncationMode(.tail)
        }
    }
}

/// The system colour a tone name stands for; nil for the default text colour.
func nativeTone(_ n: String?) -> Color? {
    switch n {
    case "ok": .green
    case "warn": .orange
    case "bad": .red
    case "muted": .secondary
    default: nil
    }
}

// MARK: Stat — a number over its caption in the system's GroupBox
struct A2UIStatView: View {
    let node: ComponentNode
    var body: some View {
        let p = node.instance.properties
        GroupBox {
            VStack(alignment: .leading, spacing: 2) {
                Text(p["value"]?.stringValue ?? "").font(.title.weight(.semibold).monospacedDigit())
                    .foregroundStyle(nativeTone(p["tone"]?.stringValue) ?? .primary)
                Text(p["label"]?.stringValue ?? "").font(.callout).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct A2UIColWidth: ViewModifier {
    let width: Double?
    func body(content: Content) -> some View {
        if let width { content.frame(width: width, alignment: .leading) }
        else { content.frame(maxWidth: .infinity, alignment: .leading) }
    }
}

/// Wheel's additions to the basic catalog, drawn natively: styled text, a box, a progress bar, a
/// disclosure and the placeholder for a component the catalog does not hold.
struct WheelA2UICatalog: CustomComponentCatalog {
    @ViewBuilder
    func build(typeName: String, node: ComponentNode, surface: SurfaceModel) -> some View {
        switch typeName {
        case "ProgressBar": A2UIProgressBarView(node: node)
        case "Disclosure": A2UIDisclosureView(node: node, surface: surface)
        case "Label": A2UILabelView(node: node, surface: surface)
        case "Box": A2UIBoxView(node: node, surface: surface)
        case "SearchField": A2UISearchFieldView(node: node, surface: surface)
        case "Segmented": A2UISegmentedView(node: node, surface: surface)
        case "Select": A2UISelectView(node: node, surface: surface)
        case "Check": A2UICheckView(node: node, surface: surface)
        case "Sheet": A2UISheetView(node: node, surface: surface)
        case "BeadTable": A2UIBeadTableView(node: node, surface: surface)
        case "Graph": A2UIGraphView(node: node, surface: surface)
        case "Stat": A2UIStatView(node: node)
        case "Pill": A2UIPillView(node: node, surface: surface)
        case "Unsupported": A2UIUnsupportedView(node: node)
        default: EmptyView()
        }
    }
}

/// What stands where a component outside the catalog was asked for, so the layout is not silently
/// shorter than the server meant it.
struct A2UIUnsupportedView: View {
    let node: ComponentNode
    var body: some View {
        let name = node.instance.properties["name"]?.stringValue ?? "?"
        Label("Unsupported component: \(name)", systemImage: "questionmark.square.dashed")
            .font(.system(size: 12))
            .foregroundStyle(.secondary)
            .padding(6)
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.secondary.opacity(0.5), style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
            .accessibilityIdentifier("a2ui.unsupported")
    }
}

// MARK: theming through A2UIStyle (set on the view model: A2UISurfaceView re-injects viewModel.a2uiStyle into the environment)
func themedStyle(_ pal: WheelPalette, leaf: CGFloat) -> A2UIStyle {
    var s = A2UIStyle(primaryColor: pal.fg)
    s.textStyles["h2"] = .init(font: .system(size: 20), weight: .bold)
    s.textStyles["h4"] = .init(font: .system(size: 15), weight: .bold)
    s.textStyles["h5"] = .init(font: .system(size: 14), weight: .semibold)
    s.textStyles["body"] = .init(font: .system(size: 13), color: pal.fg)
    s.textStyles["caption"] = .init(font: .system(size: 12), color: pal.muted)
    s.cardStyle = .init(padding: 10, cornerRadius: 10, shadowRadius: 0.5, shadowColor: pal.line, shadowY: 0, backgroundColor: pal.card)
    s.leafMargin = leaf
    return s
}

