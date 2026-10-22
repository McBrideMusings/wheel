import SwiftUI
import WebKit
import WheelKit
import WheelUI

/// One web pane's page. The model keeps it across swipes and layout changes, so a page keeps its
/// scroll position. The page opens under `WebPanes.scheme`; `PageSchemeHandler` fetches it from
/// the Mac's remote listener with the token in a header, pinning the listener's certificate.
@MainActor
final class WebPane: NSObject, WKNavigationDelegate {
    let pane: String
    let artifact: ArtifactRef
    let view: WKWebView
    private var url: URL?
    private var status: Int?

    init(pane: String, artifact: ArtifactRef, store: UUID, daemon: PageDaemon) {
        self.pane = pane
        self.artifact = artifact
        let config = WKWebViewConfiguration()
        config.websiteDataStore = WKWebsiteDataStore(forIdentifier: store)
        let pages = PageSchemeHandler(pane: pane, daemon: daemon)
        config.setURLSchemeHandler(pages, forURLScheme: WebPanes.scheme)
        view = WKWebView(frame: .zero, configuration: config)
        super.init()
        view.navigationDelegate = self
        pages.onDocument = { [weak self] code in self?.status = code }
    }

    func load(_ url: URL) {
        self.url = url
        Log.info("web pane \(pane) loading \(WebPanes.withoutQuery(url))")
        view.load(URLRequest(url: url))
    }

    /// A file in the artifact's folder changed. Opens the address again rather than
    /// `WKWebView.reload()`, which does nothing when the first load never committed.
    func reload() {
        guard let url else { return }
        Log.info("web pane \(pane) reloading")
        view.load(URLRequest(url: url))
    }

    /// The page stays inside its pane's folder; a tapped link anywhere else opens in Safari.
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let target = action.request.url, let url else { return .cancel }
        let path = URLComponents(url: target, resolvingAgainstBaseURL: false)?.path ?? target.path
        if target.scheme == url.scheme, target.host == url.host, path.hasPrefix("/artifacts/\(pane)/") { return .allow }
        if action.navigationType == .linkActivated {
            Log.info("web pane \(pane) opening \(WebPanes.withoutQuery(target)) in Safari")
            await UIApplication.shared.open(target)
        } else {
            Log.info("web pane \(pane) refused navigation to \(WebPanes.withoutQuery(target))")
        }
        return .cancel
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        let pane = pane
        let status = status.map(String.init) ?? "?"
        webView.evaluateJavaScript("document.body ? document.body.innerText : ''") { result, _ in
            let text = (result as? String ?? "").replacingOccurrences(of: "\n", with: " ⏎ ")
            Log.info("web pane \(pane) loaded, status \(status), text \"\(text.prefix(300))\"")
        }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        Log.info("web pane \(pane) failed to load: \(WebPanes.describe(error))")
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        Log.info("web pane \(pane) failed to load: \(WebPanes.describe(error))")
    }
}

/// Shows a web pane's view where SwiftUI puts it.
struct WebBody: UIViewRepresentable {
    let web: WebPane

    func makeUIView(context: Context) -> WKWebView { web.view }
    func updateUIView(_ view: WKWebView, context: Context) {}
}
