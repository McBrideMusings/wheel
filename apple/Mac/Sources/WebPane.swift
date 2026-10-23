import SwiftUI
import WebKit
import WheelKit
import WheelUI

/// One web pane's page. The model keeps it across renders and layout changes, so a reload or a
/// split keeps the same view (and its scroll position) instead of building a new one.
@MainActor
final class WebPane: NSObject, ObservableObject, WKNavigationDelegate {
    let pane: String
    /// The artifact the daemon serves for this pane. The daemon owns the folder it resolves to, so
    /// the client never holds a path.
    let artifact: ArtifactRef
    /// The identifier of the page's browser storage.
    let store: UUID
    /// The page's own browser storage (`localStorage`, IndexedDB) lives in a store kept on disk,
    /// one per artifact (`WebPanes.store`), so it survives the pane and the app. The
    /// page's origin is fixed, so the storage also survives the daemon coming back on another port.
    let view: WKWebView
    /// The address the page opens at, under `WebPanes.scheme`; `PageSchemeHandler` fetches it from
    /// the daemon.
    private var url: URL?
    /// The page's `<title>`, once it loaded.
    @Published private(set) var title = ""
    /// The page's address without its token, for logs and the debug state.
    private(set) var address: String?
    private(set) var lastReloadAt: Date?
    private(set) var loadedAt: Date?
    /// The HTTP status of the last page load.
    private(set) var status: Int?
    private(set) var loadError: String?
    /// The page's visible text after the last load, for the debug state.
    private(set) var text: String?

    init(pane: String, artifact: ArtifactRef, store: UUID, daemon: PageDaemon) {
        self.pane = pane
        self.artifact = artifact
        self.store = store
        let config = WKWebViewConfiguration()
        config.websiteDataStore = WKWebsiteDataStore(forIdentifier: store)
        let pages = PageSchemeHandler(pane: pane, daemon: daemon)
        config.setURLSchemeHandler(pages, forURLScheme: WebPanes.scheme)
        view = WKWebView(frame: .zero, configuration: config)
        super.init()
        Log.info("web pane \(pane) keeps its page's storage in store \(store.uuidString)")
        view.navigationDelegate = self
        pages.onDocument = { [weak self] code in self?.status = code }
    }

    var loaded: Bool { url != nil }

    func load(_ url: URL) {
        self.url = url
        address = WebPanes.withoutQuery(url)
        Log.info("web pane \(pane) loading \(address ?? "?")")
        view.load(URLRequest(url: url))
    }

    /// A file in the folder changed. Opens the address again rather than `WKWebView.reload()`,
    /// which does nothing when the first load never committed (the daemon was down).
    func reload() {
        guard let url else { return }
        lastReloadAt = Date()
        Log.info("web pane \(pane) reloading")
        view.load(URLRequest(url: url))
    }

    /// The page stays inside its pane's folder on the daemon; a link anywhere else opens in the
    /// default browser.
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let target = action.request.url, let url else { return .cancel }
        // `URL.path` drops a trailing slash, which would refuse the folder's own address — the
        // page a pane opens — so the path is read as it was written.
        let path = URLComponents(url: target, resolvingAgainstBaseURL: false)?.path ?? target.path
        if target.scheme == url.scheme, target.host == url.host, target.port == url.port,
           path.hasPrefix("/artifacts/\(pane)/") {
            return .allow
        }
        if action.navigationType == .linkActivated {
            Log.info("web pane \(pane) opening \(WebPanes.withoutQuery(target)) in the browser")
            NSWorkspace.shared.open(target)
        } else {
            Log.info("web pane \(pane) refused navigation to \(WebPanes.withoutQuery(target))")
        }
        return .cancel
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        loadedAt = Date()
        loadError = nil
        title = webView.title ?? ""
        Log.info("web pane \(pane) loaded, status \(status.map(String.init) ?? "?"), title \"\(title)\"")
        webView.evaluateJavaScript("document.body ? document.body.innerText : ''") { [weak self] result, _ in
            MainActor.assumeIsolated { self?.text = result as? String }
        }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        failed(error)
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        failed(error)
    }

    private func failed(_ error: Error) {
        let described = WebPanes.describe(error)
        loadError = described
        Log.info("web pane \(pane) failed to load: \(described)")
    }
}

/// Shows a web pane's view where SwiftUI puts it.
struct WebBody: NSViewRepresentable {
    let web: WebPane

    func makeNSView(context: Context) -> WKWebView { web.view }
    func updateNSView(_ view: WKWebView, context: Context) {}
}

/// The page's title, or the artifact's name until it has one.
struct WebTitle: View {
    @ObservedObject var web: WebPane

    var body: some View {
        Text(web.title.isEmpty ? web.artifact.name : web.title)
    }
}
