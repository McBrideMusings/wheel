import SwiftUI
import WebKit
import WheelKit

/// One web pane's page. The model keeps it across renders and layout changes, so a reload or a
/// split keeps the same view (and its scroll position) instead of building a new one.
@MainActor
final class WebPane: NSObject, ObservableObject, WKNavigationDelegate {
    let pane: String
    /// The artifact the daemon serves for this pane. The daemon owns the folder it resolves to, so
    /// the client never holds a path.
    let artifact: ArtifactRef
    /// Kept in memory only, so the token cookie the daemon hands the page never reaches disk.
    let view: WKWebView = {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        return WKWebView(frame: .zero, configuration: config)
    }()
    /// The address the page opens at, token included; the daemon trades the token for a cookie
    /// and redirects to the same address without it.
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

    init(pane: String, artifact: ArtifactRef) {
        self.pane = pane
        self.artifact = artifact
        super.init()
        view.navigationDelegate = self
    }

    var loaded: Bool { url != nil }

    /// The address with its query removed: the query carries the daemon token, which no log line
    /// may hold.
    static func withoutQuery(_ url: URL) -> String {
        var parts = URLComponents(url: url, resolvingAgainstBaseURL: false)
        parts?.query = nil
        return parts?.string ?? url.path
    }

    func load(_ url: URL) {
        self.url = url
        address = Self.withoutQuery(url)
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
            Log.info("web pane \(pane) opening \(Self.withoutQuery(target)) in the browser")
            NSWorkspace.shared.open(target)
        } else {
            Log.info("web pane \(pane) refused navigation to \(Self.withoutQuery(target))")
        }
        return .cancel
    }

    func webView(_ webView: WKWebView, decidePolicyFor navigationResponse: WKNavigationResponse) async -> WKNavigationResponsePolicy {
        if navigationResponse.isForMainFrame {
            status = (navigationResponse.response as? HTTPURLResponse)?.statusCode
        }
        return .allow
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
        loadError = error.localizedDescription
        Log.info("web pane \(pane) failed to load: \(error.localizedDescription)")
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
