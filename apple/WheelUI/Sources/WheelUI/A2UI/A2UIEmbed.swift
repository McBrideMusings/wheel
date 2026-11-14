import SwiftUI
import WebKit

/// A pane a page asks to show inside its own layout (ADR-0042): `Terminal{pane}` or `AgentChat{pane}`.
public struct A2UIPaneEmbed: Hashable, Sendable {
    public enum Kind: String, Sendable { case terminal, agentChat = "agent_chat" }
    public let kind: Kind
    /// The pane's id as wheeld names it (`p12`).
    public let pane: String
    public init(kind: Kind, pane: String) { self.kind = kind; self.pane = pane }
}

public extension EnvironmentValues {
    /// The hosting client's native view for an embedded pane. The host answers every pane of a kind it can show, with its own
    /// message for a pane that is gone; nil means this client cannot show that kind inside a page.
    @Entry var a2uiPaneEmbed: (@MainActor (A2UIPaneEmbed) -> AnyView?)? = nil
}

/// `Terminal{pane, height?}` and `AgentChat{pane, height?}`: the host's view of that pane, `height` points tall (default 320).
struct A2UIPaneEmbedView: View {
    let kind: A2UIPaneEmbed.Kind
    let node: ComponentNode
    @Environment(\.a2uiPaneEmbed) private var embed
    var body: some View {
        let p = node.instance.properties
        let pane = p["pane"]?.stringValue ?? ""
        let height = p["height"]?.numberValue ?? 320
        Group {
            if pane.isEmpty {
                A2UIEmbedNote(text: "\(kind == .terminal ? "Terminal" : "AgentChat") names no pane")
            } else if let view = embed?(A2UIPaneEmbed(kind: kind, pane: pane)) {
                view
            } else {
                A2UIEmbedNote(text: "This client cannot show \(kind == .terminal ? "a terminal" : "an agent chat") inside a page")
            }
        }
        .frame(maxWidth: .infinity)
        .frame(height: height)
        .clipShape(Rectangle())
        .accessibilityIdentifier("a2ui.\(kind.rawValue).\(pane)")
    }
}

/// `WebView{url, height?}`: WebKit's view of an http or https address, `height` points tall (default 320). Links inside it
/// navigate in place and only to http or https; another `url` loads that address.
struct A2UIWebViewView: View {
    let node: ComponentNode
    var body: some View {
        let p = node.instance.properties
        let raw = p["url"]?.stringValue ?? ""
        let height = p["height"]?.numberValue ?? 320
        Group {
            if let url = URL(string: raw), ["http", "https"].contains(url.scheme?.lowercased() ?? "") {
                EmbeddedWebView(url: url)
            } else {
                A2UIEmbedNote(text: raw.isEmpty ? "WebView names no url" : "WebView shows only http and https addresses, not \(raw)")
            }
        }
        .frame(maxWidth: .infinity)
        .frame(height: height)
        .clipShape(Rectangle())
        .accessibilityIdentifier("a2ui.webview")
    }
}

/// Why an embed shows nothing, in the place it would have filled.
struct A2UIEmbedNote: View {
    let text: String
    var body: some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundStyle(.secondary)
            .multilineTextAlignment(.center)
            .padding(8)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.secondary.opacity(0.5), style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
    }
}

/// One WKWebView, loading `url` when it first shows and whenever the page names another. Its navigation stays on http and
/// https: a link or redirect to any other scheme is cancelled, and a link that asks for a new window opens in place.
@MainActor
final class EmbeddedWebLoader: NSObject, WKNavigationDelegate, WKUIDelegate {
    var loaded: URL?
    func make(_ url: URL) -> WKWebView {
        let web = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        web.navigationDelegate = self
        web.uiDelegate = self
        load(url, in: web)
        return web
    }
    func load(_ url: URL, in web: WKWebView) {
        guard loaded != url else { return }
        loaded = url
        web.load(URLRequest(url: url))
    }
    static func allowed(_ url: URL?) -> Bool { ["http", "https", "about"].contains(url?.scheme?.lowercased() ?? "") }
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        Self.allowed(navigationAction.request.url) ? .allow : .cancel
    }
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration, for navigationAction: WKNavigationAction,
                 windowFeatures: WKWindowFeatures) -> WKWebView? {
        if Self.allowed(navigationAction.request.url) { webView.load(navigationAction.request) }
        return nil
    }
}

#if os(macOS)
struct EmbeddedWebView: NSViewRepresentable {
    let url: URL
    func makeCoordinator() -> EmbeddedWebLoader { EmbeddedWebLoader() }
    func makeNSView(context: Context) -> WKWebView { context.coordinator.make(url) }
    func updateNSView(_ web: WKWebView, context: Context) { context.coordinator.load(url, in: web) }
}
#else
struct EmbeddedWebView: UIViewRepresentable {
    let url: URL
    func makeCoordinator() -> EmbeddedWebLoader { EmbeddedWebLoader() }
    func makeUIView(context: Context) -> WKWebView { context.coordinator.make(url) }
    func updateUIView(_ web: WKWebView, context: Context) { context.coordinator.load(url, in: web) }
}
#endif
