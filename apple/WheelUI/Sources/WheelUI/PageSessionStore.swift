import Foundation
import WebKit
import WheelKit

/// Reads and writes one site's stored data in a web view's browser storage, for a browser pane's
/// session to follow it between the Mac and a phone (`pane_session_get`, `pane_session_put`):
/// the cookies `WKHTTPCookieStore` holds for the site, HttpOnly ones included, and the
/// localStorage and sessionStorage of a page open on it. Live page state (memory, the screen a
/// game is on) is not stored data and never moves.
@MainActor
public enum PageSessionStore {
    /// The session of `origin` (`https://example.com`, `http://localhost:5173`) in `store`: every
    /// cookie a request to its host carries, and the storage of `page` when it is open on that
    /// origin (none otherwise, since no WebKit API reads a site's storage from outside a page).
    public static func read(origin: String, store: WKWebsiteDataStore, page: WKWebView?) async -> PageSession {
        var session = PageSession(origin: origin)
        guard let host = PageSessions.host(of: origin) else { return session }
        let all = await store.httpCookieStore.allCookies()
        session.cookies = all.filter { PageSessions.carries($0.domain, to: host) }.map(PageSessions.cookie)
        if let page, let url = page.url, PageSessions.sameOrigin(BrowserPanes.macPage(url), origin),
           let value = try? await page.evaluateJavaScript(PageSessions.snapshotScript) as? String,
           let storage = PageSessions.storage(fromSnapshot: value) {
            session.local = storage.local
            session.session = storage.session
        }
        return session
    }

    /// Install `session` in `store`: its cookies now, and its storage into `page` when it is open
    /// on the session's origin, which a page reads at its next load.
    /// Only cookies a request to the session's origin carries are installed, so another device
    /// cannot plant one for any other site, and the store's cookies for that origin the session
    /// lacks are deleted, so a logout on one device follows the pane.
    public static func write(_ session: PageSession, store: WKWebsiteDataStore, page: WKWebView?) async {
        guard let host = PageSessions.host(of: session.origin) else { return }
        let given = session.cookies.filter { PageSessions.carries($0.domain, to: host) }.compactMap(PageSessions.httpCookie)
        // The session is the site's cookies whole: one the sender no longer holds (a logout)
        // goes here too.
        let kept = Set(given.map { "\($0.name)\u{0}\($0.domain.lowercased())\u{0}\($0.path)" })
        for old in await store.httpCookieStore.allCookies() where PageSessions.carries(old.domain, to: host)
            && !kept.contains("\(old.name)\u{0}\(old.domain.lowercased())\u{0}\(old.path)") {
            await store.httpCookieStore.deleteCookie(old)
        }
        for cookie in given {
            await store.httpCookieStore.setCookie(cookie)
        }
        guard !session.local.isEmpty || !session.session.isEmpty,
              let page, let url = page.url, PageSessions.sameOrigin(BrowserPanes.macPage(url), session.origin) else { return }
        _ = try? await page.evaluateJavaScript(PageSessions.storageScript(session, mark: UUID().uuidString))
    }

    /// A script that writes `session`'s storage into the first page of its origin a web view
    /// loads, before the page's own scripts run, once per tab: add it to the web view's user
    /// scripts before loading.
    public static func storageUserScript(_ session: PageSession) -> WKUserScript {
        WKUserScript(source: PageSessions.storageScript(session, mark: UUID().uuidString), injectionTime: .atDocumentStart, forMainFrameOnly: true)
    }

    /// The `Cookie` header a request to `url` carries from `store`, for a scheme handler that
    /// fetches the page itself (WebKit adds none to a custom scheme's requests); nil when none.
    public static func cookieHeader(for url: URL, store: WKWebsiteDataStore) async -> String? {
        await scriptWrites?.value
        guard let host = url.host?.lowercased() else { return nil }
        let path = url.path.isEmpty ? "/" : url.path
        let carried = await store.httpCookieStore.allCookies().filter {
            PageSessions.carries($0.domain, to: host) && PageSessions.pathMatches($0.path, path) && ($0.expiresDate.map { $0 > Date() } ?? true)
        }
        guard !carried.isEmpty else { return nil }
        return carried.map { "\($0.name)=\($0.value)" }.joined(separator: "; ")
    }

    /// The name of the script message handler `cookieScript`'s setter posts to.
    public static let cookieHandler = "wheelCookie"
    /// The page function `cookieScript` defines that replaces the page's cookies with the store's.
    static let jarSetter = "__wheelSetCookies"

    /// Script that replaces the page's `document.cookie` with `cookies`, after the store changed
    /// (a response set a cookie, or the store refused one the page wrote).
    public static func refreshScript(_ cookies: [(name: String, value: String)]) -> String {
        let pairs = (try? JSONSerialization.data(withJSONObject: cookies.map { [$0.name, $0.value] })).flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
        return "window.\(jarSetter) && window.\(jarSetter)(\(pairs)); 0"
    }

    /// The script cookie writes not yet in the store, in order; a request waits for them.
    private static var scriptWrites: Task<Void, Never>?

    /// Queue `keepScriptCookie(line, for: url, store:)` behind the writes before it, so a request
    /// the page sends right after `document.cookie = …` carries the cookie; `done` runs after.
    public static func queueScriptCookie(_ line: String, for url: URL, store: WKWebsiteDataStore, done: @escaping @MainActor () async -> Void) {
        let before = scriptWrites
        scriptWrites = Task { @MainActor in
            await before?.value
            await keepScriptCookie(line, for: url, store: store)
            await done()
        }
    }

    /// A script that gives a page under a custom scheme the `document.cookie` WebKit gives it none
    /// of: reading returns `cookies` (those `store` holds for the page, HttpOnly ones left out, as
    /// `scriptCookies` reads them) and what the page set since; writing updates that and posts the
    /// line to `cookieHandler`, whose owner keeps it with `keepScriptCookie`. Add it before the
    /// page loads, made from the store as it is then.
    public static func cookieScript(_ cookies: [(name: String, value: String)]) -> WKUserScript {
        let pairs = cookies.map { [$0.name, $0.value] }
        let initial = (try? JSONSerialization.data(withJSONObject: pairs)).flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
        let source = """
        (() => {
          const jar = new Map(\(initial));
          Object.defineProperty(window, '\(jarSetter)', {
            value: (pairs) => { jar.clear(); for (const [k, v] of pairs) jar.set(k, v); },
          });
          Object.defineProperty(Document.prototype, 'cookie', {
            configurable: true,
            get() { return [...jar].map(([k, v]) => k + '=' + v).join('; '); },
            set(line) {
              const text = String(line);
              const parts = text.split(';');
              const first = parts[0], eq = first.indexOf('=');
              const name = (eq < 0 ? '' : first.slice(0, eq)).trim();
              const value = (eq < 0 ? first : first.slice(eq + 1)).trim();
              const attrs = parts.slice(1).map((a) => a.trim().toLowerCase());
              const gone = attrs.some((a) => /^max-age=(0|-)/.test(a)
                || (a.startsWith('expires=') && Date.parse(a.slice(8)) <= Date.now()));
              if (gone) jar.delete(name); else jar.set(name, value);
              try { window.webkit.messageHandlers.\(cookieHandler).postMessage(text); } catch (e) {}
            },
          });
        })();
        """
        return WKUserScript(source: source, injectionTime: .atDocumentStart, forMainFrameOnly: true)
    }

    /// The cookies a page's script sees at `url` (an http URL, the Mac's page): those a request
    /// there carries, minus HttpOnly ones.
    public static func scriptCookies(for url: URL, store: WKWebsiteDataStore) async -> [(name: String, value: String)] {
        guard let host = url.host?.lowercased() else { return [] }
        let path = url.path.isEmpty ? "/" : url.path
        return await store.httpCookieStore.allCookies().filter {
            !$0.isHTTPOnly && PageSessions.carries($0.domain, to: host) && PageSessions.pathMatches($0.path, path)
                && ($0.expiresDate.map { $0 > Date() } ?? true)
        }.map { ($0.name, $0.value) }
    }

    /// Keep a cookie a page's script set (`document.cookie = line`) at `url` (an http URL, the
    /// Mac's page) in `store`, as a browser does: never HttpOnly, and never over an HttpOnly
    /// cookie of the same name.
    public static func keepScriptCookie(_ line: String, for url: URL, store: WKWebsiteDataStore) async {
        guard let host = url.host?.lowercased() else { return }
        let attributes = line.split(separator: ";").dropFirst().filter {
            $0.trimmingCharacters(in: .whitespaces).lowercased() != "httponly"
        }
        let cleaned = ([line.split(separator: ";").first.map(String.init) ?? ""] + attributes.map(String.init)).joined(separator: ";")
        for cookie in HTTPCookie.cookies(withResponseHeaderFields: ["Set-Cookie": cleaned], for: url) {
            guard PageSessions.carries(cookie.domain, to: host) else { continue }
            let held = await store.httpCookieStore.allCookies()
            if held.contains(where: { $0.isHTTPOnly && $0.name == cookie.name && PageSessions.carries($0.domain, to: host) }) { continue }
            if let expires = cookie.expiresDate, expires <= Date() {
                for old in held where old.name == cookie.name && old.domain == cookie.domain && old.path == cookie.path {
                    await store.httpCookieStore.deleteCookie(old)
                }
            } else {
                await store.httpCookieStore.setCookie(cookie)
            }
        }
    }

    /// Keep the cookies a scheme handler's response for `url` sets (`Set-Cookie`) in `store`, as
    /// WebKit does for an http response.
    public static func keepCookies(_ setCookie: [String], for url: URL, store: WKWebsiteDataStore) async {
        guard let host = url.host, !setCookie.isEmpty, let http = URL(string: "http://\(host)\(url.path.isEmpty ? "/" : url.path)") else { return }
        for line in setCookie {
            for cookie in HTTPCookie.cookies(withResponseHeaderFields: ["Set-Cookie": line], for: http) {
                await store.httpCookieStore.setCookie(cookie)
            }
        }
    }
}
