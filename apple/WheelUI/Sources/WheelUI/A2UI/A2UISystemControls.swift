import WheelKit
import Charts
import MapKit
import SwiftUI

// SwiftUI's own controls under their SwiftUI names (ADR-0031, ADR-0032). A property is the SwiftUI initializer's argument of the
// same name; a component's children are the initializer's content closure. Nothing here restyles a control: a page styles one with
// `modifiers`. A control that holds a value (Stepper, ColorPicker) shows its node's `value` and writes through `ViewTree.change`,
// as the basic catalog's inputs do.

@MainActor @ViewBuilder
private func child(_ node: ViewNode, _ i: Int, _ tree: ViewTree) -> some View {
    let parts = node.parts
    if i < parts.count { A2UIChildView(node: parts[i], tree: tree) }
}

@MainActor @ViewBuilder
private func kids(_ node: ViewNode, _ tree: ViewTree, from: Int = 0) -> some View {
    ForEach(Array(node.parts.dropFirst(from))) { k in A2UIChildView(node: k, tree: tree) }
}

@MainActor
private func fire(_ node: ViewNode, _ tree: ViewTree, _ key: String = "action", _ context: [String: JSONValue] = [:]) {
    guard !node.action(key).isEmpty else { return }
    tree.raise(node, key, context)
}

// MARK: ProgressView(_ title:, value:, total:) — a spinner when `value` is absent
struct A2UIProgressViewView: View {
    let node: ViewNode
    var body: some View {
        let title = node.string("title") ?? ""
        if let v = node.number("value") {
            ProgressView(title, value: v, total: node.number("total") ?? 1)
        } else {
            ProgressView(title)
        }
    }
}

// MARK: Gauge(value:, in:) { label } — `value`, `min`, `max`, `title`, `style` (linearCapacity, accessoryLinear, accessoryCircular, accessoryCircularCapacity)
struct A2UIGaugeView: View {
    let node: ViewNode
    var body: some View {
        let g = Gauge(value: node.number("value") ?? 0, in: (node.number("min") ?? 0)...(node.number("max") ?? 1)) { Text(node.string("title") ?? "") }
        switch node.string("style") {
        case "accessoryLinear": g.gaugeStyle(.accessoryLinear)
        case "accessoryCircular": g.gaugeStyle(.accessoryCircular)
        case "accessoryCircularCapacity": g.gaugeStyle(.accessoryCircularCapacity)
        default: g.gaugeStyle(.linearCapacity)
        }
    }
}

// MARK: Menu(_ title:) { content } — the children are the menu's items (Button, Divider, Menu)
struct A2UIMenuView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        // `keepOpen` leaves the menu open after an item is chosen, for ticking several (SwiftUI's menuActionDismissBehavior).
        Menu(node.string("title") ?? "") { kids(node, tree).environment(\.a2uiInMenu, true) }
            #if os(iOS)
            .menuActionDismissBehavior(node.bool("keepOpen") == true ? .disabled : .automatic)
            #endif
    }
}

// MARK: Link(_ title:, destination:)
struct A2UILinkView: View {
    let node: ViewNode
    var body: some View {
        if let dest = node.string("destination").flatMap(URL.init(string:)) {
            Link(node.string("title") ?? dest.absoluteString, destination: dest)
        }
    }
}

// MARK: Form { content } — `style` (automatic, columns, grouped)
struct A2UIFormView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        let f = Form { kids(node, tree).environment(\.a2uiInForm, true) }
        switch node.string("style") {
        case "columns": f.formStyle(.columns)
        case "grouped": f.formStyle(.grouped)
        default: f
        }
    }
}

// MARK: Section(_ title:) { content } — `title` text, or Section { content } header: { } footer: { } with `header` and `footer`
// each a component (a List with `pinnedViews` keeps the header at its top while the rows scroll)
struct A2UISectionView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        let header = node.embedded["header"]
        let footer = node.embedded["footer"]
        if header != nil || footer != nil {
            Section { kids(node, tree) } header: {
                if let header { A2UIChildView(node: header, tree: tree) }
            } footer: {
                if let footer { A2UIChildView(node: footer, tree: tree) }
            }
        } else {
            Section(node.string("title") ?? "") { kids(node, tree) }
        }
    }
}

// MARK: LabeledContent(_ title:) { content } — `value` text, or children as the content
struct A2UILabeledContentView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        let title = node.string("title") ?? ""
        if let value = node.string("value") {
            LabeledContent(title, value: value)
        } else {
            LabeledContent(title) { kids(node, tree) }
        }
    }
}

// MARK: Grid(alignment:, horizontalSpacing:, verticalSpacing:) { GridRow }
struct A2UIGridView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        Grid(alignment: gridAlignment(node.string("alignment")), horizontalSpacing: node.number("horizontalSpacing").map { CGFloat($0) },
             verticalSpacing: node.number("verticalSpacing").map { CGFloat($0) }) { kids(node, tree) }
    }
}

private func gridAlignment(_ name: String?) -> Alignment {
    switch name {
    case "center": .center
    case "trailing": .trailing
    case "top": .top
    case "bottom": .bottom
    case "topLeading": .topLeading
    case "firstTextBaseline": .leadingFirstTextBaseline
    default: .leading
    }
}

// MARK: GridRow { cells }
struct A2UIGridRowView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View { GridRow { kids(node, tree) } }
}

// MARK: Stepper(_ title:, value:, in:, step:) — `value`, `min`, `max`, `step`
struct A2UIStepperView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        let binding = Binding<Double>(
            get: { node.value?.numberValue ?? 0 },
            set: { tree.change(node, .number($0)) })
        let step = node.number("step") ?? 1
        if let lo = node.number("min"), let hi = node.number("max") {
            Stepper(node.string("title") ?? "", value: binding, in: lo...hi, step: step)
        } else {
            Stepper(node.string("title") ?? "", value: binding, step: step)
        }
    }
}

// MARK: ColorPicker(_ title:, selection:, supportsOpacity:) — `value` "#RRGGBB"
struct A2UIColorPickerView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        let binding = Binding<Color>(
            get: { hexColor(node.value?.stringValue) },
            set: { tree.change(node, .string(hexString($0))) })
        ColorPicker(node.string("title") ?? "", selection: binding, supportsOpacity: node.bool("supportsOpacity") ?? true)
    }
}

private func hexColor(_ s: String?) -> Color {
    guard var h = s, !h.isEmpty else { return .accentColor }
    if h.hasPrefix("#") { h.removeFirst() }
    guard h.count == 6, let v = UInt32(h, radix: 16) else { return .accentColor }
    return Color(red: Double(v >> 16 & 255) / 255, green: Double(v >> 8 & 255) / 255, blue: Double(v & 255) / 255)
}

private func hexString(_ c: Color) -> String {
    let r = c.resolve(in: EnvironmentValues())
    func b(_ x: Float) -> Int { max(0, min(255, Int((x * 255).rounded()))) }
    return String(format: "#%02X%02X%02X", b(r.red), b(r.green), b(r.blue))
}

// MARK: Popover — children[0] is the trigger label of a button that opens a popover holding children[1]: `arrowEdge`
struct A2UIPopoverView: View {
    let node: ViewNode
    let tree: ViewTree
    @State private var open = false
    var body: some View {
        Button { open.toggle() } label: { child(node, 0, tree) }
            .popover(isPresented: $open, arrowEdge: edge(node.string("arrowEdge"))) {
                child(node, 1, tree).padding()
            }
    }
}

private func edge(_ name: String?) -> Edge {
    switch name {
    case "top": .top
    case "leading": .leading
    case "trailing": .trailing
    default: .bottom
    }
}

// MARK: Alert and ConfirmationDialog — `title`, `message`, `presented`, `buttons` [{title, role, action}]; closing raises `dismissAction`
private struct A2UIPresentedButtons: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        let buttons = (node.props["buttons"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        ForEach(Array(buttons.enumerated()), id: \.offset) { i, b in
            let role: ButtonRole? = b["role"]?.stringValue == "destructive" ? .destructive : b["role"]?.stringValue == "cancel" ? .cancel : nil
            Button(b["title"]?.stringValue ?? "OK", role: role) {
                if case .object = b["action"] { tree.raise(node, "buttons.\(i).action") }
            }
        }
    }
}

@MainActor
private func presentedBinding(_ node: ViewNode, _ tree: ViewTree) -> Binding<Bool> {
    Binding(get: { node.bool("presented") ?? false },
            set: { open in if !open { fire(node, tree, "dismissAction") } })
}

struct A2UIAlertView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        Color.clear.frame(width: 0, height: 0)
            .alert(node.string("title") ?? "", isPresented: presentedBinding(node, tree)) {
                A2UIPresentedButtons(node: node, tree: tree)
            } message: { Text(node.string("message") ?? "") }
    }
}

struct A2UIConfirmationDialogView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        Color.clear.frame(width: 0, height: 0)
            .confirmationDialog(node.string("title") ?? "", isPresented: presentedBinding(node, tree), titleVisibility: .visible) {
                A2UIPresentedButtons(node: node, tree: tree)
            } message: { Text(node.string("message") ?? "") }
    }
}

// MARK: ViewThatFits(in:) { content } — `in` (horizontal, vertical; both when absent): the first child whose ideal size fits the
// space it is offered on those axes, else the last
struct A2UIViewThatFitsView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        let axes: SwiftUI.Axis.Set = switch node.string("in") {
        case "horizontal": .horizontal
        case "vertical": .vertical
        default: [.horizontal, .vertical]
        }
        ViewThatFits(in: axes) { kids(node, tree) }
    }
}

// MARK: NavigationSplitView — children[0] sidebar, children[1] detail
struct A2UINavigationSplitViewView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        NavigationSplitView { child(node, 0, tree) } detail: { child(node, 1, tree) }
    }
}

// MARK: NavigationStack — children[0] is the root; `title` is its navigationTitle
struct A2UINavigationStackView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        NavigationStack { child(node, 0, tree).navigationTitle(node.string("title") ?? "") }
    }
}

// MARK: HSplitView and VSplitView — children are the panes (a stack on iOS, which has neither)
struct A2UIHSplitViewView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        #if os(macOS)
        HSplitView { kids(node, tree) }
        #else
        HStack { kids(node, tree) }
        #endif
    }
}

struct A2UIVSplitViewView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        #if os(macOS)
        VSplitView { kids(node, tree) }
        #else
        VStack { kids(node, tree) }
        #endif
    }
}

// MARK: Toolbar — SwiftUI's `.toolbar` modifier on children[0]; the other children are its items: `placement` (automatic, principal, primaryAction, navigation, cancellationAction, confirmationAction)
struct A2UIToolbarView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        child(node, 0, tree).toolbar {
            ToolbarItemGroup(placement: placement(node.string("placement"))) { kids(node, tree, from: 1) }
        }
    }
}

private func placement(_ name: String?) -> ToolbarItemPlacement {
    switch name {
    case "principal": .principal
    case "primaryAction": .primaryAction
    case "navigation": .navigation
    case "cancellationAction": .cancellationAction
    case "confirmationAction": .confirmationAction
    default: .automatic
    }
}

// MARK: Chart { marks } — `mark` (bar, line, point, area), `data` [{x, y, series}]; x is text
struct A2UIChartView: View {
    let node: ViewNode
    var body: some View {
        let rows = (node.props["data"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let kind = node.string("mark") ?? "bar"
        Chart(Array(rows.enumerated()), id: \.offset) { _, r in
            let x = PlottableValue.value("x", r["x"]?.stringValue ?? "")
            let y = PlottableValue.value("y", r["y"]?.numberValue ?? 0)
            let s = PlottableValue.value("series", r["series"]?.stringValue ?? "")
            switch kind {
            case "line": LineMark(x: x, y: y).foregroundStyle(by: s)
            case "point": PointMark(x: x, y: y).foregroundStyle(by: s)
            case "area": AreaMark(x: x, y: y).foregroundStyle(by: s)
            default: BarMark(x: x, y: y).foregroundStyle(by: s)
            }
        }
        .frame(minHeight: 120)
    }
}

// MARK: Map — `latitude`, `longitude`, `span` (degrees), `markers` [{title, latitude, longitude}]
struct A2UIMapView: View {
    let node: ViewNode
    var body: some View {
        let center = CLLocationCoordinate2D(latitude: node.number("latitude") ?? 0, longitude: node.number("longitude") ?? 0)
        let span = node.number("span") ?? 10
        let markers = (node.props["markers"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        Map(initialPosition: .region(MKCoordinateRegion(center: center, span: MKCoordinateSpan(latitudeDelta: span, longitudeDelta: span)))) {
            ForEach(Array(markers.enumerated()), id: \.offset) { _, m in
                Marker(m["title"]?.stringValue ?? "", coordinate: CLLocationCoordinate2D(latitude: m["latitude"]?.numberValue ?? 0, longitude: m["longitude"]?.numberValue ?? 0))
            }
        }
        .frame(minHeight: 160)
    }
}

// MARK: ShareLink(item:, subject:, message:) — `item` text, `title` its label
struct A2UIShareLinkView: View {
    let node: ViewNode
    var body: some View {
        let item = node.string("item") ?? ""
        let subject = node.string("subject").map { Text($0) }
        let message = node.string("message").map { Text($0) }
        if let title = node.string("title") {
            ShareLink(title, item: item, subject: subject, message: message)
        } else {
            ShareLink(item: item, subject: subject, message: message)
        }
    }
}

// MARK: PasteButton(payloadType: String.self) — raises `action` with the pasted text as `text`
struct A2UIPasteButtonView: View {
    let node: ViewNode
    let tree: ViewTree
    var body: some View {
        PasteButton(payloadType: String.self) { strings in
            if let s = strings.first { fire(node, tree, "action", ["text": .string(s)]) }
        }
    }
}
