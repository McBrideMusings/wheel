import SwiftUI
import WebKit
import WheelKit
import WheelUI

/// One browser pane's page drawn by the phone itself (Phone mode, ADR-0005). The phone's web view
/// loads the page wheeld says the pane is on, reports every page it lands on with
/// `pane_browser_url`, and follows the pages someone else sends the pane to. A page on the Mac's
/// loopback loads under `BrowserPanes.loopbackScheme`, fetched from wheeld's `/loopback/<port>/`.
/// Agents reach the page with `--on phone`; each command they run shows in `banner` for a moment.
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
    @ObservationIgnored private var watches: [NSKeyValueObservation] = []
    @ObservationIgnored private var bannerGeneration = 0

    init(pane: String, daemon: PageDaemon, report: @escaping (String) -> Void) {
        self.pane = pane
        self.report = report
        let config = WKWebViewConfiguration()
        // The phone's own browser storage, shared by every browser pane it draws.
        config.websiteDataStore = .default()
        config.setURLSchemeHandler(PageSchemeHandler(pane: pane, daemon: daemon, loopback: true), forURLScheme: BrowserPanes.loopbackScheme)
        view = WKWebView(frame: .zero, configuration: config)
        super.init()
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
        show(by: by, ran: op, arg: arg)
        return await BrowserInspect.run(op, arg: arg, in: view)
    }

    /// The page as a PNG, the part in view or with `full` the whole document, for `render_pane`.
    func shot(full: Bool, by: String?) async -> Result<Data, BrowserInspect.Refusal> {
        show(by: by, ran: "shot", arg: nil)
        return await BrowserInspect.shotPage(view, full: full).flatMap { image in
            BrowserInspect.png(image).map(Result.success) ?? .failure("the drawing could not be encoded as a PNG")
        }
    }

    private func show(by: String?, ran op: String, arg: String?) {
        let command = [op, arg].compactMap { $0 }.joined(separator: " ")
        banner = (by ?? "An agent", command)
        Log.info("browser pane \(pane) \(by ?? "An agent") ran \(command) on this page")
        bannerGeneration += 1
        let generation = bannerGeneration
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(4))
            if self?.bannerGeneration == generation { self?.banner = nil }
        }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        Log.info("browser pane \(pane) loaded")
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        Log.info("browser pane \(pane) failed to load: \(WebPanes.describe(error))")
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        Log.info("browser pane \(pane) failed to load: \(WebPanes.describe(error))")
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
                    PhoneBrowserBody(browser: browser)
                        .overlay(alignment: .bottom) { notice }
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
