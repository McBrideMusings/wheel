import SwiftUI
import WebKit
import WheelKit
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// The artifacts plugin pages embed (`WebView{artifact}`, ADR-0046) on one client: each asks wheeld for its page with
/// `plugin_embed` and shows `/artifacts/<id>/` under the client's page scheme, exactly as a web pane's page. A host makes
/// one, hands it every wheeld message (`handle`, for the replies and `pane_reload`) and calls `connected` on each new
/// connection, since a restarted wheeld knows none of the old page ids.
@MainActor
public final class ArtifactEmbeds {
    private let send: (ClientMsg) -> Void
    private let nextReq: () -> UInt64
    let daemon: PageDaemon
    private var waiting: [UInt64: ArtifactEmbed] = [:]
    private var live: [ObjectIdentifier: ArtifactEmbed] = [:]

    public init(daemon: PageDaemon, send: @escaping (ClientMsg) -> Void, nextReq: @escaping () -> UInt64) {
        self.daemon = daemon
        self.send = send
        self.nextReq = nextReq
    }

    /// The view of artifact `name` inside plugin pane `pane`'s page. `cwd` is the space's directory, which keys the page's
    /// browser storage as a web pane's is.
    public func view(pane: String, name: String, cwd: String) -> AnyView {
        AnyView(ArtifactEmbedView(embeds: self, pane: pane, name: name, cwd: cwd).id("\(pane)/\(name)"))
    }

    /// Ask again for every page shown: wheeld may have restarted and forgotten them.
    public func connected() {
        waiting = [:]
        for embed in live.values { open(embed) }
    }

    /// Take one wheeld message; true when it was for an embed.
    @discardableResult
    public func handle(_ msg: ServerMsg) -> Bool {
        switch msg {
        case let .pluginEmbed(req, _, _, scope, id):
            guard let embed = waiting.removeValue(forKey: req) else { return false }
            embed.show(id: id, scope: scope)
        case let .error(req?, _, message):
            guard let embed = waiting.removeValue(forKey: req) else { return false }
            embed.fail(message)
        case let .paneReload(pane):
            let shown = live.values.filter { $0.id == pane }
            shown.forEach { $0.reload() }
            return !shown.isEmpty
        default: return false
        }
        return true
    }

    /// What the debug state lists for plugin pane `pane`: each artifact it embeds, its page id, the HTTP status of its last
    /// document, its loads (reloads included) and wheeld's refusal.
    public func state(pane: String) -> [[String: Any]] {
        live.values.filter { $0.pane == pane }.map { e in
            var o: [String: Any] = ["artifact": e.name, "loads": e.loads]
            if let id = e.id { o["id"] = id }
            if let error = e.error { o["error"] = error }
            if let status = e.status { o["status"] = status }
            return o
        }
    }

    func appear(_ embed: ArtifactEmbed) {
        live[ObjectIdentifier(embed)] = embed
        open(embed)
    }

    func disappear(_ embed: ArtifactEmbed) {
        live[ObjectIdentifier(embed)] = nil
        waiting = waiting.filter { $0.value !== embed }
    }

    private func open(_ embed: ArtifactEmbed) {
        let req = nextReq()
        waiting[req] = embed
        send(.pluginEmbed(req: req, pane: embed.pane, artifact: embed.name))
    }
}

/// One embedded artifact: its page id once wheeld answered, and the web view showing it.
@MainActor @Observable
final class ArtifactEmbed: NSObject, WKNavigationDelegate {
    let pane: String
    let name: String
    let cwd: String
    private(set) var id: String?
    private(set) var error: String?
    private(set) var web: WKWebView?
    @ObservationIgnored private(set) var loads = 0
    @ObservationIgnored private(set) var status: Int?
    @ObservationIgnored private let daemon: PageDaemon

    init(pane: String, name: String, cwd: String, daemon: PageDaemon) {
        self.pane = pane
        self.name = name
        self.cwd = cwd
        self.daemon = daemon
    }

    func show(id: String, scope: ArtifactScope) {
        error = nil
        guard id != self.id || web == nil, let page = WebPanes.pageURL(pane: id) else { return }
        self.id = id
        let config = WKWebViewConfiguration()
        config.websiteDataStore = WKWebsiteDataStore(forIdentifier: WebPanes.store(cwd: cwd, artifact: ArtifactRef(scope: scope, name: name)))
        let handler = PageSchemeHandler(pane: id, daemon: daemon)
        handler.onDocument = { [weak self] code in self?.status = code }
        config.setURLSchemeHandler(handler, forURLScheme: WebPanes.scheme)
        let view = WKWebView(frame: .zero, configuration: config)
        view.navigationDelegate = self
        web = view
        loads += 1
        view.load(URLRequest(url: page))
    }

    func fail(_ message: String) {
        error = message
        web = nil
        id = nil
    }

    /// A file of the page changed. Opens the address again rather than `WKWebView.reload()`, which does nothing when the
    /// first load never committed.
    func reload() {
        guard let id, let page = WebPanes.pageURL(pane: id) else { return }
        loads += 1
        web?.load(URLRequest(url: page))
    }

    /// The page stays inside its own folder; a link anywhere else opens in the system's browser.
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let target = action.request.url, let id else { return .cancel }
        if WebPanes.allowsInlineFrame(target, isMainFrame: action.targetFrame?.isMainFrame ?? true) { return .allow }
        let path = URLComponents(url: target, resolvingAgainstBaseURL: false)?.path ?? target.path
        if target.scheme == WebPanes.scheme, path.hasPrefix("/artifacts/\(id)/") { return .allow }
        if action.navigationType == .linkActivated {
            #if os(macOS)
            NSWorkspace.shared.open(target)
            #else
            await UIApplication.shared.open(target)
            #endif
        }
        return .cancel
    }
}

private struct ArtifactEmbedView: View {
    let embeds: ArtifactEmbeds
    @State private var embed: ArtifactEmbed

    init(embeds: ArtifactEmbeds, pane: String, name: String, cwd: String) {
        self.embeds = embeds
        _embed = State(initialValue: ArtifactEmbed(pane: pane, name: name, cwd: cwd, daemon: embeds.daemon))
    }

    var body: some View {
        Group {
            if let web = embed.web {
                ArtifactWebView(web: web)
            } else {
                Text(embed.error ?? "Opening \(embed.name)")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .padding(8)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onAppear { embeds.appear(embed) }
        .onDisappear { embeds.disappear(embed) }
    }
}

#if os(macOS)
private struct ArtifactWebView: NSViewRepresentable {
    let web: WKWebView
    func makeNSView(context: Context) -> WKWebView { web }
    func updateNSView(_ view: WKWebView, context: Context) {}
}
#else
private struct ArtifactWebView: UIViewRepresentable {
    let web: WKWebView
    func makeUIView(context: Context) -> WKWebView { web }
    func updateUIView(_ view: WKWebView, context: Context) {}
}
#endif
