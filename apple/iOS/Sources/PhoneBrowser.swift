import SwiftUI
import WebKit
import WheelKit
import WheelUI

/// One browser pane's page drawn by the phone itself (ADR-0005). The phone's web view is its own
/// browser: it opens at the pane's home once and then goes only where the person or an agent sends
/// it (a change of the home moves it nowhere), and nothing it does is reported to wheeld. A page on the Mac's loopback loads under
/// `BrowserPanes.loopbackScheme`, fetched from wheeld's `/loopback/<port>/`. Agents reach the
/// page with `--on phone`. What the page
/// writes to its console goes to wheeld as the phone's own (`wheel pane console --on phone`).
@MainActor
@Observable
final class PhoneBrowser: NSObject, WKNavigationDelegate {
    let pane: String
    @ObservationIgnored let view: WKWebView
    /// The page the web view is on, as the Mac names it.
    private(set) var page: String = ""
    /// Whether the web view can go back.
    private(set) var canGoBack = false
    @ObservationIgnored private let relay = ConsoleRelay()
    @ObservationIgnored private let handler: PageSchemeHandler
    /// The page `load` sent the web view to, until its navigation starts: one the browser started.
    @ObservationIgnored private var browserLoad: URL?
    /// The pane's home as the layout last named it: where `goHome` goes.
    private(set) var home: String
    /// Counts `prepareScripts` calls, so one that waited on the store while a later navigation
    /// started leaves the later one's scripts alone.
    @ObservationIgnored private var scriptGeneration = 0
    @ObservationIgnored private var watches: [NSKeyValueObservation] = []
    /// Why the last load failed, shown over the page until the next one starts.
    private(set) var failure: String?

    init(pane: String, home: String, daemon: PageDaemon, console: @escaping (Bool, [ConsoleEntry]) -> Void) {
        self.pane = pane
        self.home = home
        let config = WKWebViewConfiguration()
        // The phone's own browser storage, shared by every browser pane it draws.
        config.websiteDataStore = .default()
        let handler = PageSchemeHandler(pane: pane, daemon: daemon, loopback: true)
        self.handler = handler
        config.setURLSchemeHandler(handler, forURLScheme: BrowserPanes.loopbackScheme)
        view = WKWebView(frame: .zero, configuration: config)
        super.init()
        config.userContentController.add(CookieMessages(self), name: LoopbackCookies.cookieHandler)
        // The relay holds no reference to this browser, so the content controller, which retains
        // its handlers, keeps neither alive.
        config.userContentController.add(relay, name: BrowserConsole.handlerName)
        // A script on the Mac's loopback is named by the page it stands for, as the Mac names it.
        relay.onMessage = { reset, entry in
            console(reset, entry.map { e in
                [ConsoleEntry(time: e.time, level: e.level, text: e.text,
                              url: e.url.map { URL(string: $0).map(BrowserPanes.macPage) ?? $0 }, line: e.line)]
            } ?? [])
        }
        handler.onCookies = { [weak self] in self?.cookiesChanged() }
        view.navigationDelegate = self
        view.allowsBackForwardNavigationGestures = true
        // A page that moves itself (history.pushState) changes the URL with no navigation.
        watches.append(view.observe(\.url, options: [.new]) { [weak self] web, _ in
            MainActor.assumeIsolated { self?.landed(web.url) }
        })
        watches.append(view.observe(\.canGoBack, options: [.new]) { [weak self] web, _ in
            MainActor.assumeIsolated { self?.canGoBack = web.canGoBack }
        })
        load(home)
    }

    /// The layout holds `url` as this pane's home now. The browser stays where it is.
    func homeIs(_ url: String) {
        home = url
    }

    /// Go to the pane's home.
    func goHome() {
        load(home)
    }

    /// Go to `url`, which wheeld resolved from an agent's `pane page`.
    func open(_ url: String) {
        load(url)
    }

    /// `pane_reload`: load the page the browser is on again, or the last one it was sent to when none committed.
    func reload() {
        if view.url == nil { load(page) } else { view.reload() }
    }

    private func load(_ url: String) {
        page = url
        guard let target = BrowserPanes.phonePage(url) ?? URL(string: url) else {
            Log.info("browser pane \(pane) cannot load \(WebPanes.withoutQuery(URL(string: url) ?? URL(fileURLWithPath: "/")))")
            return
        }
        Log.info("browser pane \(pane) loading \(WebPanes.withoutQuery(target))")
        browserLoad = target
        view.load(URLRequest(url: target))
    }

    func goBack() { view.goBack() }

    /// The web view is on `url` now: `page` names the page it stands for.
    private func landed(_ url: URL?) {
        guard let url, url.absoluteString != "about:blank" else { return }
        page = BrowserPanes.macPage(url)
    }

    /// `inspect_pane` `op` with `arg`, run on this page.
    func run(_ op: String, arg: String?) async -> BrowserInspect.Outcome {
        if op == "where" {
            let current = view.url.map(BrowserPanes.macPage) ?? (page.isEmpty ? "about:blank" : page)
            return .text(BrowserInspect.place(url: current.isEmpty ? "about:blank" : current, title: view.title ?? ""))
        }
        if op == "open" {
            guard let arg, !arg.isEmpty else { return .refused("open needs a page") }
            open(arg)
            return .text("")
        }
        return await BrowserInspect.run(op, arg: arg, in: view)
    }

    /// The page as a PNG, the part in view or with `full` the whole document, for `render_pane`.
    func shot(full: Bool) async -> Result<Data, BrowserInspect.Refusal> {
        return await BrowserInspect.shotPage(view, full: full).flatMap { image in
            BrowserInspect.png(image).map(Result.success) ?? .failure("the drawing could not be encoded as a PNG")
        }
    }

    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        // A page from elsewhere may link to the Mac's loopback page, but never post a form to it:
        // the request would reach the Mac's program with no Origin to refuse it by. A loopback
        // page may post to any loopback port, as on the Mac.
        if action.request.url?.scheme == BrowserPanes.loopbackScheme, (action.request.httpMethod ?? "GET") != "GET",
           action.sourceFrame.request.url.map({
               BrowserPanes.loopbackRequestAllowed(action.request.url!, document: $0, origin: nil, method: "GET")
           }) != true {
            Log.info("browser pane \(pane) refused a \(action.request.httpMethod ?? "-") to the Mac's loopback from another site")
            return .cancel
        }
        guard action.targetFrame?.isMainFrame != false, let url = action.request.url else { return .allow }
        // Who started a loopback navigation decides the `SameSite` of its cookies: the browser
        // (its own load, back, forward, reload) or the page in the frame that started it.
        if url.scheme == BrowserPanes.loopbackScheme {
            let byBrowser = url == browserLoad || [WKNavigationType.backForward, .reload].contains(action.navigationType)
            handler.navigation(to: url, from: byBrowser ? nil : action.sourceFrame.request.url)
        }
        if url == browserLoad { browserLoad = nil }
        await prepareScripts(for: url)
        return .allow
    }

    /// The user scripts the page about to load at `url` starts with: the console relay, and on
    /// the Mac's loopback a `document.cookie` from the store, which WebKit gives a custom scheme
    /// none of.
    /// This controller holds no other script: every one it runs is added here.
    private func prepareScripts(for url: URL) async {
        scriptGeneration += 1
        let generation = scriptGeneration
        var cookies: [(name: String, value: String)]?
        if url.scheme == BrowserPanes.loopbackScheme, let mac = URL(string: BrowserPanes.macPage(url)) {
            cookies = await LoopbackCookies.scriptCookies(for: mac, store: view.configuration.websiteDataStore)
        }
        guard generation == scriptGeneration else { return }
        let scripts = view.configuration.userContentController
        scripts.removeAllUserScripts()
        scripts.addUserScript(WKUserScript(source: BrowserConsole.script, injectionTime: .atDocumentStart, forMainFrameOnly: false))
        if let cookies { scripts.addUserScript(LoopbackCookies.cookieScript(cookies)) }
    }

    /// The store's cookies changed (a response set one, or the page wrote one): the page in view
    /// and the next one it loads read the store's.
    private func cookiesChanged() {
        guard let url = view.url, url.scheme == BrowserPanes.loopbackScheme, let mac = URL(string: BrowserPanes.macPage(url)) else { return }
        Task {
            let cookies = await LoopbackCookies.scriptCookies(for: mac, store: view.configuration.websiteDataStore)
            _ = try? await view.evaluateJavaScript(LoopbackCookies.refreshScript(cookies))
            await prepareScripts(for: url)
        }
    }

    /// The loopback page set `document.cookie = line`: keep it in the store, where the scheme
    /// handler's next request finds it.
    fileprivate func pageSetCookie(_ line: String) {
        guard let url = view.url, url.scheme == BrowserPanes.loopbackScheme, let mac = URL(string: BrowserPanes.macPage(url)) else { return }
        LoopbackCookies.queueScriptCookie(line, for: mac, store: view.configuration.websiteDataStore) { [weak self] in
            self?.cookiesChanged()
        }
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        failure = nil
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        Log.info("browser pane \(pane) loaded")
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        failed(error)
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        failed(error)
    }

    private func failed(_ error: Error) {
        // A navigation the page itself replaced (a link clicked mid-load) is no failure to show.
        if (error as NSError).code == NSURLErrorCancelled { return }
        failure = WebPanes.describe(error)
        Log.info("browser pane \(pane) failed to load: \(failure ?? "")")
    }
}

/// Shows a phone browser pane's web view where SwiftUI puts it.
struct PhoneBrowserBody: UIViewRepresentable {
    let browser: PhoneBrowser

    func makeUIView(context: Context) -> WKWebView { browser.view }
    func updateUIView(_ view: WKWebView, context: Context) {}
}

/// The buttons of a browser pane's bottom toolbar, in the order they are drawn.
enum BrowserButton: String, CaseIterable {
    case back, home, setHome = "set-home", reload

    var accessibilityID: String { "browser-\(rawValue)" }
    var label: String {
        switch self {
        case .back: "Back"
        case .home: "Home"
        case .setHome: "Set as home"
        case .reload: "Reload"
        }
    }
    var symbol: String {
        switch self {
        case .back: "chevron.backward"
        case .home: "house"
        case .setHome: "house.and.flag"
        case .reload: "arrow.clockwise"
        }
    }
}

extension PhoneBrowser {
    /// Whether `button` takes a press now: back needs a page to go back to, and the page in view
    /// being the pane's home leaves nothing to set.
    func enabled(_ button: BrowserButton) -> Bool {
        switch button {
        case .back: canGoBack
        case .setHome: page != home
        case .home, .reload: true
        }
    }
}

/// A browser pane on the phone: the phone's own page under the system navigation bar, whose title
/// is the page's host, with back, Home, Set as home and reload in the system toolbar below it.
/// UIKit's navigation controller draws both; a SwiftUI `NavigationStack` in each page of the
/// carousel's pager asserts in `UINavigationBar.layoutSubviews` once a second page exists.
struct BrowserPage: UIViewControllerRepresentable {
    let model: PhoneModel
    let pane: String
    let url: String

    func makeUIViewController(context: Context) -> UINavigationController {
        let root = UIHostingController(rootView: BrowserContent(browser: model.browsers[pane]))
        let nav = UINavigationController(rootViewController: root)
        nav.setToolbarHidden(false, animated: false)
        root.navigationItem.titleView = nil
        root.navigationItem.largeTitleDisplayMode = .never
        return nav
    }

    func updateUIViewController(_ nav: UINavigationController, context: Context) {
        guard let root = nav.viewControllers.first as? UIHostingController<BrowserContent> else { return }
        let browser = model.browsers[pane]
        root.rootView = BrowserContent(browser: browser)
        root.navigationItem.title = title(browser)
        var items: [UIBarButtonItem] = []
        for (index, button) in BrowserButton.allCases.enumerated() {
            if index > 0 { items.append(UIBarButtonItem(systemItem: .flexibleSpace)) }
            let item = UIBarButtonItem(image: UIImage(systemName: button.symbol), primaryAction: UIAction { [model, pane] _ in
                model.press(button, in: pane)
            })
            item.isEnabled = browser?.enabled(button) == true
            item.accessibilityLabel = button.label
            item.accessibilityIdentifier = button.accessibilityID
            items.append(item)
        }
        root.toolbarItems = items
    }

    /// The page's host, or the page itself for a file or a blank one.
    private func title(_ browser: PhoneBrowser?) -> String {
        let page = browser?.page.isEmpty == false ? browser!.page : url
        switch BrowserPanes.shown(page) {
        case .blank: return page
        case let .web(_, host, _): return host
        case .file, .other: return page
        }
    }
}

/// The page under the navigation bar: the web view, and why it could not open when it could not.
private struct BrowserContent: View {
    let browser: PhoneBrowser?

    var body: some View {
        ZStack {
            if let browser {
                PhoneBrowserBody(browser: browser)
                    .overlay {
                        if let failure = browser.failure {
                            ContentUnavailableView("Could not open the page", systemImage: "exclamationmark.triangle",
                                                   description: Text(failure))
                                .background(.background)
                                .accessibilityIdentifier("browser-failure")
                        }
                    }
            }
        }
    }
}

/// Hands `document.cookie` writes to the browser without the content controller, which retains
/// its handlers, keeping the browser alive.
private final class CookieMessages: NSObject, WKScriptMessageHandler {
    weak var browser: PhoneBrowser?
    init(_ browser: PhoneBrowser) { self.browser = browser }
    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let line = message.body as? String else { return }
        MainActor.assumeIsolated { browser?.pageSetCookie(line) }
    }
}
