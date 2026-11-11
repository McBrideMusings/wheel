#if os(macOS)
import AppKit
import WebKit
import WheelKit

/// One browser pane's page and chrome state, for every Mac client that draws a browser pane:
/// the SwiftUI app shows it under its own chrome, the GPUI client hosts it from its bridge and
/// draws the chrome itself. The owner keeps it across renders and layout changes, so a split
/// keeps the same view, its history and its scroll position.
///
/// Unlike a web pane, this is an ordinary browser: any http or https page, or any file on this
/// Mac, with read access to the whole disk so a raw HTML page loads whatever it names. It never
/// sees the daemon's token.
@MainActor
public final class BrowserPane: NSObject, ObservableObject, WKNavigationDelegate, WKUIDelegate {
    public let pane: String
    /// Whether wheeld runs on another machine, so a file page names a path this Mac does not have.
    private let daemonIsRemote: Bool
    /// Persistent website data, as a browser keeps: a login survives a relaunch.
    public let view: WKWebView
    private let relay = ConsoleRelay()
    /// The page the view is on, or the one the daemon gave it before anything committed.
    @Published public private(set) var url: String
    /// The page's `<title>`; empty until it names itself.
    @Published public private(set) var title = ""
    @Published public private(set) var loading = false
    /// WebKit's estimate, 0 to 1, of how much of the current load has arrived.
    @Published public private(set) var progress = 0.0
    @Published public private(set) var canGoBack = false
    @Published public private(set) var canGoForward = false
    /// The last load that failed, shown over the page until the next one starts.
    @Published public private(set) var failure: Failure?
    /// The text in the address field while the person edits it; nil when they are not.
    @Published public var editing: String?
    /// Whether the pane is too narrow for the full chrome row, as the row last measured it.
    @Published public var narrow = false
    public private(set) var loadedAt: Date?
    public private(set) var status: Int?
    /// The page's visible text after the last load, for the debug state.
    public private(set) var text: String?
    /// Where the pane's log lines go: the owner's log, or nowhere when it gave none.
    private let log: (String) -> Void

    public struct Failure: Equatable, Sendable {
        /// The address that failed, as the field shows it.
        public let address: String
        /// The page to try again; nil when the daemon refused the text as no page at all.
        public let target: URL?
        public let message: String
    }

    /// Tells the daemon the pane is on a new page.
    private let report: (String) -> Void
    /// Asks the daemon what typed text means; it answers through `opened` or `refused`.
    private let ask: (String) -> Void
    /// Tells the daemon what the page wrote to its console; `true` says the page just loaded.
    private let console: (Bool, [ConsoleEntry]) -> Void
    private var navigation = BrowserPanes.Navigation()
    /// The page the daemon was last told about, or last asked for itself.
    private var reported: String?
    private var observations: [NSKeyValueObservation] = []

    public init(pane: String, url: String, daemonIsRemote: Bool, log: @escaping (String) -> Void = { _ in },
                report: @escaping (String) -> Void, ask: @escaping (String) -> Void,
                console: @escaping (Bool, [ConsoleEntry]) -> Void) {
        let configuration = WKWebViewConfiguration()
        configuration.userContentController.addUserScript(
            WKUserScript(source: BrowserConsole.script, injectionTime: .atDocumentStart, forMainFrameOnly: false))
        configuration.userContentController.add(relay, name: BrowserConsole.handlerName)
        view = WKWebView(frame: .zero, configuration: configuration)
        view.allowsBackForwardNavigationGestures = true
        // Shows the pane under Safari's Develop menu.
        view.isInspectable = true
        self.console = console
        self.pane = pane
        self.url = url
        self.daemonIsRemote = daemonIsRemote
        self.log = log
        self.report = report
        self.ask = ask
        super.init()
        relay.onMessage = { [weak self] reset, entry in
            guard let self else { return }
            self.console(reset, entry.map { [$0] } ?? [])
        }
        view.navigationDelegate = self
        view.uiDelegate = self
        observations = [
            view.observe(\.url) { [weak self] view, _ in MainActor.assumeIsolated { self?.committed(view.url) } },
            view.observe(\.title) { [weak self] view, _ in MainActor.assumeIsolated { self?.title = view.title ?? "" } },
            view.observe(\.isLoading) { [weak self] view, _ in MainActor.assumeIsolated { self?.loading = view.isLoading } },
            view.observe(\.estimatedProgress) { [weak self] view, _ in MainActor.assumeIsolated { self?.progress = view.estimatedProgress } },
            view.observe(\.canGoBack) { [weak self] view, _ in MainActor.assumeIsolated { self?.canGoBack = view.canGoBack } },
            view.observe(\.canGoForward) { [weak self] view, _ in MainActor.assumeIsolated { self?.canGoForward = view.canGoForward } },
        ]
        go(url)
    }

    /// What the address field shows: the failed address while a failure shows, else the page.
    public var address: String { failure?.address ?? url }

    /// The daemon says this pane is on `url`, in a whole layout or a `pane_url`. One of this
    /// pane's own reports coming back changes nothing; any other page is somebody asking the pane
    /// to go there.
    public func daemonSays(_ url: String) {
        guard let target = navigation.daemonSays(url, page: self.url, failed: failure != nil) else { return }
        log("browser pane \(pane) sent to \(url) by the daemon")
        go(target)
    }

    /// Open what the person typed into the address field: the daemon says what page it means.
    public func open(typed: String) {
        editing = nil
        navigation.asked()
        log("browser pane \(pane) asking the daemon what typed text means, \(typed.count) chars")
        ask(typed)
    }

    /// The daemon read the typed text as `url`. The pane goes there even when it is already on it,
    /// as a browser does, unless the daemon has moved it since.
    public func opened(_ url: String) {
        guard let target = navigation.opened(url) else { return }
        log("browser pane \(pane) opening typed text as \(url)")
        go(target)
    }

    /// The daemon read the typed text as no page, and `message` says why.
    public func refused(typed: String, _ message: String) {
        log("browser pane \(pane) typed text refused: \(message)")
        failure = Failure(address: typed, target: nil, message: message)
    }

    /// Home: the page behind a failure shows at once, because when the homepage is that page the
    /// daemon has nothing new to send; any other homepage arrives as the daemon moving the pane.
    public func clearFailure() { failure = nil }

    public func back() { view.goBack() }
    public func forward() { view.goForward() }
    public func stop() { view.stopLoading() }

    /// Reload the page, or try the failed address again.
    public func reload() {
        if let failure {
            // Text that was never an address has nothing to try again; the page behind it shows.
            guard let target = failure.target else {
                self.failure = nil
                return
            }
            load(target)
        } else if view.url == nil {
            go(url)
        } else {
            log("browser pane \(pane) reloading")
            view.reload()
        }
    }

    /// Go where the daemon says, without telling it back what it just said.
    private func go(_ url: String) {
        reported = url
        navigation.went(url)
        self.url = url
        guard let target = URL(string: url) else { return }
        load(target)
    }

    private func load(_ target: URL) {
        failure = nil
        status = nil
        if target.isFileURL, daemonIsRemote {
            // The path is on the daemon's machine; a file of this Mac's at the same path is a
            // different file.
            let path = target.path(percentEncoded: false)
            log("browser pane \(pane) refused file \(path): wheeld is on another machine")
            failure = Failure(address: path, target: target, message: "This file is on the machine wheeld runs on, which this Mac can’t reach.")
        } else if target.isFileURL {
            // The whole disk, so a page reaches every stylesheet, script and image it names
            // wherever it sits: the owner's call, knowing the page's scripts can read any file.
            view.loadFileURL(target, allowingReadAccessTo: URL(fileURLWithPath: "/"))
        } else {
            view.load(URLRequest(url: target))
        }
    }

    /// The view moved to a new page — a load committed, a redirect, or an in-page navigation.
    private func committed(_ page: URL?) {
        guard let page else { return }
        let string = page.absoluteString
        url = string
        // Every page goes to the daemon, which keeps only the ones a pane may be on: a `data:` or
        // `blob:` document is shown but refused there.
        guard string != reported else { return }
        reported = string
        navigation.sent(string)
        log("browser pane \(pane) on \(string); telling the daemon")
        report(string)
    }

    // MARK: WKNavigationDelegate

    /// Everything a web view can show opens here. A link to anything else (mail, an app's own
    /// scheme) goes to whatever on this Mac handles it, but only when the person clicked it in the
    /// page itself: a script, a redirect or a frame never gets to launch an app.
    public func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let target = action.request.url else { return .cancel }
        switch target.scheme?.lowercased() {
        case "http", "https", "file", "about", "data", "blob":
            return .allow
        default:
            if action.navigationType == .linkActivated, action.targetFrame?.isMainFrame ?? true {
                log("browser pane \(pane) handing a clicked \(target.scheme ?? "?"): link to the system")
                NSWorkspace.shared.open(target)
            } else {
                log("browser pane \(pane) refused a \(target.scheme ?? "?"): navigation nobody clicked")
            }
            return .cancel
        }
    }

    /// A page the view cannot show is a download, which a pane does not do: a web one goes to the
    /// default browser, and a file says it cannot be shown rather than doing nothing.
    public func webView(_ webView: WKWebView, decidePolicyFor navigationResponse: WKNavigationResponse) async -> WKNavigationResponsePolicy {
        guard navigationResponse.isForMainFrame else { return .allow }
        status = (navigationResponse.response as? HTTPURLResponse)?.statusCode
        guard !navigationResponse.canShowMIMEType, let target = navigationResponse.response.url else { return .allow }
        if target.isFileURL {
            log("browser pane \(pane) cannot show \(target.path(percentEncoded: false)), \(navigationResponse.response.mimeType ?? "no type")")
            failure = Failure(address: target.path(percentEncoded: false), target: target, message: "A browser pane can’t show this kind of file.")
        } else {
            log("browser pane \(pane) handing a download to the default browser: \(target.absoluteString)")
            NSWorkspace.shared.open(target)
        }
        return .cancel
    }

    public func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        loadedAt = Date()
        log("browser pane \(pane) loaded \(url), status \(status.map(String.init) ?? "-"), title \"\(title)\"")
        webView.evaluateJavaScript("document.body ? document.body.innerText : ''") { [weak self] result, _ in
            MainActor.assumeIsolated { self?.text = result as? String }
        }
    }

    public func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        failed(error)
    }

    public func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        failed(error)
    }

    private func failed(_ error: Error) {
        let error = error as NSError
        // A load the person stopped, or replaced with another, is not a failure.
        if error.domain == NSURLErrorDomain, error.code == NSURLErrorCancelled { return }
        if error.domain == "WebKitErrorDomain", error.code == 102 { return }
        let failing = (error.userInfo[NSURLErrorFailingURLErrorKey] as? URL)
            ?? (error.userInfo[NSURLErrorFailingURLStringErrorKey] as? String).flatMap(URL.init(string:))
        let address = failing.map { $0.isFileURL ? $0.path(percentEncoded: false) : $0.absoluteString } ?? url
        log("browser pane \(pane) failed to load \(address): \(error.domain) \(error.code) \(error.localizedDescription)")
        // WebKit words a missing file as a server's 404, which a file has none of.
        let missingFile = failing?.isFileURL == true && error.domain == NSURLErrorDomain && error.code == NSURLErrorFileDoesNotExist
        failure = Failure(
            address: address,
            target: failing ?? URL(string: url),
            message: missingFile ? "The file couldn’t be opened because there is no such file." : error.localizedDescription
        )
    }

    // MARK: WKUIDelegate

    /// A link that asks for a new window opens in this pane: a pane has no tabs. A file opens this
    /// way only from a page that is itself a file, so a site's script cannot steer the pane onto
    /// the disk; the person opening a file is the only way a web page's pane gets there.
    public func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                        for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        guard let target = action.request.url else { return nil }
        if target.isFileURL {
            guard webView.url?.isFileURL == true else {
                log("browser pane \(pane) refused a new window on a file from \(webView.url?.absoluteString ?? "?")")
                return nil
            }
            load(target)
        } else {
            webView.load(action.request)
        }
        return nil
    }
}
#endif
