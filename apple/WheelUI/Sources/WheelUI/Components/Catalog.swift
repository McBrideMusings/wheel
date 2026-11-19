import WheelKit
import SwiftUI

// The palette is the system's own semantic colours, so a page follows light and dark and the
// person's accent colour like any native window.
struct WheelPalette {
    let fg, muted, bg, card, soft, line, ok, warn, bad, todo, focus, accent, purple: Color
    init(dark _: Bool) {
        let t = WebTheme.shared
        bg = t.bg; card = t.group; line = t.separator
        fg = t.label; muted = t.secondary; soft = t.fill; todo = t.tertiary
        ok = t.green; warn = t.orange; bad = t.red; focus = t.blue; accent = t.blue; purple = t.purple
    }
    func tone(_ n: String?) -> Color? {
        switch n {
        case "fg": return fg; case "muted": return muted; case "ok": return ok; case "warn": return warn
        case "bad": return bad; case "line": return line; case "card": return card; case "soft": return soft
        case "accent": return accent; case "purple": return purple; case "bg": return bg; case "todo": return todo; case "focus": return focus; case "faint": return todo; default: return nil
        }
    }
}

func weightOf(_ s: String?) -> Font.Weight {
    switch s { case "bold": return .bold; case "semibold": return .semibold; case "medium": return .medium; default: return .regular }
}
func insets(_ v: JSONValue?) -> EdgeInsets {
    guard let a = v?.arrayValue?.compactMap({ $0.numberValue }) else { return EdgeInsets() }
    if a.count == 2 { return EdgeInsets(top: a[0], leading: a[1], bottom: a[0], trailing: a[1]) }
    if a.count == 4 { return EdgeInsets(top: a[0], leading: a[3], bottom: a[2], trailing: a[1]) }
    return EdgeInsets()
}

/// Wheel's own components and the SwiftUI controls the basic components lack, drawn natively, and the placeholder for a component
/// the catalog does not hold.
@MainActor
struct PaneCatalog {
    @ViewBuilder
    func build(typeName: String, node: ViewNode, tree: ViewTree) -> some View {
        switch typeName {
        case "Table": PaneTableView(node: node, tree: tree)
        case "Canvas": PaneCanvasView(node: node, tree: tree)
        case "Terminal": PaneEmbedView(kind: .terminal, node: node)
        case "AgentChat": PaneEmbedView(kind: .agentChat, node: node)
        case "WebView": PaneWebViewView(node: node)
        case "Unsupported": PaneUnsupportedView(name: node.props["name"]?.stringValue ?? "?")
        case "ProgressView": PaneProgressViewView(node: node)
        case "Gauge": PaneGaugeView(node: node)
        case "Menu": PaneMenuView(node: node, tree: tree)
        case "Link": PaneLinkView(node: node)
        case "Form": PaneFormView(node: node, tree: tree)
        case "Section": PaneSectionView(node: node, tree: tree)
        case "LabeledContent": PaneLabeledContentView(node: node, tree: tree)
        case "Grid": PaneGridView(node: node, tree: tree)
        case "GridRow": PaneGridRowView(node: node, tree: tree)
        case "Stepper": PaneStepperView(node: node, tree: tree)
        case "ColorPicker": PaneColorPickerView(node: node, tree: tree)
        case "Popover": PanePopoverView(node: node, tree: tree)
        case "Alert": PaneAlertView(node: node, tree: tree)
        case "ConfirmationDialog": PaneConfirmationDialogView(node: node, tree: tree)
        case "NavigationSplitView": PaneNavigationSplitViewView(node: node, tree: tree)
        case "NavigationStack": PaneNavigationStackView(node: node, tree: tree)
        case "ViewThatFits": PaneViewThatFitsView(node: node, tree: tree)
        case "HSplitView": PaneHSplitViewView(node: node, tree: tree)
        case "VSplitView": PaneVSplitViewView(node: node, tree: tree)
        case "Toolbar": PaneToolbarView(node: node, tree: tree)
        case "Chart": PaneChartView(node: node)
        case "Map": PaneMapView(node: node)
        case "ShareLink": PaneShareLinkView(node: node)
        case "PasteButton": PanePasteButtonView(node: node, tree: tree)
        default: EmptyView()
        }
    }
}

/// What stands where a component outside the catalog was asked for, so the layout is not silently
/// shorter than the server meant it.
struct PaneUnsupportedView: View {
    let name: String
    var body: some View {
        Label("Unsupported component: \(name)", systemImage: "questionmark.square.dashed")
            .font(.system(size: 12))
            .foregroundStyle(.secondary)
            .padding(6)
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.secondary.opacity(0.5), style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
            .accessibilityIdentifier("pane.unsupported")
    }
}

