import SwiftUI

// A component's optional `modifiers` property is a list of SwiftUI modifiers applied in order, each named as SwiftUI names it
// and taking SwiftUI's own arguments:
//   [{"font": {"size": 12, "weight": "semibold", "design": "monospaced"}}, {"foregroundStyle": "green"}, {"padding": 12}]
// font: a text style name ("caption") or {size, weight, design}; fontWeight; monospacedDigit; foregroundStyle and background: a
// colour name; padding: n | [v, h] | [top, right, bottom, left] | {horizontal|vertical|top|bottom|leading|trailing: n};
// frame: {width, height, minWidth, idealWidth, maxWidth, minHeight, idealHeight, maxHeight ("infinity" or n), alignment};
// fixedSize: true | {horizontal, vertical}; overlay: {stroke: colour,
// cornerRadius}; clipShape: {cornerRadius}; opacity; lineLimit; truncationMode (head, middle, tail); multilineTextAlignment; textSelection; textCase; tracking; help;
// disabled, accessibilityLabel. A name this does not map draws a visible marker, never nothing.

/// The colour a modifier's colour name means.
func a2uiColor(_ name: String?) -> Color? {
    switch name {
    case "primary": return .primary
    #if os(macOS)
    case "secondary": return Color(nsColor: .secondaryLabelColor)
    #else
    case "secondary": return Color(uiColor: .secondaryLabel)
    #endif
    case "red": return .red
    case "orange": return .orange
    case "yellow": return .yellow
    case "green": return .green
    case "mint": return .mint
    case "teal": return .teal
    case "cyan": return .cyan
    case "blue": return .blue
    case "indigo": return .indigo
    case "purple": return .purple
    case "pink": return .pink
    case "brown": return .brown
    case "gray": return .gray
    case "black": return .black
    case "white": return .white
    case "tint", "accentColor": return .accentColor
    #if os(macOS)
    case "separator": return Color(nsColor: .separatorColor)
    case "windowBackground": return Color(nsColor: .windowBackgroundColor)
    case "controlBackground": return Color(nsColor: .controlBackgroundColor)
    case "textBackground": return Color(nsColor: .textBackgroundColor)
    case "tertiary": return Color(nsColor: .tertiaryLabelColor)
    #else
    case "separator": return Color(uiColor: .separator)
    case "windowBackground": return Color(uiColor: .systemBackground)
    case "controlBackground": return Color(uiColor: .secondarySystemBackground)
    case "textBackground": return Color(uiColor: .systemBackground)
    case "tertiary": return Color(uiColor: .tertiaryLabel)
    #endif
    default: return nil
    }
}

private func fontWeight(_ name: String?) -> Font.Weight {
    switch name {
    case "ultraLight": return .ultraLight
    case "thin": return .thin
    case "light": return .light
    case "medium": return .medium
    case "semibold": return .semibold
    case "bold": return .bold
    case "heavy": return .heavy
    case "black": return .black
    default: return .regular
    }
}

private func textStyle(_ name: String) -> Font? {
    switch name {
    case "largeTitle": return .largeTitle
    case "title": return .title
    case "title2": return .title2
    case "title3": return .title3
    case "headline": return .headline
    case "subheadline": return .subheadline
    case "body": return .body
    case "callout": return .callout
    case "footnote": return .footnote
    case "caption": return .caption
    case "caption2": return .caption2
    default: return nil
    }
}

func a2uiFont(_ v: AnyCodable) -> Font? {
    if let s = v.stringValue { return textStyle(s) }
    guard let d = v.dictionaryValue else { return nil }
    let design: Font.Design = d["design"]?.stringValue == "monospaced" ? .monospaced : d["design"]?.stringValue == "serif" ? .serif : d["design"]?.stringValue == "rounded" ? .rounded : .default
    if let size = d["size"]?.numberValue { return .system(size: size, weight: fontWeight(d["weight"]?.stringValue), design: design) }
    if let style = d["style"]?.stringValue, let f = textStyle(style) { return f.weight(fontWeight(d["weight"]?.stringValue)) }
    return nil
}

func a2uiPadding(_ v: AnyCodable) -> EdgeInsets? {
    if let n = v.numberValue { return EdgeInsets(top: n, leading: n, bottom: n, trailing: n) }
    if let a = v.arrayValue?.compactMap({ $0.numberValue }) {
        if a.count == 2 { return EdgeInsets(top: a[0], leading: a[1], bottom: a[0], trailing: a[1]) }
        if a.count == 4 { return EdgeInsets(top: a[0], leading: a[3], bottom: a[2], trailing: a[1]) }
        return nil
    }
    guard let d = v.dictionaryValue else { return nil }
    let h = d["horizontal"]?.numberValue ?? 0, vv = d["vertical"]?.numberValue ?? 0
    return EdgeInsets(top: d["top"]?.numberValue ?? vv, leading: d["leading"]?.numberValue ?? h,
                      bottom: d["bottom"]?.numberValue ?? vv, trailing: d["trailing"]?.numberValue ?? h)
}

private func length(_ v: AnyCodable?) -> CGFloat? {
    if v?.stringValue == "infinity" { return .infinity }
    return v?.numberValue.map { CGFloat($0) }
}

private func alignment(_ name: String?) -> Alignment {
    switch name {
    case "leading": return .leading
    case "trailing": return .trailing
    case "top": return .top
    case "bottom": return .bottom
    case "center": return .center
    default: return .leading
    }
}

/// Applies a component's `modifiers` list, in order.
struct A2UIModifiers: ViewModifier {
    let list: [AnyCodable]

    func body(content: Content) -> some View {
        var view = AnyView(content)
        for entry in list {
            guard let d = entry.dictionaryValue, let (name, arg) = d.first else { continue }
            view = apply(name, arg, to: view)
        }
        return view
    }

    private func apply(_ name: String, _ arg: AnyCodable, to v: AnyView) -> AnyView {
        switch name {
        case "font":
            if let f = a2uiFont(arg) { return AnyView(v.font(f)) }
        case "fontWeight": return AnyView(v.fontWeight(fontWeight(arg.stringValue)))
        case "monospacedDigit": if arg.boolValue != false { return AnyView(v.monospacedDigit()) } else { return v }
        case "tint": if let c = a2uiColor(arg.stringValue) { return AnyView(v.tint(c)) }
        case "foregroundStyle":
            if let c = a2uiColor(arg.stringValue) { return AnyView(v.foregroundStyle(c)) }
        case "background":
            if let c = a2uiColor(arg.stringValue) { return AnyView(v.background(c)) }
        case "padding":
            if let e = a2uiPadding(arg) { return AnyView(v.padding(e)) }
        case "frame":
            if let d = arg.dictionaryValue {
                return AnyView(v.frame(minWidth: length(d["minWidth"]), idealWidth: length(d["idealWidth"]), maxWidth: length(d["maxWidth"]),
                                       minHeight: length(d["minHeight"]), idealHeight: length(d["idealHeight"]), maxHeight: length(d["maxHeight"]),
                                       alignment: alignment(d["alignment"]?.stringValue))
                    .frame(width: length(d["width"]), height: length(d["height"]), alignment: alignment(d["alignment"]?.stringValue)))
            }
        case "overlay":
            if let d = arg.dictionaryValue, let c = a2uiColor(d["stroke"]?.stringValue) {
                let r = d["cornerRadius"]?.numberValue ?? 0
                return AnyView(v.overlay(RoundedRectangle(cornerRadius: r, style: .continuous).strokeBorder(c)))
            }
        case "clipShape":
            if let r = arg.dictionaryValue?["cornerRadius"]?.numberValue { return AnyView(v.clipShape(RoundedRectangle(cornerRadius: r, style: .continuous))) }
        case "disabled": if arg.boolValue == true { return AnyView(v.disabled(true)) } else { return v }
        case "opacity": if let n = arg.numberValue { return AnyView(v.opacity(n)) }
        case "lineLimit": if let n = arg.numberValue { return AnyView(v.lineLimit(Int(n))) }
        case "truncationMode":
            return AnyView(v.truncationMode(arg.stringValue == "head" ? .head : arg.stringValue == "middle" ? .middle : .tail))
        case "layoutPriority": if let n = arg.numberValue { return AnyView(v.layoutPriority(n)) }
        case "fixedSize":
            if let d = arg.dictionaryValue {
                return AnyView(v.fixedSize(horizontal: d["horizontal"]?.boolValue ?? false, vertical: d["vertical"]?.boolValue ?? false))
            }
            if arg.boolValue == true { return AnyView(v.fixedSize()) } else { return v }
        case "multilineTextAlignment":
            return AnyView(v.multilineTextAlignment(arg.stringValue == "trailing" ? .trailing : arg.stringValue == "center" ? .center : .leading))
        case "textSelection": if arg.boolValue == true { return AnyView(v.textSelection(.enabled)) } else { return v }
        case "textCase": return AnyView(v.textCase(arg.stringValue == "uppercase" ? .uppercase : arg.stringValue == "lowercase" ? .lowercase : nil))
        case "tracking": if let n = arg.numberValue { return AnyView(v.tracking(n)) }
        case "help": if let s = arg.stringValue { return AnyView(v.help(s)) }
        case "accessibilityLabel": if let s = arg.stringValue { return AnyView(v.accessibilityLabel(s)) }
        default: break
        }
        return AnyView(v.overlay(alignment: .topTrailing) { Text("⚠︎ \(name)").font(.caption2).foregroundStyle(.red) })
    }
}

extension View {
    /// The component's `modifiers` property, applied.
    func a2uiModified(_ node: ComponentNode) -> some View {
        modifier(A2UIModifiers(list: node.instance.properties["modifiers"]?.arrayValue ?? []))
    }
}

