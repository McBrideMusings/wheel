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
    /// The feeds the render in progress has read.
    @ObservationIgnored private var asked: [FeedKey: JSONValue] = [:]

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
                runtime = try PaneRuntime(source: source, chunkName: chunk, host: PaneRuntime.Host(feed: { [weak self] name, params in
                    self?.read(feed: name, params: params) ?? .null
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
    /// The arguments are what JSONSerialization returns. A patch that does not apply sets `error` and keeps the last value.
    public func feedUpdate(feed: String, params: Any?, value: Any?, patch: Any?) {
        let params = Self.jsonValue(params ?? NSNull())
        let key = FeedKey(name: feed, params: Self.canonical(params))
        guard subscribed[key] != nil else { return }
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

    private func read(feed name: String, params given: JSONValue) -> JSONValue {
        // Params are an object; a Luau `{}` reaches here as an empty array.
        let params: JSONValue = given == .array([]) ? .object([:]) : given
        let key = FeedKey(name: name, params: Self.canonical(params))
        asked[key] = params
        return feedValues[key] ?? .null
    }

    /// After a good render: subscribes the feeds it read that are new and unsubscribes those it no longer reads.
    private func syncFeeds() {
        let subscribe = asked.filter { subscribed[$0.key] == nil }
        let unsubscribe = subscribed.filter { asked[$0.key] == nil }
        guard !subscribe.isEmpty || !unsubscribe.isEmpty else { return }
        for key in unsubscribe.keys { feedValues[key] = nil }
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
        syncFeeds()
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
