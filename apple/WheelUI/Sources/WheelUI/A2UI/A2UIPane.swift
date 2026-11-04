import A2UISwiftCore
import A2UISwiftUI
import Foundation
import Observation
import WheelKit

/// What a web pane on an MCP server artifact shows when that server speaks A2UI over MCP: the
/// layout resource its tool links, drawn as native controls on both clients. The model asks the
/// daemon for the tools (`artifact_tools`), reads the layout (`artifact_read`), calls the linked
/// tool for the first data (`artifact_call`) and answers a button's action with the tool the
/// action names. Every reply reaches `handle`, which the app model calls for each daemon message.
@MainActor @Observable
public final class A2UIPane {
    public enum State: Equatable {
        /// The tool list has been asked for; nothing says yet whether the server speaks A2UI.
        case probing
        /// The artifact is not an A2UI server (or the daemon cannot say): the web page stays.
        case web
        case loading
        case ready
        /// The server did not answer; the message is the daemon's. A surface already drawn stays.
        case failed(String)
    }

    public private(set) var state: State = .probing
    /// The last action's refusal or failure, shown above the surface.
    public private(set) var notice: String?
    /// The names of the components the layout asked for and the catalog does not hold.
    public private(set) var unsupported: [String] = []
    public private(set) var surface: SurfaceViewModel?

    /// Whether the pane shows this model in place of the artifact's web page: the server linked an
    /// A2UI layout, whether or not it has answered since.
    public var isNative: Bool {
        switch state {
        case .probing, .web: false
        case .loading, .ready, .failed: true
        }
    }

    public let pane: String
    private let artifact: ArtifactRef
    private let space: String?
    private let send: (ClientMsg) -> Void
    private let nextReq: () -> UInt64
    private var link: A2UIMessages.Link?
    private var pending: [UInt64: Step] = [:]
    private var subscription: Subscription?
    static let catalogNames = basicCatalog.componentNames.union(wheelA2UIExtensions)
    private var catalogNames: Set<String> { Self.catalogNames }

    private enum Step { case tools, layout, data, action(String) }

    public init(pane: String, artifact: ArtifactRef, space: String?, send: @escaping (ClientMsg) -> Void, nextReq: @escaping () -> UInt64) {
        self.pane = pane
        self.artifact = artifact
        self.space = space
        self.send = send
        self.nextReq = nextReq
    }

    /// The string at `path` of the surface's data model, for a check reading what the pane holds.
    public func text(at path: String) -> String? { surface?.surface.dataModel.get(path)?.stringValue }

    /// The state as a word, for a check.
    public var stateName: String {
        switch state {
        case .probing: "probing"
        case .web: "web"
        case .loading: "loading"
        case .ready: "ready"
        case .failed: "failed"
        }
    }

    /// Raise the action a button would, with string context, for a check that has no pointer.
    public func press(_ name: String, context: [String: String]) {
        surface?.surface.dispatchAction(name: name, sourceComponentId: "debug", context: context.mapValues { .string($0) })
    }

    /// Ask the daemon whether the artifact's server links an A2UI layout.
    public func start() {
        state = .probing
        let req = nextReq()
        pending[req] = .tools
        send(.artifactTools(req: req, name: artifact.name, scope: artifact.scope, space: space))
    }

    /// Take one daemon message; true when it answered a request of this pane.
    @discardableResult
    public func handle(_ msg: ServerMsg) -> Bool {
        switch msg {
        case let .tools(req, tools):
            guard case .tools? = pending.removeValue(forKey: req) else { return false }
            guard let link = A2UIMessages.link(in: tools) else { state = .web; return true }
            self.link = link
            state = .loading
            read(link)
        case let .resource(req, contents):
            guard case .layout? = pending.removeValue(forKey: req) else { return false }
            guard let text = A2UIMessages.text(ofResource: contents) else {
                state = .failed("the layout resource has no text")
                return true
            }
            layout(text)
        case let .toolResult(req, result):
            switch pending.removeValue(forKey: req) {
            case .data?: apply(result, action: nil)
            case let .action(name)?: apply(result, action: name)
            default: return false
            }
        case let .mcpUpdated(pane):
            // The server announced a change: ask the linked tool for the data again.
            guard pane == self.pane, let link, state == .ready else { return false }
            let req = nextReq()
            pending[req] = .data
            send(.artifactCall(req: req, name: artifact.name, scope: artifact.scope, space: space, tool: link.tool, arguments: nil))
        case let .error(req, _, message):
            guard let req, let step = pending.removeValue(forKey: req) else { return false }
            switch step {
            case .tools: state = .web
            case .layout, .data: state = .failed(message)
            case let .action(name): notice = "\(name): \(message)"
            }
        default: return false
        }
        return true
    }

    private func read(_ link: A2UIMessages.Link) {
        let req = nextReq()
        pending[req] = .layout
        send(.artifactRead(req: req, name: artifact.name, scope: artifact.scope, space: space, uri: link.uri))
    }

    private func layout(_ text: String) {
        do {
            let decoded = try A2UIMessages.decode(text: text, catalog: catalogNames)
            let catalog = Catalog(id: "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
                                  componentNames: catalogNames, functions: basicCatalog.functions)
            let model = SurfaceViewModel(catalog: catalog)
            let errors = model.processMessages(decoded.messages)
            if !errors.isEmpty { notice = errors.map { "\($0)" }.joined(separator: "; ") }
            unsupported = decoded.unsupported
            subscription = model.onAction { [weak self] action in
                MainActor.assumeIsolated { self?.act(action) }
            }
            surface = model
        } catch {
            state = .failed("\(error)")
            return
        }
        guard let link else { return }
        let req = nextReq()
        pending[req] = .data
        send(.artifactCall(req: req, name: artifact.name, scope: artifact.scope, space: space, tool: link.tool, arguments: nil))
    }

    /// A button's action calls the tool the action's event names, its context as the arguments.
    private func act(_ action: A2uiClientAction) {
        notice = nil
        let req = nextReq()
        pending[req] = .action(action.name)
        send(.artifactCall(req: req, name: artifact.name, scope: artifact.scope, space: space, tool: action.name,
                           arguments: A2UIMessages.arguments(action)))
    }

    private func apply(_ result: JSONValue, action: String?) {
        if let why = A2UIMessages.failure(ofResult: result) {
            if let action { notice = "\(action): \(why)" } else { state = .failed(why) }
            return
        }
        guard let surface else { return }
        for text in A2UIMessages.texts(ofResult: result) {
            guard let decoded = try? A2UIMessages.decode(text: text, catalog: catalogNames) else { continue }
            unsupported += decoded.unsupported
            let errors = surface.processMessages(decoded.messages)
            if !errors.isEmpty { notice = errors.map { "\($0)" }.joined(separator: "; ") }
        }
        state = .ready
    }
}
