import AVKit
import SwiftUI

// The 18 components of A2UI's basic catalog, drawn as SwiftUI draws them. A control that holds a value reads and writes the surface's
// data model at its path (`a2uiPath`); it never names an action or a tool.

/// Builds the view for a node: a basic component, one of Wheel's own, or a SwiftUI control (A2UISystemControls).
struct A2UIChildView: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        view.a2uiModified(node)
    }

    @ViewBuilder
    private var view: some View {
        switch node.instance.type {
        case "Text": A2UIText(node: node)
        case "Image": A2UIImage(node: node)
        case "Icon": A2UIIcon(node: node)
        case "Video": A2UIVideo(node: node)
        case "AudioPlayer": A2UIUnsupportedView(node: ComponentNode(instance: ComponentInstance(id: node.instance.id, type: "Unsupported", properties: ["name": .string("AudioPlayer")]), children: []))
        case "Row": A2UIStack(node: node, surface: surface, horizontal: true)
        case "Column": A2UIStack(node: node, surface: surface, horizontal: false)
        case "List": A2UIList(node: node, surface: surface)
        case "Card": GroupBox { if let c = node.children.first { A2UIChildView(node: c, surface: surface) } }
        case "Tabs": A2UITabs(node: node, surface: surface)
        case "Divider": if node.instance.properties["axis"]?.stringValue == "vertical" { Divider().frame(height: 18) } else { Divider() }
        case "Modal": A2UIModal(node: node, surface: surface)
        case "Button": A2UIButton(node: node, surface: surface)
        case "CheckBox": A2UICheckBox(node: node, surface: surface)
        case "TextField": A2UITextField(node: node, surface: surface)
        case "DateTimeInput": A2UIDateTimeInput(node: node, surface: surface)
        case "ChoicePicker": A2UIChoicePicker(node: node, surface: surface)
        case "Slider": A2UISlider(node: node, surface: surface)
        default: WheelA2UICatalog().build(typeName: node.instance.type, node: node, surface: surface)
        }
    }
}

/// The page: the root component resolved, scrolled by the pane that holds it.
struct A2UISurfaceView: View {
    let surface: SurfaceModel
    var body: some View {
        // The Mac's body size on every platform, so a page reads the same on a phone; a `font` modifier on a component still wins.
        if let root = surface.root { A2UIChildView(node: root, surface: surface).font(.system(size: 13)) }
    }
}

/// What a control shows: the data model's value at its path, else the value the server sent in the component.
@MainActor
func a2uiValue(_ node: ComponentNode, _ surface: SurfaceModel) -> AnyCodable? {
    surface.get(a2uiPath(of: node.instance, in: surface)) ?? node.props["value"]
}

// MARK: Text — `text`, `variant` (h1 to h5, body, caption). Body sets no font, so a `font` modifier on it takes effect.
struct A2UIText: View {
    let node: ComponentNode
    var body: some View {
        let t = Text(node.props["text"]?.text ?? "")
        switch node.props["variant"]?.stringValue {
        case "h1": t.font(.system(size: 28, weight: .bold))
        case "h2": t.font(.system(size: 20, weight: .bold))
        case "h3": t.font(.system(size: 17, weight: .semibold))
        case "h4": t.font(.system(size: 15, weight: .bold))
        case "h5": t.font(.system(size: 14, weight: .semibold))
        case "caption": t.font(.system(size: 12)).foregroundStyle(.secondary)
        default: t
        }
    }
}

// MARK: Image — `url`, `fit`, `variant`
struct A2UIImage: View {
    let node: ComponentNode
    var body: some View {
        let size: CGFloat? = switch node.props["variant"]?.stringValue {
        case "icon": 24
        case "avatar": 40
        case "smallFeature": 50
        case "mediumFeature": 150
        case "largeFeature": 400
        default: nil
        }
        AsyncImage(url: node.props["url"]?.stringValue.flatMap(URL.init(string:))) { img in
            img.resizable().aspectRatio(contentMode: node.props["fit"]?.stringValue == "cover" ? .fill : .fit)
        } placeholder: { ProgressView().controlSize(.small) }
        .frame(maxWidth: size)
        .clipShape(RoundedRectangle(cornerRadius: node.props["variant"]?.stringValue == "avatar" ? 20 : 0))
    }
}

// MARK: Icon — `name`, an SF Symbol or one of the material names
struct A2UIIcon: View {
    let node: ComponentNode
    private static let material = ["check": "checkmark", "close": "xmark", "add": "plus", "delete": "trash", "edit": "pencil", "home": "house",
                                   "search": "magnifyingglass", "settings": "gearshape", "star": "star", "info": "info.circle", "warning": "exclamationmark.triangle",
                                   "play": "play.fill", "pause": "pause.fill", "refresh": "arrow.clockwise", "send": "paperplane", "share": "square.and.arrow.up"]
    var body: some View {
        let name = node.props["name"]?.stringValue ?? ""
        Image(systemName: Self.material[name] ?? name)
    }
}

// MARK: Video — `url`
struct A2UIVideo: View {
    let node: ComponentNode
    @State private var player: AVPlayer?
    var body: some View {
        VideoPlayer(player: player).frame(minHeight: 180)
            .onAppear { if player == nil, let url = node.props["url"]?.stringValue.flatMap(URL.init(string:)) { player = AVPlayer(url: url) } }
    }
}

// MARK: Row and Column — `spacing` (VStack and HStack's own), `justify` (start, center, end, spaceBetween, spaceEvenly, spaceAround), `align` (start, center, end, stretch)
struct A2UIStack: View {
    let node: ComponentNode
    let surface: SurfaceModel
    let horizontal: Bool

    var body: some View {
        let justify = node.props["justify"]?.stringValue ?? "start"
        let kids = Array(node.children.enumerated())
        let spacing = node.props["spacing"]?.numberValue.map { CGFloat($0) } ?? 8
        Group {
            if horizontal {
                HStack(alignment: vertical(node.props["align"]?.stringValue), spacing: spacing) { content(kids, justify) }
            } else {
                VStack(alignment: horizontalAlign(node.props["align"]?.stringValue), spacing: spacing) { content(kids, justify) }
                    .frame(maxWidth: .infinity, alignment: frameAlignment(node.props["align"]?.stringValue))
            }
        }
    }

    @ViewBuilder
    private func content(_ kids: [(offset: Int, element: ComponentNode)], _ justify: String) -> some View {
        if justify == "end" || justify == "center" || justify == "spaceEvenly" || justify == "spaceAround" { Spacer(minLength: 0) }
        ForEach(kids, id: \.offset) { i, k in
            if i > 0 && justify == "spaceBetween" || i > 0 && justify == "spaceEvenly" || i > 0 && justify == "spaceAround" { Spacer(minLength: 0) }
            A2UIChildView(node: k, surface: surface)
        }
        // The spacer that keeps a start-aligned stack at the top gives way to a Table, which takes the height left over.
        // Only the page column gets it: in a card or a box a spacer made the stack take height and stretched it.
        let pinsTop = horizontal || node.instance.id == "page"
        if pinsTop && (justify == "start" || justify == "center" || justify == "spaceEvenly" || justify == "spaceAround") { Spacer(minLength: 0).layoutPriority(-1) }
    }

    private func vertical(_ a: String?) -> VerticalAlignment { a == "start" ? .top : a == "end" ? .bottom : .center }
    private func horizontalAlign(_ a: String?) -> HorizontalAlignment { a == "center" ? .center : a == "end" ? .trailing : .leading }
    private func frameAlignment(_ a: String?) -> Alignment { a == "center" ? .center : a == "end" ? .trailing : .leading }
}

// MARK: List — `direction` (vertical, horizontal)
struct A2UIList: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let horizontal = node.props["direction"]?.stringValue == "horizontal"
        ScrollView(horizontal ? .horizontal : .vertical) {
            if horizontal {
                HStack(spacing: node.props["spacing"]?.numberValue.map { CGFloat($0) } ?? 8) { items }
            } else {
                LazyVStack(alignment: .leading, spacing: node.props["spacing"]?.numberValue.map { CGFloat($0) } ?? 8) { items }
            }
        }
        .fixedSize(horizontal: false, vertical: horizontal)
    }
    private var items: some View {
        ForEach(Array(node.children.enumerated()), id: \.offset) { _, k in A2UIChildView(node: k, surface: surface) }
    }
}

// MARK: Tabs — `tabs` [{title, child}]
struct A2UITabs: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let titles = (node.props["tabs"]?.arrayValue ?? []).compactMap { $0.dictionaryValue?["title"]?.stringValue }
        TabView {
            ForEach(Array(node.children.enumerated()), id: \.offset) { i, k in
                A2UIChildView(node: k, surface: surface).tabItem { Text(i < titles.count ? titles[i] : "") }
            }
        }
    }
}

// MARK: Modal — `trigger` opens a sheet holding `content`. With `presented` the server opens it: it dims and covers its own pane
// (A2UIPresentedModals, drawn by the pane over the page), and `dismissAction` is raised when it closes.
struct A2UIModal: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @State private var open = false
    var body: some View {
        if node.instance.properties["presented"] == nil, node.children.count >= 2 {
            A2UIChildView(node: node.children[0], surface: surface)
                .simultaneousGesture(TapGesture().onEnded { open = true })
                .sheet(isPresented: $open) {
                    // The system's sheet: a title bar with Done, a grabber and medium and large heights on a phone.
                    NavigationStack {
                        A2UIChildView(node: node.children[1], surface: surface)
                            .navigationTitle(node.props["title"]?.text ?? "")
                            #if os(iOS)
                            .navigationBarTitleDisplayMode(.inline)
                            #endif
                            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { open = false } } }
                    }
                    #if os(iOS)
                    .presentationDetents([.medium, .large])
                    #else
                    .frame(minWidth: 380, minHeight: 420)
                    #endif
                }
        }
    }
}

/// The presented Modals of a surface, drawn over the pane that shows it: a dimmed scrim and a card, so only that pane is covered.
struct A2UIPresentedModals: View {
    let surface: SurfaceModel
    var body: some View {
        let ids = surface.components.values.filter { $0.type == "Modal" && $0.properties["presented"]?.boolValue == true }.map(\.id).sorted()
        ForEach(ids, id: \.self) { id in
            if let node = surface.node(id) { card(node) }
        }
    }

    private func card(_ node: ComponentNode) -> some View {
        let dismiss = node.instance.properties["dismissAction"]?.stringValue
        let close = { if let dismiss { surface.dispatchAction(name: dismiss, sourceComponentId: node.instance.id) } }
        return ZStack {
            Color.black.opacity(0.4).onTapGesture(perform: close)
            VStack(spacing: 0) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(Array(node.children.enumerated()), id: \.offset) { _, k in A2UIChildView(node: k, surface: surface) }
                    }
                    .font(.system(size: 13))
                    .frame(maxWidth: .infinity, alignment: .leading).padding(18)
                }
                if dismiss != nil {
                    Divider()
                    HStack { Spacer(); Button("Close", action: close).keyboardShortcut(.cancelAction) }.padding(10)
                }
            }
            .frame(maxWidth: 640, maxHeight: .infinity)
            .background(a2uiColor("controlBackground") ?? Color.gray.opacity(0.2), in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.separator))
            .shadow(radius: 18)
            .padding(24)
        }
        .accessibilityIdentifier("a2ui.modal")
    }
}

// MARK: Button — `child`, `variant` (default, primary, borderless), `action` {event: {name, context}}
struct A2UIButton: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let button = Button {
            guard let event = node.props["action"]?.dictionaryValue?["event"]?.dictionaryValue, let name = event["name"]?.stringValue else { return }
            var context: [String: AnyCodable] = [:]
            for (k, v) in event["context"]?.dictionaryValue ?? [:] {
                if let p = v.dictionaryValue?["path"]?.stringValue, v.dictionaryValue?.count == 1 { context[k] = surface.get(p) ?? .null } else { context[k] = v }
            }
            surface.dispatchAction(name: name, sourceComponentId: node.instance.id, context: context)
        } label: {
            if let c = node.children.first { A2UIChildView(node: c, surface: surface) }
        }
        switch node.props["variant"]?.stringValue {
        case "primary": button.buttonStyle(.borderedProminent)
        case "borderless": button.buttonStyle(.borderless)
        default: button
        }
    }
}

// MARK: CheckBox — `label`, `value`
private struct A2UIInFormKey: EnvironmentKey { static let defaultValue = false }
extension EnvironmentValues {
    /// Whether a component is a row of a Form, where a choice draws as its own rows.
    var a2uiInForm: Bool {
        get { self[A2UIInFormKey.self] }
        set { self[A2UIInFormKey.self] = newValue }
    }
}

private struct A2UIInMenuKey: EnvironmentKey { static let defaultValue = false }
extension EnvironmentValues {
    /// Whether a component is an item of a Menu, where a Toggle draws as a tick row.
    var a2uiInMenu: Bool {
        get { self[A2UIInMenuKey.self] }
        set { self[A2UIInMenuKey.self] = newValue }
    }
}

struct A2UICheckBox: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @Environment(\.a2uiInMenu) private var inMenu
    var body: some View {
        let path = a2uiPath(of: node.instance, in: surface)
        let binding = Binding<Bool>(get: { a2uiValue(node, surface)?.boolValue ?? false }, set: { surface.set(path, .bool($0)) })
        let toggle = Toggle(node.props["label"]?.text ?? "", isOn: binding)
        #if os(macOS)
        if inMenu { toggle } else { toggle.toggleStyle(.checkbox) }
        #else
        toggle
        #endif
    }
}

// MARK: TextField — `label`, `value`, `variant` (shortText, longText, number, obscured)
struct A2UITextField: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let path = a2uiPath(of: node.instance, in: surface)
        let label = node.props["label"]?.text ?? ""
        let binding = Binding<String>(get: { a2uiValue(node, surface)?.text ?? "" }, set: { surface.set(path, .string($0)) })
        switch node.props["variant"]?.stringValue {
        case "longText": TextEditor(text: binding).frame(minHeight: 60).overlay(alignment: .topLeading) { if binding.wrappedValue.isEmpty { Text(label).foregroundStyle(.tertiary).padding(6) } }
        case "obscured": SecureField(label, text: binding)
        case "number": TextField(label, text: binding).autocorrectionDisabled()
        default:
            // A filled, bordered box, so it reads as something to tap on a dark page.
            TextField(label, text: binding).autocorrectionDisabled().textFieldStyle(.plain)
                .padding(.horizontal, 10).padding(.vertical, 8)
                .background(Color.secondary.opacity(0.16), in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color.secondary.opacity(0.45)))
        }
    }
}

// MARK: DateTimeInput — `label`, `value` (ISO 8601), `enableDate`, `enableTime`
struct A2UIDateTimeInput: View {
    let node: ComponentNode
    let surface: SurfaceModel
    private static let formats = ["yyyy-MM-dd'T'HH:mm:ss", "yyyy-MM-dd", "'T'HH:mm:ss", "HH:mm:ss"]
    var body: some View {
        let path = a2uiPath(of: node.instance, in: surface)
        let date = node.props["enableDate"]?.boolValue ?? true, time = node.props["enableTime"]?.boolValue ?? false
        let components: DatePickerComponents = date && time ? [.date, .hourAndMinute] : time ? [.hourAndMinute] : [.date]
        let binding = Binding<Date>(
            get: { Self.parse(a2uiValue(node, surface)?.stringValue) },
            set: { surface.set(path, .string(Self.format($0, date: date, time: time))) })
        DatePicker(node.props["label"]?.text ?? "", selection: binding, displayedComponents: components)
    }
    private static func parse(_ s: String?) -> Date {
        guard let s else { return Date() }
        for f in formats { let d = DateFormatter(); d.dateFormat = f; d.locale = Locale(identifier: "en_US_POSIX"); if let v = d.date(from: s) { return v } }
        return Date()
    }
    private static func format(_ d: Date, date: Bool, time: Bool) -> String {
        let f = DateFormatter(); f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = date && time ? "yyyy-MM-dd'T'HH:mm:ss" : time ? "'T'HH:mm:ss" : "yyyy-MM-dd"
        return f.string(from: d)
    }
}

// MARK: ChoicePicker — `label`, `options` [{label, value}], `value` [selected], `variant` (mutuallyExclusive, multipleSelection), `displayStyle` (checkbox, chips), `filterable`
struct A2UIChoicePicker: View {
    let node: ComponentNode
    let surface: SurfaceModel
    @State private var filter = ""
    @Environment(\.a2uiPane) private var pane
    @Environment(\.a2uiInForm) private var inForm
    var body: some View {
        let path = a2uiPath(of: node.instance, in: surface)
        let all = (node.props["options"]?.arrayValue ?? []).compactMap { $0.dictionaryValue }.map { (label: $0["label"]?.text ?? "", value: $0["value"]?.text ?? "") }
        let options = filter.isEmpty ? all : all.filter { $0.label.localizedCaseInsensitiveContains(filter) }
        let selected = (a2uiValue(node, surface)?.arrayValue ?? []).map(\.text)
        let multiple = node.props["variant"]?.stringValue == "multipleSelection"
        let single = Binding<String>(get: { selected.first ?? "" }, set: { surface.set(path, .array([.string($0)])) })
        let title = node.props["label"]?.text ?? ""
        if inForm {
            // In a Form each option is its own row: a tick for one of several, and a labelled menu row for a single choice.
            if multiple {
                ForEach(options, id: \.value) { o in
                    let on = selected.contains(o.value)
                    Button {
                        var next = selected.filter { $0 != o.value }
                        if !on { next.append(o.value) }
                        surface.set(path, .array(next.map { .string($0) }))
                    } label: {
                        HStack { Text(o.label); Spacer(); if on { Image(systemName: "checkmark").foregroundStyle(.tint).fontWeight(.semibold) } }
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            } else {
                Picker(title, selection: single) { ForEach(options, id: \.value) { Text($0.label).tag($0.value) } }.pickerStyle(.menu)
            }
        } else {
        VStack(alignment: .leading, spacing: 6) {
            if node.props["filterable"]?.boolValue == true { TextField("Filter", text: $filter) }
            if multiple {
                if node.props["displayStyle"]?.stringValue == "chips" {
                    HStack { ForEach(options, id: \.value) { o in toggle(o, selected, path, chip: true) } }
                } else {
                    VStack(alignment: .leading) { ForEach(options, id: \.value) { o in toggle(o, selected, path, chip: false) } }
                }
            } else if node.props["displayStyle"]?.stringValue == "chips" || options.count <= 4 {
                // A narrow pane gets the menu, which is one control wide. A wider one gets the segmented control when it fits the width
                // it is given and the menu when it does not, so it never runs off the pane.
                if pane.width < 640 {
                    Picker(title, selection: single) { ForEach(options, id: \.value) { Text($0.label).tag($0.value) } }
                        .pickerStyle(.menu).labelsHidden().fixedSize()
                } else {
                ViewThatFits(in: .horizontal) {
                    Picker(title, selection: single) { ForEach(options, id: \.value) { Text($0.label).tag($0.value) } }
                        .pickerStyle(.segmented).labelsHidden().fixedSize(horizontal: true, vertical: false)
                    Picker(title, selection: single) { ForEach(options, id: \.value) { Text($0.label).tag($0.value) } }
                        .pickerStyle(.menu).labelsHidden().frame(maxWidth: .infinity, alignment: .leading)
                }
                }
            } else {
                Picker(title, selection: single) { ForEach(options, id: \.value) { Text($0.label).tag($0.value) } }.pickerStyle(.menu).labelsHidden().fixedSize()
            }
        }
        }
    }

    @ViewBuilder
    private func toggle(_ o: (label: String, value: String), _ selected: [String], _ path: String, chip: Bool) -> some View {
        let on = selected.contains(o.value)
        let binding = Binding<Bool>(get: { on }, set: { new in
            var next = selected.filter { $0 != o.value }
            if new { next.append(o.value) }
            surface.set(path, .array(next.map { .string($0) }))
        })
        if chip { Toggle(o.label, isOn: binding).toggleStyle(.button) } else {
            Toggle(o.label, isOn: binding)
                #if os(macOS)
                .toggleStyle(.checkbox)
                #endif
        }
    }
}

// MARK: Slider — `label`, `value`, `min`, `max`
struct A2UISlider: View {
    let node: ComponentNode
    let surface: SurfaceModel
    var body: some View {
        let path = a2uiPath(of: node.instance, in: surface)
        let lo = node.props["min"]?.numberValue ?? 0, hi = node.props["max"]?.numberValue ?? 100
        let binding = Binding<Double>(get: { a2uiValue(node, surface)?.numberValue ?? lo }, set: { surface.set(path, .number($0)) })
        Slider(value: binding, in: lo...hi) { Text(node.props["label"]?.text ?? "") }
    }
}
