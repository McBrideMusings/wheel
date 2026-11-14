import Foundation
import WheelKit

/// A plugin pane on a client that talks to wheeld in `ClientMsg` and `ServerMsg` (the iPhone app): the `PluginPane`
/// and the requests that feed it. It asks for the script (`pluginSource`), subscribes the feeds the script reads, sends
/// its `wheel.action`s, and takes each reply and push in `handle`, which the app model calls for every wheeld message.
/// A script saved on the Mac arrives as an unasked `pluginSource` and reloads the pane with its state kept.
@MainActor
public final class PluginPaneLink {
    public let pane: String
    public let plugin = PluginPane()
    private let send: (ClientMsg) -> Void
    private let nextReq: () -> UInt64
    private enum Step {
        case source
        case feed(String, JSONValue)
        case unsubscribe
        case action(String)
    }
    private var pending: [UInt64: Step] = [:]
    /// False between `dropped` and the next `start`: a `wheel.action` then fails at once instead of waiting on a reply
    /// that cannot come.
    private var live = true

    /// `context` is what the script's `view` gets beside its feeds: `pane`, `space`, `cwd`, `plugin` and `plugin_pane`.
    public init(pane: String, context: [String: JSONValue], send: @escaping (ClientMsg) -> Void, nextReq: @escaping () -> UInt64) {
        self.pane = pane
        self.send = send
        self.nextReq = nextReq
        plugin.context = context
        plugin.onFeeds = { [weak self] subscribe, unsubscribe in
            guard let self else { return }
            for (feed, params) in unsubscribe {
                let req = self.nextReq()
                self.pending[req] = .unsubscribe
                self.send(.feedUnsubscribe(req: req, pane: self.pane, feed: feed, params: params))
            }
            for (feed, params) in subscribe { self.subscribe(feed, params) }
        }
        plugin.onAction = { [weak self] action, args in
            guard let self else { return }
            guard self.live else {
                // The script is inside `update` or `view` now; its answer comes as the next message, as a reply would.
                Task { @MainActor [weak self] in self?.plugin.actionFailed(action: action, message: "no connection to wheeld") }
                return
            }
            let req = self.nextReq()
            self.pending[req] = .action(action)
            self.send(.pluginAction(req: req, pane: self.pane, action: action, args: args))
        }
    }

    /// Ask for the script, and subscribe again every feed the pane reads. Called when the pane is made and again on each
    /// new connection to wheeld, whose subscriptions start empty.
    public func start() {
        live = true
        let req = nextReq()
        pending[req] = .source
        send(.pluginSource(req: req, pane: pane))
        for (feed, params) in plugin.feeds { subscribe(feed, params) }
    }

    /// The connection to wheeld dropped: every request waiting on it is lost. The pane keeps its last view and says so
    /// until `start` on the next connection loads the script again.
    public func dropped() {
        live = false
        let lost = pending
        pending = [:]
        for case let .action(name) in lost.values { plugin.actionFailed(action: name, message: "lost the connection to wheeld") }
        plugin.fail(message: "lost the connection to wheeld, retrying")
    }

    public func close() {
        plugin.close()
        pending = [:]
    }

    private func subscribe(_ feed: String, _ params: JSONValue) {
        let req = nextReq()
        pending[req] = .feed(feed, params)
        send(.feedSubscribe(req: req, pane: pane, feed: feed, params: params))
    }

    /// Take one wheeld message; true when it was for this pane.
    @discardableResult
    public func handle(_ msg: ServerMsg) -> Bool {
        switch msg {
        case let .pluginSource(req, pane, package, packagePane, chunk, source, modules):
            guard pane == self.pane else { return false }
            if let req { pending[req] = nil }
            // Sent for what the pane showed before it came to run this plugin pane.
            guard plugin.context["plugin"] == .string(package), plugin.context["plugin_pane"] == .string(packagePane) else { return true }
            plugin.load(source: source, modules: modules, chunk: chunk)
        case let .feedUpdate(req, pane, feed, params, value, patch, stale):
            guard pane == self.pane else { return false }
            if let req { pending[req] = nil }
            plugin.feedUpdate(feed: feed, params: params.foundation, value: value?.foundation, patch: patch?.foundation, stale: stale)
        case let .pluginAction(req, pane, action, result):
            guard pane == self.pane, case .action? = pending.removeValue(forKey: req) else { return false }
            plugin.actionResult(action: action, result: result.foundation)
        case let .ok(req):
            guard pending.removeValue(forKey: req) != nil else { return false }
        case let .error(req, _, message):
            guard let req, let step = pending.removeValue(forKey: req) else { return false }
            switch step {
            case .source: plugin.fail(message: message)
            case let .feed(feed, params): plugin.feedFailed(feed: feed, params: params.foundation, message: message)
            case let .action(name): plugin.actionFailed(action: name, message: message)
            case .unsubscribe: break
            }
        default: return false
        }
        return true
    }
}
