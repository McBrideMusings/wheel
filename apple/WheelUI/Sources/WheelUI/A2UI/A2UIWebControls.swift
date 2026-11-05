import A2UISwiftCore
import A2UISwiftUI
import SwiftUI

// The page's controls, drawn as the web artifact draws them (`daemon/src/wheel.css`): a bordered
// search field, a segmented control whose buttons are as wide pressed as not, pop-up menus of a
// fixed width, a checkbox, small bordered buttons and one grouped box of counts.

private struct A2UIViewportKey: EnvironmentKey { static let defaultValue = CGSize(width: 640, height: 640) }
extension EnvironmentValues {
    /// The pane's own size, which the page's rows and lists fit themselves to.
    var a2uiPane: CGSize {
        get { self[A2UIViewportKey.self] }
        set { self[A2UIViewportKey.self] = newValue }
    }
    var a2uiViewport: CGFloat { a2uiPane.height }
}

// MARK: SearchField — a bordered text field; it raises its action, text as `q`, as the person types
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
        let t = WebTheme.shared
        let served = p["value"]?.stringValue ?? ""
        TextField("", text: $text, prompt: Text(p["placeholder"]?.stringValue ?? "").foregroundColor(t.tertiary))
            .textFieldStyle(.plain)
            .font(.system(size: WebTheme.body))
            .foregroundStyle(t.label)
            .autocorrectionDisabled()
            #if os(iOS)
            .textInputAutocapitalization(.never)
            #endif
            .padding(.horizontal, 8)
            .frame(minWidth: 160, idealWidth: 160, minHeight: 26, maxHeight: 26)
            .background(t.group, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(t.separatorStrong, lineWidth: 1))
            .accessibilityIdentifier("a2ui.search")
            .onChange(of: served) { _, new in if new != text { text = new } }
            .onChange(of: text) { _, new in
                if new == served { return }
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

// MARK: Segmented — `options` [{value, text}], `selected`, `action` raised with the value as `value`
struct A2UISegmentedView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        let t = WebTheme.shared
        let options = (p["options"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let selected = p["selected"]?.stringValue ?? ""
        HStack(spacing: 0) {
            ForEach(Array(options.enumerated()), id: \.offset) { i, o in
                let value = o["value"]?.stringValue ?? ""
                let text = o["text"]?.stringValue ?? ""
                let on = value == selected
                if i > 0 { Rectangle().fill(t.separator).frame(width: 1) }
                Button {
                    guard let name = p["action"]?.stringValue else { return }
                    surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: ["value": .string(value)])
                } label: {
                    // The bold label sits unseen under the shown one, so a button is as wide pressed as not.
                    ZStack {
                        Text(text).font(.system(size: WebTheme.callout, weight: .semibold)).hidden()
                        Text(text).font(.system(size: WebTheme.callout, weight: on ? .semibold : .regular))
                    }
                    .foregroundStyle(on ? t.label : t.secondary)
                    .padding(.horizontal, 11)
                    .frame(height: 24)
                    .background(on ? t.accentFill : Color.clear)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .background(t.group)
        .clipShape(RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(t.separatorStrong, lineWidth: 1))
        .fixedSize()
        .accessibilityIdentifier("a2ui.segmented")
    }
}

// MARK: Pill — a small bordered button raising its action; `style` link draws it as accent text, `hideOff` keeps its room when it is disabled
struct A2UIPillView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        let t = WebTheme.shared
        let link = p["style"]?.stringValue == "link"
        let off = p["disabled"]?.boolValue == true
        Button {
            guard let name = p["name"]?.stringValue else { return }
            surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: p["context"]?.dictionaryValue ?? [:])
        } label: {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(p["text"]?.stringValue ?? "")
                    .font(.system(size: WebTheme.callout))
                    .foregroundStyle(link ? t.blue : t.label)
                    .multilineTextAlignment(.leading)
                if let src = p["src"]?.stringValue {
                    Text(src).font(.system(size: WebTheme.caption)).foregroundStyle(t.secondary)
                }
            }
            .padding(.horizontal, link ? 0 : 10)
            .frame(height: link ? nil : 26)
            .background { if !link { RoundedRectangle(cornerRadius: 6).fill(t.group) } }
            .overlay { if !link { RoundedRectangle(cornerRadius: 6).strokeBorder(t.separatorStrong, lineWidth: 1) } }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(off)
        .opacity(off && p["hideOff"]?.boolValue == true ? 0 : off ? 0.5 : 1)
        .accessibilityIdentifier("a2ui.pill.\(node.instance.id)")
    }
}

// MARK: Select — the system's pop-up menu of a set width: `options` [{value, text}], `selected`; raises its action with `key` and `value`
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
        .frame(width: p["width"]?.numberValue ?? 140, height: 26, alignment: .leading)
        .disabled(p["disabled"]?.boolValue == true)
        .accessibilityIdentifier("a2ui.select.\(p["key"]?.stringValue ?? "")")
    }
}

// MARK: Check — a checkbox: `text`, `value`; raises its action with `value` "true" or "false"
struct A2UICheckView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        let t = WebTheme.shared
        let on = p["value"]?.boolValue ?? false
        Button {
            guard let name = p["action"]?.stringValue else { return }
            surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: ["value": .string(on ? "false" : "true")])
        } label: {
            HStack(spacing: 5) {
                ZStack {
                    RoundedRectangle(cornerRadius: 3).fill(on ? t.blue : t.group)
                    RoundedRectangle(cornerRadius: 3).strokeBorder(on ? t.blue : t.separatorStrong, lineWidth: 1)
                    if on { Image(systemName: "checkmark").font(.system(size: 9, weight: .heavy)).foregroundStyle(.white) }
                }
                .frame(width: 14, height: 14)
                Text(p["text"]?.stringValue ?? "").font(.system(size: WebTheme.callout)).foregroundStyle(t.label)
            }
            .frame(height: 26)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("a2ui.check")
    }
}

// MARK: Stat — a count over its caption; the cells of a box with `dividers` make the counts strip
struct A2UIStatView: View {
    let node: ComponentNode
    var body: some View {
        let p = node.instance.properties
        let t = WebTheme.shared
        let zero = p["tone"]?.stringValue == "muted"
        VStack(alignment: .leading, spacing: 0) {
            Text(p["value"]?.stringValue ?? "")
                .font(.system(size: 22, weight: .semibold).monospacedDigit())
                .foregroundStyle(zero ? t.tertiary : (t.tone(p["tone"]?.stringValue) ?? t.label))
            Text(p["label"]?.stringValue ?? "")
                .font(.system(size: WebTheme.caption))
                .foregroundStyle(t.secondary)
                .lineLimit(1)
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
