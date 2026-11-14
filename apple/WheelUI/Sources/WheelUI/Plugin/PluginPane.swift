import Foundation
import Observation
import SwiftUI
import WheelKit
internal import WheelLuau

/// A plugin pane's body (ADR-0041): the Luau script's `view` drawn as A2UI components. The pane owns the
/// script's `PaneRuntime` and a `SurfaceModel`. Every table the script puts under a key named `action` or ending in
/// `Action`, at any depth of a component's properties (a Button's `action`, a Table's `rowAction`, a row's `menu[].action`, a
/// Canvas hit's `action` and `doubleAction`, a Modal's `dismissAction`), is sent to the script's `update` when the
/// component raises it, with the component's own fields for the event (a row's `id`, a sort's `key` and `asc`, a hit's
/// `id`) added where the table has no such key: a pane script never speaks A2UI's action shape. A control with an
/// `on_change` table (TextField, CheckBox, ChoicePicker, Slider, Stepper, DateTimeInput, ...) sends it with `value` set to
/// what the control wrote. `view` gets `context` (the pane's ids and directory) as its `data`.
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
    /// For each control's data path, the table its `on_change` names.
    @ObservationIgnored private var changeMsgs: [String: JSONValue] = [:]
    /// For each action name the last render gave a component, the table the script wrote there.
    @ObservationIgnored private var actionMsgs: [String: JSONValue] = [:]
    /// For each bound control's data path, the `value` the script gave at the last render.
    @ObservationIgnored private var lastGiven: [String: AnyCodable] = [:]
    /// The fields `view` gets in `data` besides `feed`: the host sets `pane`, `space`, `cwd`, `plugin` and `plugin_pane`.
    @ObservationIgnored public var context: [String: JSONValue] = [:]
    /// Called after every load, send and failure, for a host that reports `error`.
    @ObservationIgnored public var onUpdate: (() -> Void)?
    /// Called after a render changed which feeds the script reads, with the (feed, params) pairs to subscribe and to
    /// unsubscribe; the host sends them to wheeld.
    @ObservationIgnored public var onFeeds: ((_ subscribe: [(String, JSONValue)], _ unsubscribe: [(String, JSONValue)]) -> Void)?

    /// Called when the script calls `wheel.action(name, args)`; the host sends it to wheeld and later answers with
    /// `actionResult` or `actionFailed`. It only forwards: the script may call it from inside `update` or `view`.
    @ObservationIgnored public var onAction: ((_ action: String, _ args: JSONValue) -> Void)?

    /// A feed read by name and params; params are compared as canonical JSON text (keys sorted).
    private struct FeedKey: Hashable {
        let name: String
        let params: String
    }
    /// The feeds subscribed with wheeld, each with its params as the script gave them.
    @ObservationIgnored private var subscribed: [FeedKey: JSONValue] = [:]
    /// The value held for each subscribed feed.
    @ObservationIgnored private var feedValues: [FeedKey: JSONValue] = [:]
    /// The subscribed feeds whose backend is down: their held value is the last one it sent.
    @ObservationIgnored private var staleFeeds: Set<FeedKey> = []
    /// The feeds the render in progress has read.
    @ObservationIgnored private var asked: [FeedKey: JSONValue] = [:]

    public init() {
        surface.onAction = { [weak self] action in
            guard let self else { return }
            if action.name.hasPrefix(canvasSizePrefix) {
                self.canvasSize(key: String(action.name.dropFirst(canvasSizePrefix.count)), context: action.context)
                return
            }
            guard let template = self.actionMsgs[action.name] else { return }
            var msg: [String: JSONValue] = [:]
            if case let .object(fields) = template { msg = fields }
            for (key, value) in action.context where msg[key] == nil { msg[key] = Self.json(value) }
            self.deliver(.object(msg))
        }
        surface.onChange = { [weak self] change in
            guard let self, let template = self.changeMsgs[change.path] else { return }
            var msg: [String: JSONValue] = [:]
            if case let .object(fields) = template { msg = fields }
            msg["value"] = Self.json(change.value)
            self.deliver(.object(msg))
        }
    }

    /// A command Canvas measured itself: the runtime keeps the size for `draw`, and the view is drawn again only when it changed.
    /// The report never reaches `update`.
    private func canvasSize(key: String, context: [String: AnyCodable]) {
        guard let runtime, let w = context["w"]?.numberValue, let h = context["h"]?.numberValue,
              runtime.setCanvasSize(key: key, width: w, height: h) else { return }
        do {
            try render()
            if !loadFailed { error = nil }
        } catch {
            fail(error)
        }
        onUpdate?()
    }

    /// Takes the script's source and the package's other `.luau` files (path in the package to source) for its
    /// `require`: the first call starts the runtime, a later one reloads it with its state kept.
    public func load(source: String, modules: [String: String] = [:], chunk: String) {
        self.chunk = chunk
        do {
            if let runtime {
                try runtime.reload(source: source, modules: modules)
                reloads += 1
            } else {
                runtime = try PaneRuntime(source: source, modules: modules, chunkName: chunk, host: PaneRuntime.Host(feed: { [weak self] name, params in
                    self?.read(feed: name, params: params) ?? PaneRuntime.FeedRead(value: .null)
                }, action: { [weak self] name, args in
                    self?.onAction?(name, args)
                }))
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

    /// Takes a change to a feed the pane reads: the whole `value`, or an RFC 6902 `patch` applied to the value held.
    /// `stale` says the feed's backend is down and `value` is the last one it sent; an update without it is fresh.
    /// The arguments are what JSONSerialization returns. A patch that does not apply sets `error` and keeps the last value.
    public func feedUpdate(feed: String, params: Any?, value: Any?, patch: Any?, stale: Bool = false) {
        let params = Self.jsonValue(params ?? NSNull())
        let key = FeedKey(name: feed, params: Self.canonical(params))
        guard subscribed[key] != nil else { return }
        if stale { staleFeeds.insert(key) } else { staleFeeds.remove(key) }
        do {
            if let patch {
                feedValues[key] = try JSONPatch.apply(Self.jsonValue(patch), to: feedValues[key] ?? .null)
            } else if let value {
                feedValues[key] = Self.jsonValue(value)
            } else {
                feedValues[key] = .null
            }
            guard runtime != nil else { return }
            try render()
            if !loadFailed { error = nil }
        } catch {
            // The held value no longer matches wheeld's: drop it and read the feed again, so wheeld sends it whole.
            feedValues[key] = nil
            if let held = subscribed[key] { onFeeds?([(feed, held)], [(feed, held)]) }
            fail(message: "feed \(feed): \(error)")
            return
        }
        onUpdate?()
    }

    /// wheeld refused a `feed_subscribe`: the feed is not held, so the next render subscribes it again, and `message` is shown.
    public func feedFailed(feed: String, params: Any?, message: String) {
        let key = FeedKey(name: feed, params: Self.canonical(Self.jsonValue(params ?? NSNull())))
        subscribed[key] = nil
        feedValues[key] = nil
        staleFeeds.remove(key)
        fail(message: message)
    }

    /// wheeld answered an action the script called: `update` gets `{action = <name>, result = <value>}`.
    /// `result` is what JSONSerialization returns.
    public func actionResult(action: String, result: Any?) {
        deliver(.object(["action": .string(action), "result": Self.jsonValue(result ?? NSNull())]))
    }

    /// wheeld refused an action or its backend failed: `update` gets `{action = <name>, error = <message>}`.
    public func actionFailed(action: String, message: String) {
        deliver(.object(["action": .string(action), "error": .string(message)]))
    }

    /// The pane is going away: unsubscribes every feed it holds.
    public func close() {
        let unsubscribe = subscribed.map { ($0.key.name, $0.value) }
        subscribed = [:]
        feedValues = [:]
        staleFeeds = []
        if !unsubscribe.isEmpty { onFeeds?([], unsubscribe) }
    }

    /// The feeds the pane reads now, as (feed, params), for a host that must subscribe them again.
    public var feeds: [(String, JSONValue)] {
        subscribed.map { ($0.key.name, $0.value) }.sorted { $0.0 < $1.0 }
    }

    /// The held value of each subscribed feed as JSON text, keyed `feed params`, for a check.
    public var feedJSON: [String: String] {
        var out: [String: String] = [:]
        for key in subscribed.keys {
            out[key.name + " " + key.params] = Self.canonical(feedValues[key] ?? .null)
        }
        return out
    }

    private func read(feed name: String, params given: JSONValue) -> PaneRuntime.FeedRead {
        // Params are an object; a Luau `{}` reaches here as an empty array.
        let params: JSONValue = given == .array([]) ? .object([:]) : given
        let key = FeedKey(name: name, params: Self.canonical(params))
        asked[key] = params
        return PaneRuntime.FeedRead(value: feedValues[key] ?? .null, stale: staleFeeds.contains(key))
    }

    /// After a good render: subscribes the feeds it read that are new and unsubscribes those it no longer reads.
    private func syncFeeds() {
        let subscribe = asked.filter { subscribed[$0.key] == nil }
        let unsubscribe = subscribed.filter { asked[$0.key] == nil }
        guard !subscribe.isEmpty || !unsubscribe.isEmpty else { return }
        for key in unsubscribe.keys {
            feedValues[key] = nil
            staleFeeds.remove(key)
        }
        subscribed = asked
        onFeeds?(subscribe.map { ($0.key.name, $0.value) }.sorted { $0.0 < $1.0 },
                 unsubscribe.map { ($0.key.name, $0.value) }.sorted { $0.0 < $1.0 })
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
        asked = [:]
        let tree = try runtime.view(.object(context))
        var changes: [String: JSONValue] = [:]
        var actions: [String: JSONValue] = [:]
        var data: [String: AnyCodable] = [:]
        var values: [String: AnyCodable] = [:]
        var components: [AnyCodable] = []
        for case var .object(fields) in tree.a2uiComponents(rootID: "root") {
            guard case let .string(id)? = fields["id"], case let .string(type)? = fields["component"] else { continue }
            let template = fields.removeValue(forKey: "on_change")
            Self.name(&fields, id: id, type: type, into: &actions)
            if type == "TextField" || template != nil {
                // The data model holds what the control shows: the script's `value` is written only when it differs from the
                // one the script gave last render, so a script that does not echo an edit back never undoes it.
                let path = "/" + id
                let given = fields["value"].map(Self.any)
                if let given, given != lastGiven[path] { data[path] = given }
                else if given == nil, lastGiven[path] == nil, type == "TextField" { data[path] = .string("") }
                given.map { values[path] = $0 }
                fields["value"] = .object(["path": .string(path)])
                if let template { changes[path] = template }
            }
            components.append(Self.any(.object(fields)))
        }
        syncFeeds()
        // Nothing above throws after this point, so a failed view leaves the surface and its handlers as they were.
        changeMsgs = changes
        actionMsgs = actions
        lastGiven = values
        surface.process([["updateComponents": .dictionary(["components": .array(components)])]])
        for (path, value) in data.sorted(by: { $0.key < $1.key }) {
            if surface.get(path) == value { continue }
            surface.process([["updateDataModel": .dictionary(["path": .string(path), "value": value])]])
        }
    }

    /// Replaces each table the script put where a component raises an action with a name unique to its place
    /// (`plugin:<id>/<key path>`), recorded in `names` with the table: a property named `action` or ending in `Action`,
    /// a Canvas hit's `action` and `doubleAction` (`hits`), a dialog button's `action` (`buttons`) and a Table row's
    /// `menu[].action` (`sections[].rows[].menu`). A Button raises its name as A2UI's event; a Button `action`
    /// `{copy = {text = ...}}` stays the clipboard copy it is in A2UI.
    private static func name(_ fields: inout [String: JSONValue], id: String, type: String, into names: inout [String: JSONValue]) {
        func take(_ value: JSONValue, _ path: String) -> JSONValue {
            guard case .object = value else { return value }
            names[path] = value
            return .string(path)
        }
        func each(_ value: JSONValue?, _ body: (JSONValue, Int) -> JSONValue) -> JSONValue? {
            guard case let .array(items)? = value else { return value }
            return .array(items.enumerated().map { body($0.element, $0.offset) })
        }
        func setting(_ value: JSONValue, _ keys: [String], _ path: String) -> JSONValue {
            guard case var .object(o) = value else { return value }
            for k in keys { if let v = o[k] { o[k] = take(v, "\(path).\(k)") } }
            return .object(o)
        }
        let base = "plugin:\(id)/"
        for key in fields.keys where key == "action" || key.hasSuffix("Action") {
            guard let value = fields[key] else { continue }
            if type == "Button", key == "action", case let .object(o) = value, o["copy"] != nil { continue }
            fields[key] = take(value, base + key)
            if type == "Button", key == "action", case let .string(n)? = fields[key], names[n] != nil {
                fields[key] = .object(["event": .object(["name": .string(n)])])
            }
        }
        fields["hits"] = each(fields["hits"]) { setting($0, ["action", "doubleAction"], base + "hits.\($1)") }
        fields["buttons"] = each(fields["buttons"]) { setting($0, ["action"], base + "buttons.\($1)") }
        fields["sections"] = each(fields["sections"]) { sec, si in
            guard case var .object(so) = sec else { return sec }
            so["rows"] = each(so["rows"]) { row, ri in
                guard case var .object(ro) = row else { return row }
                ro["menu"] = each(ro["menu"]) { setting($0, ["action"], base + "sections.\(si).rows.\(ri).menu.\($1)") }
                return .object(ro)
            }
            return .object(so)
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

    /// Writes a control's value as the control would; `value` is what JSONSerialization returns.
    public func set(path: String, value: Any) {
        surface.set(path, Self.any(Self.jsonValue(value)))
    }

    /// Raises the action the script put at `key` of component `id` (`rowAction`, `hits.0.doubleAction`), with the
    /// event's own fields in `context`, as the component does; false when the last render put no action there.
    @discardableResult
    public func raise(component id: String, key: String, context: Any?) -> Bool {
        let name = "plugin:\(id)/\(key)"
        guard surface.components[id] != nil, actionMsgs[name] != nil else { return false }
        var fields: [String: JSONValue] = [:]
        if case let .object(given) = Self.jsonValue(context ?? NSNull()) { fields = given }
        surface.dispatchAction(name: name, sourceComponentId: id, context: fields.mapValues(Self.any))
        return true
    }

    // MARK: JSON conversion

    /// A JSONSerialization object as a JSONValue.
    static func jsonValue(_ any: Any) -> JSONValue {
        switch any {
        case is NSNull: return .null
        case let n as NSNumber:
            return CFGetTypeID(n) == CFBooleanGetTypeID() ? .bool(n.boolValue) : .number(n.doubleValue)
        case let s as String: return .string(s)
        case let a as [Any]: return .array(a.map(jsonValue))
        case let o as [String: Any]: return .object(o.mapValues(jsonValue))
        default: return .null
        }
    }

    /// JSON text with sorted keys, so equal values give equal text.
    static func canonical(_ v: JSONValue) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: v.foundation, options: [.sortedKeys, .fragmentsAllowed]) else { return "null" }
        return String(decoding: data, as: UTF8.self)
    }

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
            Group {
                // The same decision as A2UIPaneView: a frame-less Table or vertical List takes the pane's height and
                // scrolls itself; any other surface scrolls as a whole.
                if pane.surface.fillsPane {
                    A2UISurfaceView(surface: pane.surface)
                        .frame(width: geo.size.width, height: geo.size.height, alignment: .top)
                } else {
                    ScrollView {
                        A2UISurfaceView(surface: pane.surface)
                            .frame(maxWidth: .infinity, alignment: .top)
                    }
                }
            }
            .environment(\.a2uiPane, geo.size)
            .environment(\.a2uiCanvasSizes, true)
            .overlay { A2UIPresentedModals(surface: pane.surface) }
        }
        .background(palette.bg)
        .accessibilityIdentifier("plugin.pane")
    }
}
