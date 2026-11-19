import Foundation
import Observation
import SwiftUI
import WheelKit
internal import WheelLuau

/// A plugin pane's body (ADR-0041): the Luau script's `view` reconciled into a `ViewTree` after every send or reload, so only
/// the components whose properties changed are drawn again. Every table the script puts where a component raises one (a
/// Button's `action`, a Table's `rowAction`, a row's `menu[].action`, a Canvas hit's `action` and `doubleAction`, a Modal's
/// `dismissAction`) is sent to the script's `update` when the component raises it, with the component's own fields for the
/// event (a row's `id`, a sort's `key` and `asc`, a hit's `id`) added where the table has no such key. A control with an
/// `on_change` table (TextField, CheckBox, ChoicePicker, Slider, Stepper, DateTimeInput, ...) sends it with `value` set to
/// what the control wrote. `view` gets `context` (the pane's ids and directory) as its `data`.
/// A failure keeps the last good view and sets `error`; a good load clears it. A view with a duplicate `key` among siblings
/// is drawn with the first of them and also sets `error`. A view that binds a `ui.state`, `ui.computed` or `ui.each` is
/// mounted: `view` runs once, and after each message, feed change or canvas size the runtime's flush writes only the props and
/// child lists whose values changed.
@MainActor @Observable
public final class PluginPane {
    public let tree = ViewTree()
    /// The last failure as one readable line, or nil.
    public private(set) var error: String?
    /// Reloads that took the new source, not counting the first load.
    public private(set) var reloads = 0
    /// The script's file name as wheeld sent it, in error messages.
    public private(set) var chunk = "pane"

    /// The scroll the host last asked for; the pane's ScrollView applies each new `id` once.
    public private(set) var scrollRequest = ScrollRequest(id: 0, to: .by(0))
    /// Where the pane's ScrollView stands as last reported; stale once the view fills the pane, so read it with `fillsPane`.
    public internal(set) var scrollMetrics: ScrollMetrics?

    /// A scroll asked from outside the pointer: to an edge, or by a distance in points (positive is down).
    public struct ScrollRequest: Equatable, Sendable {
        public enum Target: Equatable, Sendable { case top, bottom, by(Double) }
        public let id: Int
        public let to: Target
    }

    /// A ScrollView's offset, content height and visible height, in points.
    public struct ScrollMetrics: Equatable, Sendable {
        public var offset: Double
        public var content: Double
        public var visible: Double
    }

    /// Whether the view takes the pane's height itself (a frame-less Table or vertical List), so the pane has no ScrollView.
    public var fillsPane: Bool { tree.fillsPane }

    /// Asks the pane's ScrollView to move; a view that fills the pane has none and ignores it.
    public func scroll(_ to: ScrollRequest.Target) {
        scrollRequest = ScrollRequest(id: scrollRequest.id + 1, to: to)
    }

    /// What the last render cost, for a check: Luau `update` and `view` in ms, the reconcile into the tree in ms, the nodes
    /// it created or wrote, and how many renders ran since the pane started.
    public struct Perf: Equatable, Sendable {
        public var updateMs = 0.0
        public var viewMs = 0.0
        public var reconcileMs = 0.0
        public var nodes = 0
        public var renders = 0
        /// Flushes of a mounted view: each runs the effects that went stale and applies their writes, with no `view`.
        public var flushes = 0
    }
    @ObservationIgnored public private(set) var perf = Perf()

    @ObservationIgnored private var runtime: PaneRuntime?
    /// Whether `error` came from loading the script: the file is still broken, so a message the script handles does not clear it.
    @ObservationIgnored private var loadFailed = false
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
        tree.onRaise = { [weak self] msg in self?.deliver(msg) }
        tree.onCanvasSize = { [weak self] key, w, h in self?.canvasSize(key: key, width: w, height: h) }
    }

    /// A command Canvas measured itself: the runtime keeps the size for `draw`, and the view is drawn again only when it changed.
    /// The report never reaches `update`.
    private func canvasSize(key: String, width w: Double, height h: Double) {
        guard let runtime, runtime.setCanvasSize(key: key, width: w, height: h) else { return }
        do {
            try refresh(changed: "size:" + key)
            if !loadFailed { error = nil }
        } catch {
            fail(error)
        }
        onUpdate?()
    }

    /// Ends the running script and its state (`pane_reload`): the next `load` runs the script from the top, as the
    /// first load did. The last view stays drawn until then, and the feeds stay subscribed.
    public func restart() {
        runtime = nil
        loadFailed = false
        tree.forgetGiven()
        restarts += 1
    }
    /// Times `restart` ended the script.
    public private(set) var restarts = 0

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
                // Take the value out of the dictionary so the patch edits its only reference in place.
                // A failing patch leaves `doc` unchanged, so it goes back as it was.
                let existed = feedValues[key] != nil
                var doc = feedValues.removeValue(forKey: key) ?? .null
                do { try JSONPatch.apply(Self.jsonValue(patch), to: &doc) } catch {
                    if existed { feedValues[key] = doc }
                    throw error
                }
                feedValues[key] = doc
            } else if let value {
                feedValues[key] = Self.jsonValue(value)
            } else {
                feedValues[key] = .null
            }
            guard runtime != nil else { return }
            try refresh(changed: "feed:" + Self.feedKey(key))
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

    /// An action the host could not send at all (no connection, arguments that do not encode). The
    /// script calls `wheel.action` from inside `update`, so the failure is delivered on the next
    /// main-queue turn, never before that `update` has returned.
    public func actionFailedLater(action: String, message: String) {
        Task { @MainActor [weak self] in self?.actionFailed(action: action, message: message) }
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
        return PaneRuntime.FeedRead(value: feedValues[key] ?? .null, stale: staleFeeds.contains(key), key: Self.feedKey(key))
    }

    /// A feed's name and params as the runtime names it: `<name> <params as canonical JSON>`.
    private static func feedKey(_ key: FeedKey) -> String { key.name + " " + key.params }

    /// A mounted view keeps the feeds its live computations read (`live`, by `feedKey`): those are subscribed, the rest dropped.
    private func syncLive(_ live: [String]) {
        let keep = Set(live)
        asked = asked.filter { keep.contains(Self.feedKey($0.key)) }
        syncFeeds()
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
            let start = ContinuousClock.now
            try runtime.send(msg)
            perf.updateMs = Self.ms(since: start)
            try refresh()
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

    /// A key the view gave two siblings: the first is drawn, the rest are left out.
    struct DuplicateKey: Error, CustomStringConvertible {
        let at: String
        var description: String { "view: duplicate key \(at)" }
    }

    /// After a message, a feed change (`feed:<key>`) or a canvas size (`size:<key>`): a mounted view flushes the writes its
    /// stale effects make; any other view renders whole.
    private func refresh(changed source: String? = nil) throws {
        guard let runtime else { return }
        guard runtime.mounted else { return try render() }
        let start = ContinuousClock.now
        let changes = try runtime.flush(changed: source)
        perf.viewMs = 0
        tree.apply(changes.ops)
        perf.reconcileMs = Self.ms(since: start)
        perf.nodes = tree.written
        perf.flushes += 1
        syncLive(changes.feeds)
        if let at = tree.duplicate { throw DuplicateKey(at: at) }
    }

    private func render() throws {
        guard let runtime else { return }
        asked = [:]
        let viewStart = ContinuousClock.now
        let view = try runtime.view(.object(context))
        perf.viewMs = Self.ms(since: viewStart)
        if runtime.mounted { syncLive(runtime.feeds) } else { syncFeeds() }
        let start = ContinuousClock.now
        tree.reconcile(view)
        perf.reconcileMs = Self.ms(since: start)
        perf.nodes = tree.written
        perf.renders += 1
        if let at = tree.duplicate { throw DuplicateKey(at: at) }
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
    /// A Modal's `actions` (its footer buttons) follow its content, a Button carries `disabled` when a modifier disables it,
    /// and a Table adds one `TableRow` entry per row: its id (the row's `path`, else its `id`, else `<section>.<row>`), its node id under
    /// `node`, its table, its section's title and its cells' texts in
    /// column order joined by ` | ` under `cells`.
    public var componentTexts: [[String: String]] {
        var out: [[String: String]] = []
        func walk(_ n: ViewNode, _ depth: Int = 0) {
            guard depth < 64 else { return }
            var row = ["id": n.id, "type": n.component]
            if let t = n.string("text") { row["text"] = t }
            if let l = n.string("label") { row["label"] = l }
            if n.component == "TextField" { row["value"] = n.value?.text ?? "" }
            if n.component == "Button" {
                let mods = (n.props["modifiers"]?.arrayValue ?? []).compactMap(\.dictionaryValue)
                if mods.contains(where: { $0["disabled"]?.boolValue == true }) { row["disabled"] = "true" }
            }
            out.append(row)
            if n.component == "Table" { out += Self.tableRows(n) }
            n.parts.forEach { walk($0, depth + 1) }
            for i in (n.props["actions"]?.arrayValue ?? []).indices {
                if let a = n.embedded["actions.\(i)"] { walk(a, depth + 1) }
            }
        }
        if let root = tree.root { walk(root) }
        return out
    }

    private static func tableRows(_ table: ViewNode) -> [[String: String]] {
        let keys = (table.props["columns"]?.arrayValue ?? []).compactMap { $0.dictionaryValue?["key"]?.stringValue }
        let byId = Dictionary(table.rows.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        var out: [[String: String]] = []
        for sec in (table.props["sections"]?.arrayValue ?? []).compactMap(\.dictionaryValue) {
            for r in (sec["rows"]?.arrayValue ?? []).compactMap({ $0.stringValue.flatMap { byId[$0] } }) {
                let cells = (r.props["cells"]?.arrayValue ?? []).compactMap(\.dictionaryValue)
                let text = keys.map { k in cells.first { $0["key"]?.stringValue == k }?["text"]?.text ?? "" }
                var row = ["id": String(r.id.dropFirst(table.id.count + 1)), "node": r.id, "type": "TableRow", "table": table.id, "cells": text.joined(separator: " | ")]
                if let t = sec["title"]?.stringValue, !t.isEmpty { row["section"] = t }
                out.append(row)
            }
        }
        return out
    }

    /// Presses the component as its button would; false when the view has no node `id`.
    @discardableResult
    public func press(component id: String) -> Bool {
        guard let node = tree.node(id) else { return false }
        tree.activate(node)
        return true
    }

    /// Writes a control's value as the control would; `value` is what JSONSerialization returns. False when the view has no
    /// node `id`.
    @discardableResult
    public func set(component id: String, value: Any) -> Bool {
        guard let node = tree.node(id) else { return false }
        tree.change(node, Self.jsonValue(value))
        return true
    }

    /// Raises the action the script put at `key` of node `id` (`rowAction`, `hits.0.doubleAction`, a row's `menu.1.action`),
    /// with the event's own fields in `context`, as the component does; false when the last render put no action there.
    @discardableResult
    public func raise(component id: String, key: String, context: Any?) -> Bool {
        guard let node = tree.node(id) else { return false }
        var fields: [String: JSONValue] = [:]
        if case let .object(given) = Self.jsonValue(context ?? NSNull()) { fields = given }
        return tree.raise(node, key, fields)
    }

    private static func ms(since start: ContinuousClock.Instant) -> Double {
        let d = ContinuousClock.now - start
        return Double(d.components.seconds) * 1000 + Double(d.components.attoseconds) / 1e15
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
}

/// A plugin pane's view, scrolling when it is taller than the pane. A failure shows in the pane's header, not here.
public struct PluginPaneView: View {
    let pane: PluginPane
    @Environment(\.colorScheme) private var scheme
    @State private var position = ScrollPosition()

    public init(pane: PluginPane) { self.pane = pane }

    public var body: some View {
        let palette = WheelPalette(dark: scheme == .dark)
        GeometryReader { geo in
            Group {
                // A frame-less Table or vertical List takes the pane's height and scrolls itself; any other view scrolls as a whole.
                if pane.tree.fillsPane {
                    A2UISurfaceView(tree: pane.tree)
                        .frame(width: geo.size.width, height: geo.size.height, alignment: .top)
                } else {
                    ScrollView {
                        A2UISurfaceView(tree: pane.tree)
                            .frame(maxWidth: .infinity, alignment: .top)
                    }
                    .scrollPosition($position)
                    .onScrollGeometryChange(for: PluginPane.ScrollMetrics.self) { g in
                        PluginPane.ScrollMetrics(offset: g.contentOffset.y, content: g.contentSize.height, visible: g.visibleRect.height)
                    } action: { _, metrics in
                        pane.scrollMetrics = metrics
                    }
                    .onChange(of: pane.scrollRequest) { _, request in
                        switch request.to {
                        case .top: position.scrollTo(edge: .top)
                        case .bottom: position.scrollTo(edge: .bottom)
                        case let .by(dy): position.scrollTo(y: max(0, (pane.scrollMetrics?.offset ?? 0) + dy))
                        }
                    }
                }
            }
            .environment(\.a2uiPane, geo.size)
            .environment(\.a2uiCanvasSizes, true)
            .overlay { A2UIPresentedModals(tree: pane.tree) }
        }
        .background(palette.bg)
        .accessibilityIdentifier("plugin.pane")
    }
}
