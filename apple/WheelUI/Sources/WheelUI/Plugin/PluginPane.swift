import Foundation
import Observation
import SwiftUI
import WheelKit
internal import WheelLuau

/// A plugin pane's body (ADR-0041): the Luau script's `view` drawn as A2UI components. The pane owns the
/// script's `PaneRuntime` and a `SurfaceModel`. A Button whose `action` is a plain table sends that table to
/// the script's `update` (every table action: a pane script never speaks A2UI's action shape); a TextField's `on_change` table gets the typed text as `value` and goes the same way.
/// After every send or reload the script's `view` is drawn again. A failure keeps the last good view and sets
/// `error`; a good load clears it.
@MainActor @Observable
public final class PluginPane {
    public let surface = SurfaceModel()
    /// The last failure as one readable line, or nil.
    public private(set) var error: String?
    /// Reloads that took the new source, not counting the first load.
    public private(set) var reloads = 0
    /// The script's file name as wheeld sent it, in error messages.
    public private(set) var chunk = "pane"

    @ObservationIgnored private var runtime: PaneRuntime?
    /// Whether `error` came from loading the script: the file is still broken, so a message the script handles does not clear it.
    @ObservationIgnored private var loadFailed = false
    /// For each text field's data path, the table its `on_change` names.
    @ObservationIgnored private var changeMsgs: [String: JSONValue] = [:]
    /// Called after every load, send and failure, for a host that reports `error`.
    @ObservationIgnored public var onUpdate: (() -> Void)?

    public init() {
        surface.onAction = { [weak self] action in
            guard let self, action.name == "plugin.send", let msg = action.context["msg"] else { return }
            self.deliver(Self.json(msg))
        }
        surface.onChange = { [weak self] change in
            guard let self, let template = self.changeMsgs[change.path] else { return }
            var msg: [String: JSONValue] = [:]
            if case let .object(fields) = template { msg = fields }
            msg["value"] = Self.json(change.value)
            self.deliver(.object(msg))
        }
    }

    /// Takes the script's source: the first call starts the runtime, a later one reloads it with its state kept.
    public func load(source: String, chunk: String) {
        self.chunk = chunk
        do {
            if let runtime {
                try runtime.reload(source: source)
                reloads += 1
            } else {
                runtime = try PaneRuntime(source: source, chunkName: chunk)
            }
            try render()
            error = nil
            loadFailed = false
        } catch {
            fail(error)
            loadFailed = true
        }
        onUpdate?()
    }

    /// Reports a failure that is not the script's (wheeld could not give the source).
    public func fail(message: String) {
        error = Self.oneLine(message)
        onUpdate?()
    }

    private func deliver(_ msg: JSONValue) {
        guard let runtime else { return }
        do {
            try runtime.send(msg)
            try render()
            if !loadFailed { error = nil }
        } catch {
            fail(error)
        }
        onUpdate?()
    }

    private func fail(_ failure: Error) {
        let text = (failure as? PaneRuntime.Failure)?.description ?? "\(failure)"
        error = Self.oneLine(text.contains(chunk) ? text : "\(chunk): \(text)")
    }

    private static func oneLine(_ text: String) -> String {
        text.split(whereSeparator: \.isNewline).first.map(String.init) ?? text
    }

    private func render() throws {
        guard let runtime else { return }
        let tree = try runtime.view()
        var changes: [String: JSONValue] = [:]
        var data: [String: AnyCodable] = [:]
        var components: [AnyCodable] = []
        for case var .object(fields) in tree.a2uiComponents(rootID: "root") {
            guard case let .string(id)? = fields["id"], case let .string(type)? = fields["component"] else { continue }
            if type == "Button", let action = fields["action"], case .object = action {
                fields["action"] = .object(["event": .object(["name": .string("plugin.send"), "context": .object(["msg": action])])])
            }
            if type == "TextField" {
                let path = "/" + id
                data[path] = Self.any(fields["value"] ?? .string(""))
                fields["value"] = .object(["path": .string(path)])
                if let template = fields.removeValue(forKey: "on_change") { changes[path] = template }
            }
            components.append(Self.any(.object(fields)))
        }
        // Nothing above throws after this point, so a failed view leaves the surface and its handlers as they were.
        changeMsgs = changes
        surface.process([["updateComponents": .dictionary(["components": .array(components)])]])
        for (path, value) in data.sorted(by: { $0.key < $1.key }) {
            if let current = surface.get(path)?.stringValue, current == value.stringValue { continue }
            surface.process([["updateDataModel": .dictionary(["path": .string(path), "value": value])]])
        }
    }

    // MARK: for a check

    /// The script's state as JSON text, keys sorted.
    public var stateJSON: String {
        guard let runtime, let state = try? runtime.state,
              let data = try? JSONSerialization.data(withJSONObject: state.foundation, options: [.sortedKeys, .fragmentsAllowed])
        else { return "null" }
        return String(decoding: data, as: UTF8.self)
    }

    /// The drawn components in order, each as its id, type and text (a Text's text, a TextField's label and value).
    public var componentTexts: [[String: String]] {
        var out: [[String: String]] = []
        func walk(_ n: ComponentNode) {
            var row = ["id": n.instance.id, "type": n.instance.type]
            if let t = n.string("text") { row["text"] = t }
            if let l = n.string("label") { row["label"] = l }
            if n.instance.type == "TextField" { row["value"] = n.props["value"]?.text ?? "" }
            out.append(row)
            n.children.forEach(walk)
        }
        if let root = surface.root { walk(root) }
        return out
    }

    /// Presses the component as its button would; false when there is none.
    @discardableResult
    public func press(component id: String) -> Bool {
        guard let c = surface.components[id] else { return false }
        surface.activate(c)
        return true
    }

    /// Writes a text field's value as typing would.
    public func set(path: String, value: String) {
        surface.set(path, .string(value))
    }

    // MARK: JSON conversion

    static func any(_ v: JSONValue) -> AnyCodable {
        switch v {
        case .null: .null
        case let .bool(b): .bool(b)
        case let .number(n): .number(n)
        case let .string(s): .string(s)
        case let .array(a): .array(a.map(any))
        case let .object(o): .dictionary(o.mapValues(any))
        }
    }

    static func json(_ v: AnyCodable) -> JSONValue {
        switch v {
        case .null: .null
        case let .bool(b): .bool(b)
        case let .number(n): .number(n)
        case let .string(s): .string(s)
        case let .array(a): .array(a.map(json))
        case let .dictionary(o): .object(o.mapValues(json))
        }
    }
}

/// A plugin pane drawn like an A2UI pane: the surface, scrolling when it is taller than the pane. A failure
/// shows in the pane's header, not here.
public struct PluginPaneView: View {
    let pane: PluginPane
    @Environment(\.colorScheme) private var scheme

    public init(pane: PluginPane) { self.pane = pane }

    public var body: some View {
        let palette = WheelPalette(dark: scheme == .dark)
        GeometryReader { geo in
            ScrollView {
                A2UISurfaceView(surface: pane.surface)
                    .frame(maxWidth: .infinity, alignment: .top)
            }
            .environment(\.a2uiPane, geo.size)
            .overlay { A2UIPresentedModals(surface: pane.surface) }
        }
        .background(palette.bg)
        .accessibilityIdentifier("plugin.pane")
    }
}
