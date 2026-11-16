import SwiftUI
import WebKit
import WheelKit
import WheelUI

/// One browser pane's page drawn by the phone itself (Phone mode, ADR-0005). The phone's web view
/// loads the page wheeld says the pane is on, reports every page it lands on with
/// `pane_browser_url`, and follows the pages someone else sends the pane to. A page on the Mac's
/// loopback loads under `BrowserPanes.loopbackScheme`, fetched from wheeld's `/loopback/<port>/`.
/// Agents reach the page with `--on phone`; each command they run shows in `banner` for a moment.
/// Before the page first lands on a site, the phone asks wheeld for the Mac's session of it
/// (cookies, and the storage of a Mac page on it) and installs it, unless sharing is off for the
/// pane; `session()` reads the phone's own for handing it back. What the page writes to its
/// console goes to wheeld as the phone's own (`wheel pane console --on phone`).
@MainActor
@Observable
final class PhoneBrowser: NSObject, WKNavigationDelegate {
    let pane: String
    @ObservationIgnored let view: WKWebView
    /// The page the address field shows: the last one the pane landed on or was sent to.
    private(set) var page: String = ""
    /// Whether the web view can go back.
    private(set) var canGoBack = false
    /// What an agent last ran on this page, while it shows: who ran it and the command with its
    /// argument ("claude" ran "dom h1").
    private(set) var banner: (by: String, command: String)?
    @ObservationIgnored private var echoes = BrowserPanes.Echoes()
    @ObservationIgnored private var reported: String?
    @ObservationIgnored private let report: (String) -> Void
    @ObservationIgnored private let relay = ConsoleRelay()
    @ObservationIgnored private let fetchSession: (String) async -> Result<PageSession, PageSessionRefusal>
    /// The origins whose session the phone asked for since this page was made: once each.
    @ObservationIgnored private var asked: Set<String> = []
    /// The one-time storage scripts of the sessions taken, kept while the cookie script is made
    /// again for each page.
    @ObservationIgnored private var storageScripts: [WKUserScript] = []
    /// The origins whose session the page took, each handed back when the pane goes to the Mac
    /// (a login through another site, such as single sign-on, spans two).
    @ObservationIgnored private var taken: [String] = [] {
        didSet { Self.setTaken(taken, pane: pane) }
    }

    /// The sites each pane took a session of while the phone drew it, kept across launches: a
    /// relaunch is no handover, so the phone does not take the Mac's session again over its own.
    /// Cleared when the pane goes to the Mac.
    static func takenSites(_ pane: String) -> [String] {
        (UserDefaults.standard.dictionary(forKey: "sessionTaken")?[pane] as? [String]) ?? []
    }

    static func setTaken(_ sites: [String], pane: String) {
        var all = UserDefaults.standard.dictionary(forKey: "sessionTaken") ?? [:]
        all[pane] = sites.isEmpty ? nil : sites
        UserDefaults.standard.set(all, forKey: "sessionTaken")
    }
    /// Counts `prepareScripts` calls, so one that waited on the store while a later navigation
    /// started leaves the later one's scripts alone.
    @ObservationIgnored private var scriptGeneration = 0
    @ObservationIgnored private var watches: [NSKeyValueObservation] = []
    @ObservationIgnored private var bannerGeneration = 0
    /// Why the last load failed, shown over the page until the next one starts.
    private(set) var failure: String?

    init(pane: String, daemon: PageDaemon, report: @escaping (String) -> Void,
         fetchSession: @escaping (String) async -> Result<PageSession, PageSessionRefusal>,
         console: @escaping (Bool, [ConsoleEntry]) -> Void) {
        self.pane = pane
        self.report = report
        self.fetchSession = fetchSession
        taken = Self.takenSites(pane)
        asked = Set(taken)
        let config = WKWebViewConfiguration()
        // The phone's own browser storage, shared by every browser pane it draws.
        config.websiteDataStore = .default()
        let handler = PageSchemeHandler(pane: pane, daemon: daemon, loopback: true)
        config.setURLSchemeHandler(handler, forURLScheme: BrowserPanes.loopbackScheme)
        view = WKWebView(frame: .zero, configuration: config)
        super.init()
        config.userContentController.add(CookieMessages(self), name: PageSessionStore.cookieHandler)
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
    }

    /// wheeld says the pane is on `url`: go there, unless it is the echo of a page this view
    /// reported itself.
    func daemonSays(_ url: String) {
        if echoes.isOwn(url) { return }
        guard url != page || view.url == nil else { return }
        load(url)
    }

    private func load(_ url: String) {
        echoes.adopt(url)
        page = url
        reported = url
        guard let target = BrowserPanes.phonePage(url) ?? URL(string: url) else {
            Log.info("browser pane \(pane) cannot load \(WebPanes.withoutQuery(URL(string: url) ?? URL(fileURLWithPath: "/")))")
            return
        }
        Log.info("browser pane \(pane) loading \(WebPanes.withoutQuery(target))")
        view.load(URLRequest(url: target))
    }

    func goBack() { view.goBack() }

    /// The web view is on `url` now: report the page it stands for when it is new.
    private func landed(_ url: URL?) {
        guard let url, url.absoluteString != "about:blank" else { return }
        let now = BrowserPanes.macPage(url)
        page = now
        guard now != reported else { return }
        reported = now
        echoes.sent(now)
        Log.info("browser pane \(pane) landed on \(WebPanes.withoutQuery(URL(string: now) ?? url))")
        report(now)
    }

    /// `inspect_pane` `op` with `arg`, run on this page, shown in the banner under `by`.
    func run(_ op: String, arg: String?, by: String?) async -> BrowserInspect.Outcome {
        let shown = show(by: by, ran: op, arg: arg)
        defer { hide(shown) }
        return await BrowserInspect.run(op, arg: arg, in: view)
    }

    /// The page as a PNG, the part in view or with `full` the whole document, for `render_pane`.
    func shot(full: Bool, by: String?) async -> Result<Data, BrowserInspect.Refusal> {
        let shown = show(by: by, ran: "shot", arg: nil)
        defer { hide(shown) }
        return await BrowserInspect.shotPage(view, full: full).flatMap { image in
            BrowserInspect.png(image).map(Result.success) ?? .failure("the drawing could not be encoded as a PNG")
        }
    }

    /// Show who runs which command for as long as it runs (a recording runs for its whole
    /// length); returns the banner's generation for `hide`.
    private func show(by: String?, ran op: String, arg: String?) -> Int {
        let command = [op, arg].compactMap { $0 }.joined(separator: " ")
        banner = (by ?? "An agent", command)
        Log.info("browser pane \(pane) \(by ?? "An agent") ran \(command) on this page")
        bannerGeneration += 1
        return bannerGeneration
    }

    /// The command shown as `generation` finished: its banner stays 4 seconds more, unless a
    /// later command replaced it.
    private func hide(_ generation: Int) {
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(4))
            if self?.bannerGeneration == generation { self?.banner = nil }
        }
    }

    /// Before the main frame first goes to a site, install the Mac's session of it, so the page
    /// loads logged in as it is on the Mac.
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
        // A page from elsewhere may link to the Mac's loopback page, but never post a form to it:
        // the request would reach the Mac's program with no Origin to refuse it by.
        if action.request.url?.scheme == BrowserPanes.loopbackScheme, (action.request.httpMethod ?? "GET") != "GET",
           action.sourceFrame.request.url.map({ BrowserPanes.loopbackRequestAllowed(action.request.url!, document: $0) }) != true {
            Log.info("browser pane \(pane) refused a \(action.request.httpMethod ?? "-") to the Mac's loopback from another site")
            return .cancel
        }
        guard action.targetFrame?.isMainFrame != false, let url = action.request.url else { return .allow }
        if let origin = PageSessions.origin(of: BrowserPanes.macPage(url)), !asked.contains(origin) {
            asked.insert(origin)
            switch await fetchSession(origin) {
            case let .success(session):
                await PageSessionStore.write(session, store: webView.configuration.websiteDataStore, page: nil)
                if !session.local.isEmpty || !session.session.isEmpty {
                    storageScripts.append(PageSessionStore.storageUserScript(session))
                }
                if !taken.contains(origin) { taken.append(origin) }
                Log.info("browser pane \(pane) took the Mac's session of \(origin): \(session.cookies.count) cookies, \(session.local.count + session.session.count) stored items")
            case let .failure(why):
                // Asked again at the next page: the Mac may answer then, or sharing be on.
                asked.remove(origin)
                Log.info("browser pane \(pane) has no session of \(origin) from the Mac: \(why.reason)")
            }
        }
        await prepareScripts(for: url)
        return .allow
    }

    /// The user scripts the page about to load at `url` starts with: the console relay, the
    /// storage of each session taken, and on the Mac's loopback a `document.cookie` from the
    /// store, which WebKit gives a custom scheme none of.
    /// This controller holds no other script: every one it runs is added here.
    private func prepareScripts(for url: URL) async {
        scriptGeneration += 1
        let generation = scriptGeneration
        var cookies: [(name: String, value: String)]?
        if url.scheme == BrowserPanes.loopbackScheme, let mac = URL(string: BrowserPanes.macPage(url)) {
            cookies = await PageSessionStore.scriptCookies(for: mac, store: view.configuration.websiteDataStore)
        }
        guard generation == scriptGeneration else { return }
        let scripts = view.configuration.userContentController
        scripts.removeAllUserScripts()
        scripts.addUserScript(WKUserScript(source: BrowserConsole.script, injectionTime: .atDocumentStart, forMainFrameOnly: false))
        storageScripts.forEach(scripts.addUserScript)
        if let cookies { scripts.addUserScript(PageSessionStore.cookieScript(cookies)) }
    }

    /// The store's cookies changed (a response set one, or the page wrote one): the page in view
    /// and the next one it loads read the store's.
    private func cookiesChanged() {
        guard let url = view.url, url.scheme == BrowserPanes.loopbackScheme, let mac = URL(string: BrowserPanes.macPage(url)) else { return }
        Task {
            let cookies = await PageSessionStore.scriptCookies(for: mac, store: view.configuration.websiteDataStore)
            _ = try? await view.evaluateJavaScript(PageSessionStore.refreshScript(cookies))
            await prepareScripts(for: url)
        }
    }

    /// The loopback page set `document.cookie = line`: keep it in the store, where the scheme
    /// handler's next request and a hand-back to the Mac find it.
    fileprivate func pageSetCookie(_ line: String) {
        guard let url = view.url, url.scheme == BrowserPanes.loopbackScheme, let mac = URL(string: BrowserPanes.macPage(url)) else { return }
        PageSessionStore.queueScriptCookie(line, for: mac, store: view.configuration.websiteDataStore) { [weak self] in
            self?.cookiesChanged()
        }
    }

    /// The phone's own session of every site the page took one of, and of the site it is on, to
    /// hand back to the Mac; storage only for the site in view, the only one a page can read.
    func sessions() async -> [PageSession] {
        var origins = taken
        if let url = view.url, let origin = PageSessions.origin(of: BrowserPanes.macPage(url)), !origins.contains(origin) { origins.append(origin) }
        var found: [PageSession] = []
        for origin in origins {
            found.append(await PageSessionStore.read(origin: origin, store: view.configuration.websiteDataStore, page: view))
        }
        return found
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

/// A browser pane on the phone: back, the address and which side draws it above the page; the
/// phone's own page in Phone mode, with a notice saying agents reach it with `--on phone` and a
/// banner naming each command an agent runs on it; in Mac mode, a placeholder for the Mac's page.
struct BrowserPage: View {
    let model: PhoneModel
    let pane: String
    let url: String
    @State private var editing = false
    @State private var typed = ""
    @FocusState private var fieldFocused: Bool

    private var browser: PhoneBrowser? { model.browsers[pane] }
    private var mode: String { model.browserMode(pane) }

    var body: some View {
        VStack(spacing: 0) {
            controls
            ZStack(alignment: .top) {
                if mode == "phone", let browser {
                    // The notice sits below the page, never over it, so the page's own bottom
                    // edge (a tab bar, a Submit button) stays reachable.
                    VStack(spacing: 0) {
                        PhoneBrowserBody(browser: browser)
                            .overlay {
                                if let failure = browser.failure {
                                    ContentUnavailableView("Could not open the page", systemImage: "exclamationmark.triangle",
                                                           description: Text(failure))
                                        .background(.background)
                                        .accessibilityIdentifier("browser-failure")
                                }
                            }
                        notice
                    }
                } else {
                    macPage
                }
                if let banner = browser?.banner {
                    Label {
                        Text("\(Text(banner.by).bold()) ran \(Text(verbatim: banner.command).monospaced()) on this page")
                    } icon: {
                        Image(systemName: "sparkle")
                    }
                        .font(.footnote)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
                        .padding(.horizontal, 8)
                        .padding(.top, 6)
                        .transition(.move(edge: .top).combined(with: .opacity))
                        .accessibilityIdentifier("browser-banner")
                }
            }
            .animation(.default, value: browser?.banner?.command)
        }
    }

    private var controls: some View {
        HStack(spacing: 8) {
            Button { browser?.goBack() } label: {
                Image(systemName: "chevron.left").frame(width: 32, height: 32)
            }
            .buttonStyle(.bordered)
            .buttonBorderShape(.circle)
            .disabled(mode != "phone" || browser?.canGoBack != true)
            .accessibilityLabel("Back")
            address
            Picker("Drawn by", selection: Binding(get: { mode }, set: { model.setBrowserMode(pane, $0) })) {
                Text("Mac").tag("mac")
                Text("Phone").tag("phone")
            }
            .pickerStyle(.segmented)
            .fixedSize()
            .accessibilityIdentifier("browser-mode")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
    }

    @ViewBuilder private var address: some View {
        let page = browser?.page.isEmpty == false ? browser!.page : url
        ZStack {
            if editing {
                TextField("Address", text: $typed)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.go)
                    .focused($fieldFocused)
                    .onSubmit {
                        editing = false
                        model.openBrowser(pane, typed)
                    }
                    .onChange(of: fieldFocused) { _, focused in if !focused { editing = false } }
            } else {
                Button {
                    typed = page
                    editing = true
                    fieldFocused = true
                } label: {
                    Text(shownAddress(page)).lineLimit(1).truncationMode(.middle)
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("browser-address")
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(.quaternary, in: RoundedRectangle(cornerRadius: 10))
    }

    private func shownAddress(_ page: String) -> String {
        switch BrowserPanes.shown(page) {
        case .blank: "Search or enter address"
        case let .web(_, host, _): host
        case let .file(_, name): name
        case let .other(text): text
        }
    }

    private var notice: some View {
        // The flag stays whole on one line: a non-breaking hyphen pair and space.
        HStack(spacing: 8) {
            Circle().fill(.white.opacity(0.6)).frame(width: 6, height: 6)
            Text("This phone's own page · agents reach it with \(Text(verbatim: "\u{2011}\u{2011}on\u{00A0}phone").monospaced())")
                .font(.footnote)
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(white: 0.25), in: RoundedRectangle(cornerRadius: 10))
        .padding(8)
        .accessibilityIdentifier("browser-notice")
    }

    private var macPage: some View {
        VStack(spacing: 8) {
            Image(systemName: "desktopcomputer").font(.largeTitle)
            Text("Drawn on the Mac").font(.title3)
            Text("Switch to Phone to load this page here.").font(.footnote).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.quaternary.opacity(0.4))
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
