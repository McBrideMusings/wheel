import Foundation
import WebKit
import WheelKit

/// Where the daemon is and the token it wants; set at each connect and shared by every web pane.
/// `pairing` is set when the daemon is a remote listener with a self-signed certificate, so the
/// handler's requests pin it.
@MainActor
public final class PageDaemon {
    public var url: URL?
    public var token: String?
    public var pairing: Pairing?

    public init() {}
}

/// Lets at most `limit` page requests run at once and holds the rest in order. The daemon's remote
/// listener refuses a client address its 16th open connection (`MAX_PER_PEER`), the phone already
/// holds one per terminal, chat and live stream, and a page opens one more for each file it asks
/// for: with every page request let through, a page of ten images breaks the ones that arrive
/// past the limit. One gate is shared by every web pane, since the limit is the phone's.
@MainActor
public final class RequestGate {
    private var free: Int
    private var waiting: [CheckedContinuation<Void, Never>] = []

    public init(limit: Int) { free = limit }

    public func enter() async {
        if free > 0 {
            free -= 1
            return
        }
        await withCheckedContinuation { waiting.append($0) }
    }

    /// Hands the place to the next in line, or frees it.
    public func leave() {
        if waiting.isEmpty { free += 1 } else { waiting.removeFirst().resume() }
    }
}

/// Answers one web pane's requests under `WebPanes.scheme` by fetching them from the daemon with the
/// token in a header, for that pane's own folder only. The page's origin is the scheme and a fixed
/// host, so its `localStorage` stays with it when the daemon comes back on another port. Made with
/// `loopback`, it answers a browser pane's requests under `BrowserPanes.loopbackScheme` instead,
/// from the daemon's `/loopback/<port>/`, which reaches the Mac's loopback page the pane is on and
/// any other loopback port wheeld forwards, a request to another port held to CORS (`LoopbackCORS`).
@MainActor
public final class PageSchemeHandler: NSObject, WKURLSchemeHandler {
    private let pane: String
    private let daemon: PageDaemon
    private let loopback: Bool
    /// Page requests in flight across every pane; the event streams never count, since they stay open.
    static let gate = RequestGate(limit: 4)
    /// Told the status of each page the pane's main frame loads.
    public var onDocument: ((Int) -> Void)?
    /// A loopback response came back (it may have set cookies): its owner refreshes the page's
    /// `document.cookie` from the store.
    public var onCookies: (() -> Void)?

    /// The Mac's page a wheeld `/loopback/<port>/<path>?<query>` URL stands for, on the host
    /// `page` (a `wheel-loopback` page) names.
    static func macPage(of page: URL) -> @Sendable (URL) -> URL? {
        let host = page.host?.lowercased() ?? "localhost"
        return { url in
            var c = URLComponents(url: url, resolvingAgainstBaseURL: false)
            let parts = (c?.percentEncodedPath ?? "").split(separator: "/", maxSplits: 2, omittingEmptySubsequences: false)
            guard parts.count >= 3, parts[1] == "loopback", let port = Int(parts[2].split(separator: "/").first ?? "") else { return nil }
            let rest = parts[2].drop(while: { $0 != "/" })
            c?.scheme = "http"
            c?.host = host
            c?.port = port
            c?.percentEncodedPath = rest.isEmpty ? "/" : String(rest)
            return c?.url
        }
    }

    public init(pane: String, daemon: PageDaemon, loopback: Bool = false) {
        self.pane = pane
        self.daemon = daemon
        self.loopback = loopback
    }

    /// A remote daemon's connection pins its certificate through Network.framework, because App
    /// Transport Security refuses URLSession `https://` to a non-loopback address with a
    /// self-signed certificate (-1200). A loopback daemon uses a plain URLSession.
    private enum Transport {
        case plain(URLSession)
        case pinned(PinnedHTTP)

        var isPinned: Bool {
            if case .pinned = self { return true }
            return false
        }
    }

    private var pinned: PinnedHTTP?
    private lazy var plain: URLSession = {
        // No cookie jar and no cache: the token travels in a header, and a reload after a file
        // changed must reach the daemon.
        let config = URLSessionConfiguration.ephemeral
        config.httpCookieStorage = nil
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: config)
    }()

    private func currentTransport() -> Transport {
        guard let pairing = daemon.pairing else { return .plain(plain) }
        if let pinned, pinned.pinnedTo == pairing { return .pinned(pinned) }
        let client = PinnedHTTP(pairing: pairing) {
            Task { @MainActor in UILog.info("web page request refused: certificate does not match the pinned fingerprint") }
        }
        pinned = client
        return .pinned(client)
    }

    private struct Reply {
        let status: Int
        let headers: [String: String]
        let body: AsyncThrowingStream<Data, Error>
    }

    /// The cookies of a loopback page's requests: `store` holds them as the Mac's page would
    /// (http, its own host and path), and `mac` turns a wheeld `/loopback/<port>/…` URL into the
    /// Mac's page it stands for. `credentials` is false when the page asked for none (WebKit's
    /// `httpShouldHandleCookies`: `credentials: 'omit'`, or a cross-origin request without
    /// `'include'`); such a request carries no cookie and keeps none it is sent.
    /// `context` is how the request stands to the page that sends it, which each cookie's
    /// `SameSite` is weighed against.
    struct Jar {
        let store: WKWebsiteDataStore
        let mac: @Sendable (URL) -> URL?
        let credentials: Bool
        let context: CookieMatch.Context
    }

    /// A request to another origin than its page's, held to CORS on every hop: the page's Mac
    /// `origin`, whether it asked for `credentials`, and the `headers` the page set.
    struct Cross {
        let origin: String
        let credentials: Bool
        var headers: [String: String]
    }

    /// Why CORS refused a request, as phone.log says it.
    struct Refusal: LocalizedError {
        let why: String
        var errorDescription: String? { why }
    }

    /// Sends `request` and returns its head with the body as a stream. A streaming response
    /// (the page's event stream) arrives a line at a time; any other body whole. With `jar` (a
    /// loopback page) every hop of a redirect carries the store's cookies for its page and keeps
    /// the ones it sets, as a browser does, so a login that answers 302 with its cookie stays.
    /// With `cross`, each hop that is not a simple request is sent only after its own preflight
    /// allows it (WebKit sends none for a custom scheme), and each redirect passes the CORS check
    /// before it is followed, as the Fetch standard's redirect fetch runs them again.
    @MainActor
    private static func fetch(_ request: URLRequest, over transport: Transport, streaming: Bool, jar: Jar?, cross: Cross?) async throws -> Reply {
        if jar == nil, cross == nil, case .plain = transport { return try await open(request, over: transport, streaming: streaming, follow: true) }
        // URLSession follows redirects itself unless told not to (the daemon sends a 308 for a
        // folder without its trailing slash); here they are followed to the same host only.
        var next = request
        var cross = cross
        for _ in 0..<6 {
            if let cross, let url = next.url {
                let method = next.httpMethod ?? "GET"
                if LoopbackCORS.needsPreflight(method: method, headers: cross.headers) {
                    // A preflight is never redirected and carries no cookie.
                    var preflight = URLRequest(url: url)
                    preflight.httpMethod = "OPTIONS"
                    for (name, value) in LoopbackCORS.preflightHeaders(method: method, headers: cross.headers) {
                        preflight.setValue(value, forHTTPHeaderField: name)
                    }
                    preflight.setValue("*/*", forHTTPHeaderField: "Accept")
                    for name in ["Authorization", LoopbackCORS.originHeader, LoopbackCORS.hostHeader] {
                        preflight.setValue(next.value(forHTTPHeaderField: name), forHTTPHeaderField: name)
                    }
                    let answer = try await open(preflight, over: transport, streaming: false, follow: false)
                    for try await _ in answer.body {}
                    if let why = LoopbackCORS.preflightRefusal(status: answer.status, answer: answer.headers, origin: cross.origin,
                                                               method: method, headers: cross.headers, credentials: cross.credentials) {
                        throw Refusal(why: "refused by its preflight: \(why)")
                    }
                }
            }
            if let jar, jar.credentials, let url = next.url, let page = jar.mac(url) {
                next.setValue(await LoopbackCookies.cookieHeader(for: page, in: jar.context, store: jar.store), forHTTPHeaderField: "Cookie")
            }
            let head = try await open(next, over: transport, streaming: streaming, follow: false)
            if let jar, jar.credentials, let url = next.url, let page = jar.mac(url),
               let set = head.headers.first(where: { $0.key.lowercased() == "set-cookie" })?.value {
                await LoopbackCookies.keepCookies([set], for: page, store: jar.store)
            }
            guard [301, 302, 303, 307, 308].contains(head.status),
                  let location = head.headers.first(where: { $0.key.lowercased() == "location" })?.value,
                  let from = next.url, let target = URL(string: location, relativeTo: from)?.absoluteURL,
                  target.host() == from.host(), target.port == from.port else {
                return head
            }
            if let cross, let why = LoopbackCORS.originRefusal(answer: head.headers, origin: cross.origin, credentials: cross.credentials) {
                throw Refusal(why: "refused at its redirect: \(why)")
            }
            next.url = target
            if head.status == 303 || ((head.status == 301 || head.status == 302) && next.httpMethod == "POST") {
                next.httpMethod = "GET"
                next.httpBody = nil
                for name in ["Content-Type", "Content-Encoding", "Content-Language", "Content-Location"] {
                    next.setValue(nil, forHTTPHeaderField: name)
                }
                if let kept = cross?.headers { cross?.headers = LoopbackCORS.withoutBody(kept) }
            }
        }
        throw URLError(.httpTooManyRedirects)
    }

    /// One request and its answer, a redirect included unless `follow` (URLSession only).
    private static func open(_ request: URLRequest, over transport: Transport, streaming: Bool, follow: Bool) async throws -> Reply {
        switch transport {
        case let .pinned(client):
            let (head, body) = try await client.open(request)
            return Reply(status: head.status, headers: head.headers, body: body)
        case let .plain(session):
            let delegate: URLSessionTaskDelegate? = follow ? nil : Unredirected()
            let response: URLResponse
            let body: AsyncThrowingStream<Data, Error>
            if streaming {
                let (bytes, answered) = try await session.bytes(for: request, delegate: delegate)
                response = answered
                body = AsyncThrowingStream { continuation in
                    let work = Task {
                        do {
                            var line = Data()
                            for try await byte in bytes {
                                line.append(byte)
                                if byte == 0x0A {
                                    continuation.yield(line)
                                    line.removeAll(keepingCapacity: true)
                                }
                            }
                            if !line.isEmpty { continuation.yield(line) }
                            continuation.finish()
                        } catch {
                            continuation.finish(throwing: error)
                        }
                    }
                    continuation.onTermination = { _ in work.cancel() }
                }
            } else {
                let (data, answered) = try await session.data(for: request, delegate: delegate)
                response = answered
                body = AsyncThrowingStream { continuation in
                    continuation.yield(data)
                    continuation.finish()
                }
            }
            guard let http = response as? HTTPURLResponse else { throw URLError(.badServerResponse) }
            var headers: [String: String] = [:]
            for (name, value) in http.allHeaderFields {
                if let name = name as? String, let value = value as? String { headers[name] = value }
            }
            return Reply(status: http.statusCode, headers: headers, body: body)
        }
    }
    private var running: [ObjectIdentifier: (serial: Int, work: Task<Void, Never>)] = [:]
    private var serial = 0
    /// The validators of the last answer for each loopback page, to tell the headers WebKit adds
    /// when it revalidates one (`LoopbackCORS.pageHeaders`) from the page's own.
    private var validators: [URL: LoopbackCORS.Validators] = [:]
    /// The page that started each loopback navigation not started by the browser itself, for the
    /// `SameSite` of the navigation's cookies; `navigation(to:from:)` records it.
    private var initiators: [URL: URL] = [:]

    /// The main frame is about to load `url`, started by the page `from`, or by the browser itself
    /// (an address typed, an agent's `open`, back or reload) when nil.
    public func navigation(to url: URL, from: URL?) {
        initiators[url] = from
    }

    /// Headers the page's own request carries that the daemon must not see: any `Origin` makes it
    /// refuse the request as a browser's, so a loopback page's goes in `LoopbackCORS.originHeader`,
    /// which the page itself never sets.
    private static let dropped: Set<String> = ["origin", "referer", "host", "cookie", "authorization",
                                               LoopbackCORS.originHeader.lowercased(), LoopbackCORS.hostHeader.lowercased()]

    /// WebKit hands a request's body over as a stream.
    private static func drain(_ stream: InputStream) -> Data {
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 16384)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count <= 0 { break }
            data.append(buffer, count: count)
        }
        return data
    }

    public func webView(_ webView: WKWebView, start task: any WKURLSchemeTask) {
        let key = ObjectIdentifier(task)
        let page = task.request.url
        guard let page, let address = daemon.url, let token = daemon.token else {
            UILog.info("web pane \(pane) request refused: no daemon address or token yet")
            task.didFailWithError(URLError(.cannotConnectToHost))
            return
        }
        let origin = task.request.value(forHTTPHeaderField: "Origin")
        let method = task.request.httpMethod ?? "GET"
        if loopback, !BrowserPanes.loopbackRequestAllowed(page, document: task.request.mainDocumentURL, origin: origin, method: method) {
            UILog.info("browser pane \(pane) request refused: \(WebPanes.withoutQuery(page)) asked by a page not on the Mac's loopback (origin \(origin ?? "none"))")
            task.didFailWithError(URLError(.noPermissionsToReadFile))
            return
        }
        // An artifact's page reaches its own folder and its pane's data directory; a loopback page
        // reaches the Mac's loopback port it is on. Nothing else the token opens.
        let target = loopback
            ? BrowserPanes.loopbackDaemonURL(for: page, daemon: address)
            : WebPanes.daemonURL(for: page, daemon: address, pane: pane)
        guard let target else {
            UILog.info("web pane \(pane) request refused outside its folder: \(WebPanes.withoutQuery(page))")
            task.didFailWithError(URLError(.noPermissionsToReadFile))
            return
        }
        var request = URLRequest(url: target)
        request.httpMethod = task.request.httpMethod
        request.httpBody = task.request.httpBody ?? task.request.httpBodyStream.map(Self.drain)
        for (name, value) in task.request.allHTTPHeaderFields ?? [:] where !Self.dropped.contains(name.lowercased()) {
            request.setValue(value, forHTTPHeaderField: name)
        }
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        // The page's origin as the Mac names it, for the program to see as `Origin`; a request to
        // another origin than the page's is cross-origin, and one that is not simple waits for
        // the program to allow it in a preflight, which WebKit does not send for a custom scheme.
        // The program sees the host the page named (`localhost`, `127.0.0.1`) as the Mac's browser
        // sends it.
        if loopback, let host = page.host?.lowercased() { request.setValue(host, forHTTPHeaderField: LoopbackCORS.hostHeader) }
        let cors = loopback ? origin.flatMap { o in LoopbackCORS.macOrigin(o).map { (page: o, mac: $0) } } : nil
        if let cors { request.setValue(cors.mac, forHTTPHeaderField: LoopbackCORS.originHeader) }
        let credentials = task.request.httpShouldHandleCookies
        let asked = LoopbackCORS.pageHeaders(task.request.allHTTPHeaderFields ?? [:], revalidating: validators[page])
        let cross = cors.flatMap { cors in
            LoopbackCORS.pageOrigin(of: page).flatMap(LoopbackCORS.macOrigin) == cors.mac
                ? nil : Cross(origin: cors.mac, credentials: credentials, headers: asked)
        }
        // `SameSite`: a subresource weighs its site against the main document's; a top-level
        // navigation against the page that started it, or none when the browser did.
        let document = task.request.mainDocumentURL
        let navigating = document == page
        let from: URL? = navigating ? (initiators.removeValue(forKey: page) ?? page) : document
        let context: CookieMatch.Context = from.flatMap(LoopbackCORS.site(of:)) == LoopbackCORS.site(of: page)
            ? .sameSite : (navigating && method == "GET" ? .crossSiteNavigation : .crossSite)
        let shown = WebPanes.withoutQuery(page)
        let pane = pane
        let transport = currentTransport()
        let sent = request
        // A loopback page's cookies live in the web view's store as the Mac's page would hold them
        // (http, same host and path); WebKit adds none to a custom scheme's request nor keeps the
        // ones its response sets, so the handler does both.
        let jar = loopback && target.path.hasPrefix("/loopback/")
            ? Jar(store: webView.configuration.websiteDataStore, mac: Self.macPage(of: page), credentials: credentials, context: context) : nil
        serial += 1
        let mine = serial
        let work = Task { @MainActor [weak self] in
            defer { if self?.running[key]?.serial == mine { self?.running.removeValue(forKey: key) } }
            do {
                // The page's event stream, and any `EventSource` an app's page opens (it asks
                // for `text/event-stream`), never completes, so it is handed over as it arrives;
                // every other response is held whole so its length is known.
                let streaming = target.path.hasSuffix("/.wheel/events")
                    || (sent.value(forHTTPHeaderField: "Accept") ?? "").contains("text/event-stream")
                if !streaming { await Self.gate.enter() }
                defer { if !streaming { Self.gate.leave() } }
                guard !Task.isCancelled else { return }
                UILog.info("web pane \(pane) page \(shown) requested \(transport.isPinned ? "through the pinned connection" : "over URLSession")")
                let reply: Reply
                do {
                    reply = try await Self.fetch(sent, over: transport, streaming: streaming, jar: jar, cross: cross)
                } catch let refusal as Refusal {
                    guard !Task.isCancelled else { return }
                    UILog.info("browser pane \(pane) \(method) \(shown) from \(cross?.origin ?? "-") \(refusal.why)")
                    task.didFailWithError(URLError(.noPermissionsToReadFile))
                    return
                }
                if self?.loopback == true, let kept = LoopbackCORS.Validators(answer: reply.headers) {
                    if self?.validators.count ?? 0 >= 512 { self?.validators.removeAll() }
                    self?.validators[page] = kept
                }
                var held = Data()
                if !streaming {
                    for try await chunk in reply.body { held.append(chunk) }
                }
                guard !Task.isCancelled else { return }
                var headers = cors.map { LoopbackCORS.pageAnswer(reply.headers, origin: $0.mac, pageOrigin: $0.page) } ?? reply.headers
                if jar?.credentials == true { self?.onCookies?() }
                // The body is delivered inflated and unframed, so its encoding and framing no longer apply.
                for name in headers.keys where ["content-encoding", "transfer-encoding", "connection"].contains(name.lowercased()) {
                    headers.removeValue(forKey: name)
                }
                for name in headers.keys where name.lowercased() == "content-length" { headers.removeValue(forKey: name) }
                if !streaming { headers["Content-Length"] = String(held.count) }
                guard let answer = HTTPURLResponse(url: page, statusCode: reply.status, httpVersion: "HTTP/1.1", headerFields: headers) else {
                    task.didFailWithError(URLError(.badServerResponse))
                    return
                }
                UILog.info("web pane \(pane) page \(shown) -> \(reply.status), \(streaming ? "streaming" : "\(held.count) bytes")")
                if task.request.mainDocumentURL == page { self?.onDocument?(reply.status) }
                task.didReceive(answer)
                if streaming {
                    // Flushing at every newline sends whole lines, so a page never sees half of one.
                    var pending = Data()
                    for try await chunk in reply.body {
                        guard !Task.isCancelled else { return }
                        pending.append(chunk)
                        if let last = pending.lastIndex(of: 0x0A) {
                            task.didReceive(Data(pending[pending.startIndex...last]))
                            pending.removeSubrange(pending.startIndex...last)
                        }
                    }
                    guard !Task.isCancelled else { return }
                    if !pending.isEmpty { task.didReceive(pending) }
                } else {
                    task.didReceive(held)
                }
                task.didFinish()
            } catch {
                guard !Task.isCancelled else { return }
                UILog.info("web pane \(pane) page \(shown) failed: \(WebPanes.describe(error))")
                task.didFailWithError(error)
            }
        }
        running[key] = (mine, work)
    }

    public func webView(_ webView: WKWebView, stop task: any WKURLSchemeTask) {
        running.removeValue(forKey: ObjectIdentifier(task))?.work.cancel()
    }
}

/// Hands a redirect back instead of following it, so the scheme handler sees each hop's cookies.
private final class Unredirected: NSObject, URLSessionTaskDelegate {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest) async -> URLRequest? { nil }
}
