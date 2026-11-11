import SwiftUI
import WebKit
import WheelKit
import WheelUI

/// Shows a browser pane's view where SwiftUI puts it.
struct BrowserView: NSViewRepresentable {
    let browser: BrowserPane

    func makeNSView(context: Context) -> WKWebView { browser.view }
    func updateNSView(_ view: WKWebView, context: Context) {}
}

/// A browser pane under its title row: the chrome, then the page or why it failed.
struct BrowserBody: View {
    var model: AppModel
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
    var model: AppModel
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

/// The browser's own items in its pane header's ⋯ menu, at every width: the narrow chrome
/// row has no room for forward, reload and home.
struct BrowserMenu: View {
    var model: AppModel
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
