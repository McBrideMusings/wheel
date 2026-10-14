import SwiftUI
import WebKit
import WheelKit

/// One browser pane's page and chrome state. The model keeps it across renders and layout
/// changes, so a split keeps the same view, its history and its scroll position.
///
/// Unlike a web pane, this is an ordinary browser: any http or https page, or any file on this
/// Mac, with read access to the whole disk so a raw HTML page loads whatever it names. It never
/// sees the daemon's token.
@MainActor
final class BrowserPane: NSObject, ObservableObject, WKNavigationDelegate, WKUIDelegate {
    let pane: String
    /// Persistent website data, as a browser keeps: a login survives a relaunch.
    let view: WKWebView = {
        let view = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        view.allowsBackForwardNavigationGestures = true
        return view
    }()
    /// The page the view is on, or the one the daemon gave it before anything committed.
    @Published private(set) var url: String
    /// The page's `<title>`; empty until it names itself.
    @Published private(set) var title = ""
    @Published private(set) var loading = false
    /// WebKit's estimate, 0 to 1, of how much of the current load has arrived.
    @Published private(set) var progress = 0.0
    @Published private(set) var canGoBack = false
    @Published private(set) var canGoForward = false
    /// The last load that failed, shown over the page until the next one starts.
    @Published private(set) var failure: Failure?
    /// The text in the address field while the person edits it; nil when they are not.
    @Published var editing: String?
    /// Whether the pane is too narrow for the full chrome row, as the row last measured it.
    @Published var narrow = false
    private(set) var loadedAt: Date?
    private(set) var status: Int?
    /// The page's visible text after the last load, for the debug state.
    private(set) var text: String?

    struct Failure: Equatable {
        /// The address that failed, as the field shows it.
        let address: String
        let message: String
    }

    /// Tells the daemon the pane is on a new page.
    private let report: (String) -> Void
    private var echoes = BrowserPanes.Echoes()
    /// The page the daemon was last told about, or last asked for itself.
    private var reported: String?
    private var observations: [NSKeyValueObservation] = []

    init(pane: String, url: String, report: @escaping (String) -> Void) {
        self.pane = pane
        self.url = url
        self.report = report
        super.init()
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
    var address: String { failure?.address ?? url }

    /// The daemon says this pane is on `url`, in a whole layout or a `pane_url`. One of this
    /// pane's own reports coming back changes nothing; any other page is somebody asking the pane
    /// to go there.
    func daemonSays(_ url: String) {
        if echoes.isOwn(url) { return }
        if url == self.url, failure == nil { return }
        Log.info("browser pane \(pane) sent to \(url) by the daemon")
        go(url)
    }

    /// Open what the person typed into the address field.
    func open(typed: String) {
        editing = nil
        guard let target = BrowserPanes.address(typed: typed) else {
            Log.info("browser pane \(pane) refused typed text, \(typed.count) chars")
            failure = Failure(address: typed, message: "“\(typed)” isn’t a web address or a file path.")
            return
        }
        Log.info("browser pane \(pane) opening typed \(target.absoluteString)")
        load(target)
    }

    /// Home: the page behind a failure shows at once, because when the homepage is that page the
    /// daemon has nothing new to send; any other homepage arrives as the daemon moving the pane.
    func clearFailure() { failure = nil }

    func back() { view.goBack() }
    func forward() { view.goForward() }
    func stop() { view.stopLoading() }

    /// Reload the page, or try the failed address again.
    func reload() {
        if let failure {
            // Text that was never an address has nothing to try again; the page behind it shows.
            guard let target = BrowserPanes.address(typed: failure.address) else {
                self.failure = nil
                return
            }
            load(target)
        } else if view.url == nil {
            go(url)
        } else {
            Log.info("browser pane \(pane) reloading")
            view.reload()
        }
    }

    /// Go where the daemon says, without telling it back what it just said.
    private func go(_ url: String) {
        reported = url
        echoes.adopt(url)
        self.url = url
        guard let target = URL(string: url) else { return }
        load(target)
    }

    private func load(_ target: URL) {
        failure = nil
        status = nil
        if target.isFileURL {
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
        guard BrowserPanes.reportable(page), string != reported else { return }
        reported = string
        echoes.sent(string)
        Log.info("browser pane \(pane) on \(string); telling the daemon")
        report(string)
    }

    // MARK: WKNavigationDelegate

    /// Everything a web view can show opens here. A link to anything else (mail, an app's own
    /// scheme) goes to whatever on this Mac handles it, but only when the person clicked it in the
    /// page itself: a script, a redirect or a frame never gets to launch an app.
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let target = action.request.url else { return .cancel }
        switch target.scheme?.lowercased() {
        case "http", "https", "file", "about", "data", "blob":
            return .allow
        default:
            if action.navigationType == .linkActivated, action.targetFrame?.isMainFrame ?? true {
                Log.info("browser pane \(pane) handing a clicked \(target.scheme ?? "?"): link to the system")
                NSWorkspace.shared.open(target)
            } else {
                Log.info("browser pane \(pane) refused a \(target.scheme ?? "?"): navigation nobody clicked")
            }
            return .cancel
        }
    }

    /// A page the view cannot show is a download, which a pane does not do: a web one goes to the
    /// default browser, and a file says it cannot be shown rather than doing nothing.
    func webView(_ webView: WKWebView, decidePolicyFor navigationResponse: WKNavigationResponse) async -> WKNavigationResponsePolicy {
        guard navigationResponse.isForMainFrame else { return .allow }
        status = (navigationResponse.response as? HTTPURLResponse)?.statusCode
        guard !navigationResponse.canShowMIMEType, let target = navigationResponse.response.url else { return .allow }
        if target.isFileURL {
            Log.info("browser pane \(pane) cannot show \(target.path(percentEncoded: false)), \(navigationResponse.response.mimeType ?? "no type")")
            failure = Failure(address: target.path(percentEncoded: false), message: "A browser pane can’t show this kind of file.")
        } else {
            Log.info("browser pane \(pane) handing a download to the default browser: \(target.absoluteString)")
            NSWorkspace.shared.open(target)
        }
        return .cancel
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        loadedAt = Date()
        Log.info("browser pane \(pane) loaded \(url), status \(status.map(String.init) ?? "-"), title \"\(title)\"")
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
        let error = error as NSError
        // A load the person stopped, or replaced with another, is not a failure.
        if error.domain == NSURLErrorDomain, error.code == NSURLErrorCancelled { return }
        if error.domain == "WebKitErrorDomain", error.code == 102 { return }
        let failing = (error.userInfo[NSURLErrorFailingURLErrorKey] as? URL)
            ?? (error.userInfo[NSURLErrorFailingURLStringErrorKey] as? String).flatMap(URL.init(string:))
        let address = failing.map { $0.isFileURL ? $0.path(percentEncoded: false) : $0.absoluteString } ?? url
        Log.info("browser pane \(pane) failed to load \(address): \(error.domain) \(error.code) \(error.localizedDescription)")
        // WebKit words a missing file as a server's 404, which a file has none of.
        let missingFile = failing?.isFileURL == true && error.domain == NSURLErrorDomain && error.code == NSURLErrorFileDoesNotExist
        failure = Failure(
            address: address,
            message: missingFile ? "The file couldn’t be opened because there is no such file." : error.localizedDescription
        )
    }

    // MARK: WKUIDelegate

    /// A link that asks for a new window opens in this pane: a pane has no tabs. A file opens this
    /// way only from a page that is itself a file, so a site's script cannot steer the pane onto
    /// the disk; the person opening a file is the only way a web page's pane gets there.
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        guard let target = action.request.url else { return nil }
        if target.isFileURL {
            guard webView.url?.isFileURL == true else {
                Log.info("browser pane \(pane) refused a new window on a file from \(webView.url?.absoluteString ?? "?")")
                return nil
            }
            load(target)
        } else {
            webView.load(action.request)
        }
        return nil
    }
}

/// Shows a browser pane's view where SwiftUI puts it.
struct BrowserView: NSViewRepresentable {
    let browser: BrowserPane

    func makeNSView(context: Context) -> WKWebView { browser.view }
    func updateNSView(_ view: WKWebView, context: Context) {}
}

/// A browser pane under its title row: the chrome, then the page or why it failed.
struct BrowserBody: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @ObservedObject var browser: BrowserPane

    var body: some View {
        VStack(spacing: 0) {
            BrowserBar(model: model, paneId: paneId, browser: browser)
            Divider()
            BrowserView(browser: browser)
                .overlay {
                    if let failure = browser.failure {
                        BrowserFailure(failure: failure, retry: browser.reload)
                    }
                }
        }
    }
}

/// Back, forward, reload or stop, home and the address field. Under 360 pt only back and the
/// field stay; the pane's ⋯ menu carries the rest at every width.
struct BrowserBar: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @ObservedObject var browser: BrowserPane

    /// Narrower than this, the row keeps back and the field.
    static let fullWidth: CGFloat = 360

    var body: some View {
        GeometryReader { geo in
            HStack(spacing: 2) {
                button("Back", "chevron.left", enabled: browser.canGoBack, browser.back)
                if !browser.narrow {
                    button("Forward", "chevron.right", enabled: browser.canGoForward, browser.forward)
                    if browser.loading {
                        button("Stop Loading", "xmark", enabled: true, browser.stop)
                    } else {
                        button("Reload Page", "arrow.clockwise", enabled: true, browser.reload)
                    }
                    button("Home", "house", enabled: model.connection == .connected) { model.goHome(paneId) }
                }
                AddressField(browser: browser)
                    .padding(.leading, 6)
            }
            .padding(.horizontal, 8)
            .frame(width: geo.size.width, height: geo.size.height)
            .overlay(alignment: .bottomLeading) {
                if browser.loading {
                    Rectangle()
                        .fill(Color.accentColor)
                        .frame(width: geo.size.width * max(browser.progress, 0.05), height: 2)
                        .accessibilityHidden(true)
                }
            }
            .onChange(of: geo.size.width < Self.fullWidth, initial: true) { _, narrow in browser.narrow = narrow }
        }
        .frame(height: 34)
        .font(.body)
    }

    private func button(_ label: String, _ symbol: String, enabled: Bool, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .frame(width: 26, height: 24)
                .contentShape(Rectangle())
        }
        .buttonStyle(.borderless)
        .disabled(!enabled)
        .help(label)
        .accessibilityLabel(label)
    }
}

/// The address field. While the person is not editing it, it shows the page split the way a
/// reader scans it — the host or the file's name in full, the rest dimmed — and a click starts an
/// edit of the address as it is.
struct AddressField: View {
    @ObservedObject var browser: BrowserPane
    @FocusState private var focused: Bool

    static let placeholder = "Enter a web address or a file path"

    var body: some View {
        if let editing = browser.editing {
            TextField(Self.placeholder, text: Binding(get: { editing }, set: { browser.editing = $0 }))
                .textFieldStyle(.roundedBorder)
                .focused($focused)
                .onAppear { focused = true }
                .onSubmit { browser.open(typed: browser.editing ?? "") }
                .onExitCommand { browser.editing = nil }
                .onChange(of: focused) { _, now in if !now { browser.editing = nil } }
                .accessibilityLabel("Address")
        } else {
            Button {
                browser.editing = Self.editable(browser.address)
            } label: {
                shown
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .padding(.horizontal, 6)
                    .frame(maxWidth: .infinity, minHeight: 22, alignment: .leading)
                    .background(.background, in: RoundedRectangle(cornerRadius: 5))
                    .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(.separator))
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Address")
            .accessibilityValue(browser.address)
            .accessibilityHint("Edit the address")
        }
    }

    /// The address as the person edits it: a file as its path, a blank page as nothing.
    static func editable(_ address: String) -> String {
        switch BrowserPanes.shown(address) {
        case .blank: ""
        case let .file(folder, name): folder + name
        default: address
        }
    }

    @ViewBuilder private var shown: some View {
        switch BrowserPanes.shown(browser.address) {
        case .blank:
            Text(Self.placeholder).foregroundStyle(.tertiary)
        case let .web(secure, host, rest):
            HStack(spacing: 4) {
                if secure { Image(systemName: "lock.fill").font(.caption).foregroundStyle(.secondary) }
                Text(host) + Text(rest).foregroundStyle(.secondary)
            }
        case let .file(folder, name):
            HStack(spacing: 4) {
                Image(systemName: "doc").font(.caption).foregroundStyle(.secondary)
                Text(folder).foregroundStyle(.secondary) + Text(name)
            }
        case let .other(text):
            Text(text)
        }
    }
}

/// Why the page did not load, over the page, with a way to try the same address again.
struct BrowserFailure: View {
    let failure: BrowserPane.Failure
    let retry: () -> Void

    var body: some View {
        VStack(spacing: 6) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 26, weight: .light))
                .foregroundStyle(.secondary)
                .padding(.bottom, 4)
                .accessibilityHidden(true)
            Text("Can’t open this page").font(.body).fontWeight(.semibold)
            Text(failure.message).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
            Text(failure.address).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            Button("Try Again", action: retry).padding(.top, 8)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.background)
    }
}

/// A browser pane's title: the page's own, else its host or file name, dimmed, until it names
/// itself; "Browser" on a blank page.
struct BrowserTitle: View {
    @ObservedObject var browser: BrowserPane

    var body: some View {
        if !browser.title.isEmpty, browser.failure == nil {
            Text(browser.title)
        } else {
            Text(fallback).foregroundStyle(.secondary)
        }
    }

    private var fallback: String {
        switch BrowserPanes.shown(browser.address) {
        case .blank: "Browser"
        case let .web(_, host, _): host
        case let .file(_, name): name
        case let .other(text): text
        }
    }
}

/// The browser's own items at the top of its pane's ⋯ menu, at every width: the narrow chrome
/// row has no room for forward, reload and home.
struct BrowserMenu: View {
    @ObservedObject var model: AppModel
    let paneId: String
    @ObservedObject var browser: BrowserPane

    var body: some View {
        Button("Back", action: browser.back).disabled(!browser.canGoBack)
        Button("Forward", action: browser.forward).disabled(!browser.canGoForward)
        if browser.loading {
            Button("Stop Loading", action: browser.stop)
        } else {
            Button("Reload Page", action: browser.reload)
        }
        Button("Home") { model.goHome(paneId) }
    }
}
