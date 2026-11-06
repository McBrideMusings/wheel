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

// MARK: SearchField — the system's rounded text field; it raises its action, text as `q`, as the person types
struct A2UISearchFieldView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @State private var text: String
    @State private var pending: Task<Void, Never>?
    /// Queries sent and not yet echoed back; an echo of one of them says nothing about what the field should hold.
    @State private var inFlight: [String] = []
    init(node: ComponentNode, surface: SurfaceModel) {
        self.node = node; self.surface = surface
        _text = State(initialValue: node.instance.properties["value"]?.stringValue ?? "")
    }
    var body: some View {
        let p = node.instance.properties
        let served = p["value"]?.stringValue ?? ""
        TextField(p["placeholder"]?.stringValue ?? "", text: $text)
            .textFieldStyle(.roundedBorder)
            .autocorrectionDisabled()
            .frame(minWidth: 160, idealWidth: 160)
            .overlay(alignment: .trailing) {
                // Spins from the keystroke until the page answers, so a slow answer reads as work in progress.
                if pending != nil || !inFlight.isEmpty {
                    ProgressView().controlSize(.small).padding(.trailing, 8).accessibilityIdentifier("a2ui.search.busy")
                }
            }
            .accessibilityIdentifier("a2ui.search")
            .onChange(of: served) { _, new in
                // The page echoes each query back. An echo of a query the person has typed past must not
                // rewrite the field; only a change the person did not make (Clear) does.
                if let i = inFlight.firstIndex(of: new) {
                    inFlight.removeFirst(i + 1)
                } else if new != text {
                    inFlight.removeAll()
                    text = new
                }
            }
            .onChange(of: text) { _, new in
                if new == served { return }
                pending?.cancel()
                guard let name = p["action"]?.stringValue else { return }
                pending = Task { @MainActor in
                    try? await Task.sleep(nanoseconds: 200_000_000)
                    if Task.isCancelled { return }
                    pending = nil
                    inFlight.append(new)
                    Task { @MainActor in
                        // An answer that never changes the field (same text back) would leave the spinner on.
                        try? await Task.sleep(nanoseconds: 5_000_000_000)
                        if let i = inFlight.firstIndex(of: new) { inFlight.remove(at: i) }
                    }
                    surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: ["q": .string(new)])
                }
            }
    }
}

// MARK: Segmented — the system's segmented control: `options` [{value, text}], `selected`, `action` raised with the value as `value`
struct A2UISegmentedView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        let options = (p["options"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }
        let binding = Binding<String>(
            get: { p["selected"]?.stringValue ?? "" },
            set: { new in
                guard let name = p["action"]?.stringValue else { return }
                surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: ["value": .string(new)])
            })
        Picker("", selection: binding) {
            ForEach(Array(options.enumerated()), id: \.offset) { _, o in
                Text(o["text"]?.stringValue ?? "").tag(o["value"]?.stringValue ?? "")
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
        .accessibilityIdentifier("a2ui.segmented")
    }
}

// MARK: Pill — the system's small bordered button raising its action; `style` link draws it as a link, `hideOff` keeps its room when it is disabled
struct A2UIPillStyle: ViewModifier {
    let style: String
    func body(content: Content) -> some View {
        if style == "plain" {
            content.buttonStyle(.borderless)
        } else if style == "link" {
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

struct A2UIPillView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let p = node.instance.properties
        let off = p["disabled"]?.boolValue == true
        Button {
            guard let name = p["name"]?.stringValue else { return }
            surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: p["context"]?.dictionaryValue ?? [:])
        } label: {
            if let icon = p["icon"]?.stringValue {
                Image(systemName: icon)
            } else {
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Text(p["text"]?.stringValue ?? "")
                    if let src = p["src"]?.stringValue {
                        Text(src).font(.caption2).foregroundStyle(.secondary)
                    }
                }
            }
        }
        .modifier(A2UIPillStyle(style: p["style"]?.stringValue ?? ""))
        .help(p["help"]?.stringValue ?? "")
        .accessibilityLabel(p["help"]?.stringValue ?? p["text"]?.stringValue ?? "")
        .disabled(off)
        .opacity(off && p["hideOff"]?.boolValue == true ? 0 : 1)
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
        .frame(width: p["width"]?.numberValue ?? 140, alignment: .leading)
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
            .fixedSize()
            .accessibilityIdentifier("a2ui.check")
    }
}

