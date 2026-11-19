import WheelKit
import SwiftUI

/// The names Wheel adds to the basic catalog.
let wheelA2UIExtensions: Set<String> = ["Table", "Canvas", "Terminal", "AgentChat", "WebView", "Unsupported",
    "ProgressView", "Gauge", "Menu", "Link", "Form", "Section", "LabeledContent", "Grid", "GridRow", "Stepper", "ColorPicker", "Popover", "Alert", "ConfirmationDialog", "NavigationSplitView", "NavigationStack", "ViewThatFits", "HSplitView", "VSplitView", "Toolbar", "Chart", "Map", "ShareLink", "PasteButton"]

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

/// Wheel's own components and the SwiftUI controls the basic catalog lacks, drawn natively, and the placeholder for a component
/// the catalog does not hold.
@MainActor
struct WheelA2UICatalog {
    @ViewBuilder
    func build(typeName: String, node: ViewNode, tree: ViewTree) -> some View {
        switch typeName {
        case "Table": A2UITableView(node: node, tree: tree)
        case "Canvas": A2UICanvasView(node: node, tree: tree)
        case "Terminal": A2UIPaneEmbedView(kind: .terminal, node: node)
        case "AgentChat": A2UIPaneEmbedView(kind: .agentChat, node: node)
        case "WebView": A2UIWebViewView(node: node)
        case "Unsupported": A2UIUnsupportedView(name: node.props["name"]?.stringValue ?? "?")
        case "ProgressView": A2UIProgressViewView(node: node)
        case "Gauge": A2UIGaugeView(node: node)
        case "Menu": A2UIMenuView(node: node, tree: tree)
        case "Link": A2UILinkView(node: node)
        case "Form": A2UIFormView(node: node, tree: tree)
        case "Section": A2UISectionView(node: node, tree: tree)
        case "LabeledContent": A2UILabeledContentView(node: node, tree: tree)
        case "Grid": A2UIGridView(node: node, tree: tree)
        case "GridRow": A2UIGridRowView(node: node, tree: tree)
        case "Stepper": A2UIStepperView(node: node, tree: tree)
        case "ColorPicker": A2UIColorPickerView(node: node, tree: tree)
        case "Popover": A2UIPopoverView(node: node, tree: tree)
        case "Alert": A2UIAlertView(node: node, tree: tree)
        case "ConfirmationDialog": A2UIConfirmationDialogView(node: node, tree: tree)
        case "NavigationSplitView": A2UINavigationSplitViewView(node: node, tree: tree)
        case "NavigationStack": A2UINavigationStackView(node: node, tree: tree)
        case "ViewThatFits": A2UIViewThatFitsView(node: node, tree: tree)
        case "HSplitView": A2UIHSplitViewView(node: node, tree: tree)
        case "VSplitView": A2UIVSplitViewView(node: node, tree: tree)
        case "Toolbar": A2UIToolbarView(node: node, tree: tree)
        case "Chart": A2UIChartView(node: node)
        case "Map": A2UIMapView(node: node)
        case "ShareLink": A2UIShareLinkView(node: node)
        case "PasteButton": A2UIPasteButtonView(node: node, tree: tree)
        default: EmptyView()
        }
    }
}

/// What stands where a component outside the catalog was asked for, so the layout is not silently
/// shorter than the server meant it.
struct A2UIUnsupportedView: View {
    let name: String
    var body: some View {
        Label("Unsupported component: \(name)", systemImage: "questionmark.square.dashed")
            .font(.system(size: 12))
            .foregroundStyle(.secondary)
            .padding(6)
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.secondary.opacity(0.5), style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
            .accessibilityIdentifier("a2ui.unsupported")
    }
}

